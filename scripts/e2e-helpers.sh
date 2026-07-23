#!/usr/bin/env bash

# Shared tmux harness helpers. Callers set HK_ROOT, HK_HOME_DIR, TMUX_SOCKET,
# SESSION, LAST_SCREEN, and HERDR_BIN before sourcing this file.

h() {
    env HOME="$HK_HOME_DIR" "$HERDR_BIN" "$@"
}

t() {
    "$TMUX_BIN" -S "$TMUX_SOCKET" "$@"
}

capture() {
    t capture-pane -p -t "$SESSION" >"$LAST_SCREEN"
}

fail() {
    printf 'e2e failure: %s\n' "$1" >&2
    if t has-session -t "$SESSION" 2>/dev/null; then
        capture || true
        printf '%s\n' '--- captured screen ---' >&2
        sed -n '1,160p' "$LAST_SCREEN" >&2 || true
        printf '%s\n' '--- plugin log ---' >&2
        h plugin log list --plugin thomasschafer.herdr-kiosk >&2 || true
    fi
    exit 1
}

cleanup() {
    h server stop >/dev/null 2>&1 || true
    t kill-server >/dev/null 2>&1 || true
}

wait_screen_contains() {
    local pattern=$1
    local attempts=${2:-80}
    local count
    for ((count = 0; count < attempts; count++)); do
        capture
        if grep -Fq "$pattern" "$LAST_SCREEN"; then
            return 0
        fi
        sleep 0.1
    done
    fail "screen did not contain: $pattern"
}

wait_screen_absent() {
    local pattern=$1
    local attempts=${2:-80}
    local count
    for ((count = 0; count < attempts; count++)); do
        capture
        if ! grep -Fq "$pattern" "$LAST_SCREEN"; then
            return 0
        fi
        sleep 0.1
    done
    fail "screen still contained: $pattern"
}

assert_screen_contains() {
    capture
    grep -Fq "$1" "$LAST_SCREEN" || fail "screen did not contain: $1"
}

assert_screen_absent() {
    capture
    if grep -Fq "$1" "$LAST_SCREEN"; then
        fail "screen unexpectedly contained: $1"
    fi
}

assert_screen_line_contains_all() {
    local anchor=$1
    shift
    local lines
    capture
    lines=$(grep -F "$anchor" "$LAST_SCREEN" || true)
    [ -n "$lines" ] || fail "screen had no line containing: $anchor"
    local pattern
    for pattern in "$@"; do
        printf '%s\n' "$lines" | grep -Fq "$pattern" \
            || fail "line containing '$anchor' did not also contain: $pattern"
    done
}

assert_screen_line_before() {
    local first=$1
    local second=$2
    local first_line
    local second_line
    capture
    first_line=$(grep -Fn -m1 "$first" "$LAST_SCREEN" | cut -d: -f1)
    second_line=$(grep -Fn -m1 "$second" "$LAST_SCREEN" | cut -d: -f1)
    [ -n "$first_line" ] || fail "screen had no line containing: $first"
    [ -n "$second_line" ] || fail "screen had no line containing: $second"
    [ "$first_line" -lt "$second_line" ] \
        || fail "'$first' was not before '$second'"
}

assert_focused_checkout() {
    local checkout=$1
    local workspaces
    workspaces=$(h workspace list)
    printf '%s' "$workspaces" | /usr/bin/python3 -c '
import json
import os
import sys

checkout = os.path.realpath(sys.argv[1])
workspaces = json.load(sys.stdin)["result"]["workspaces"]
if not any(
    workspace.get("focused")
    and os.path.realpath(workspace.get("worktree", {}).get("checkout_path", "")) == checkout
    for workspace in workspaces
):
    raise SystemExit(1)
' "$checkout" || fail "workspace for $checkout was not focused and grouped"
}

workspace_count() {
    h workspace list | /usr/bin/python3 -c '
import json
import sys

print(len(json.load(sys.stdin)["result"]["workspaces"]))
'
}

