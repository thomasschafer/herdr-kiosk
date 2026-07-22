#!/usr/bin/env bash

# Shared tmux harness helpers. Callers set HK_ROOT, HK_HOME_DIR, TMUX_SOCKET,
# SESSION, LAST_SCREEN, and HERDR_BIN before sourcing this file.

h() {
    env HOME="$HK_HOME_DIR" "$HERDR_BIN" "$@"
}

t() {
    tmux -S "$TMUX_SOCKET" "$@"
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