workspace_id_for_cwd() {
    local cwd=$1
    h pane list | /usr/bin/python3 -c '
import json
import os
import sys

cwd = os.path.realpath(sys.argv[1])
for pane in json.load(sys.stdin)["result"]["panes"]:
    pane_cwd = pane.get("cwd")
    if pane_cwd and os.path.realpath(pane_cwd) == cwd:
        print(pane["workspace_id"])
        break
else:
    raise SystemExit(1)
' "$cwd"
}

pane_id_for_workspace() {
    local workspace_id=$1
    h pane list | /usr/bin/python3 -c '
import json
import sys

workspace_id = sys.argv[1]
for pane in json.load(sys.stdin)["result"]["panes"]:
    if pane["workspace_id"] == workspace_id:
        print(pane["pane_id"])
        break
else:
    raise SystemExit(1)
' "$workspace_id"
}

workspace_id_other_than() {
    local excluded_workspace_id=$1
    h workspace list | /usr/bin/python3 -c '
import json
import sys

excluded_workspace_id = sys.argv[1]
for workspace in json.load(sys.stdin)["result"]["workspaces"]:
    workspace_id = workspace.get("workspace_id")
    if workspace_id and workspace_id != excluded_workspace_id:
        print(workspace_id)
        break
else:
    raise SystemExit(1)
' "$excluded_workspace_id"
}

wait_pane_cwd() {
    local cwd=$1
    local attempts=${2:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        if workspace_id_for_cwd "$cwd" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    fail "pane cwd did not resolve: $cwd"
}

wait_workspace_pane_cwd() {
    local workspace_id=$1
    local cwd=$2
    local attempts=${3:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        if h pane list | /usr/bin/python3 -c '
import json
import os
import sys

workspace_id = sys.argv[1]
cwd = os.path.realpath(sys.argv[2])
if not any(
    pane["workspace_id"] == workspace_id
    and pane.get("cwd")
    and os.path.realpath(pane["cwd"]) == cwd
    for pane in json.load(sys.stdin)["result"]["panes"]
):
    raise SystemExit(1)
' "$workspace_id" "$cwd"; then
            return 0
        fi
        sleep 0.1
    done
    fail "workspace pane cwd did not resolve: $workspace_id -> $cwd"
}

assert_focused_workspace() {
    local workspace_id=$1
    h workspace list | /usr/bin/python3 -c '
import json
import sys

workspace_id = sys.argv[1]
workspaces = json.load(sys.stdin)["result"]["workspaces"]
if not any(
    workspace.get("workspace_id") == workspace_id and workspace.get("focused")
    for workspace in workspaces
):
    raise SystemExit(1)
' "$workspace_id" || fail "workspace was not focused: $workspace_id"
}

assert_workspace_absent_checkout() {
    local checkout=$1
    h workspace list | /usr/bin/python3 -c '
import json
import os
import sys

checkout = os.path.realpath(sys.argv[1])
workspaces = json.load(sys.stdin)["result"]["workspaces"]
if any(
    os.path.realpath(workspace.get("worktree", {}).get("checkout_path", "")) == checkout
    for workspace in workspaces
):
    raise SystemExit(1)
' "$checkout" || fail "workspace for $checkout still existed"
}

wait_path_absent() {
    local path=$1
    local attempts=${2:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        if [ ! -e "$path" ]; then
            return 0
        fi
        sleep 0.1
    done
    fail "path still existed: $path"
}

wait_path_exists() {
    local path=$1
    local attempts=${2:-120}
    local count
    for ((count = 0; count < attempts; count++)); do
        if [ -e "$path" ]; then
            return 0
        fi
        sleep 0.1
    done
    fail "path did not appear: $path"
}

assert_branch_exists() {
    local repo=$1
    local branch=$2
    git -C "$repo" show-ref --verify --quiet "refs/heads/$branch" \
        || fail "branch no longer existed: $branch"
}

make_repo() {
    local path=$1
    mkdir -p "$path"
    git -C "$path" init -q -b master
    printf 'fixture\n' >"$path/README.md"
    git -C "$path" add README.md
    git -C "$path" -c user.name=E2E -c user.email=e2e@example.invalid commit -qm initial
}
