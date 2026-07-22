#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
HK_ROOT=${HK_E2E_HOME:-/tmp/hk-m4}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-m4
LAST_SCREEN="$HK_ROOT/last-screen.txt"
CARGO_PATH=/Users/tomschafer/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin

case "$HK_ROOT" in
    /tmp/* | /private/tmp/*) ;;
    *)
        printf 'HK_E2E_HOME must be below /tmp or /private/tmp: %s\n' "$HK_ROOT" >&2
        exit 2
        ;;
esac

if [ ! -x "$HERDR_BIN" ]; then
    printf 'Herdr binary is not executable: %s\n' "$HERDR_BIN" >&2
    exit 2
fi

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
trap cleanup EXIT

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

make_repo() {
    local path=$1
    mkdir -p "$path"
    git -C "$path" init -q -b master
    printf 'fixture\n' >"$path/README.md"
    git -C "$path" add README.md
    git -C "$path" -c user.name=E2E -c user.email=e2e@example.invalid commit -qm initial
}

cleanup
rm -rf -- "$HK_ROOT"
mkdir -p "$HK_HOME_DIR/.config/herdr" "$HK_ROOT/worktrees"

cat >"$HK_HOME_DIR/.config/herdr/config.toml" <<EOF
[worktrees]
directory = "$HK_ROOT/worktrees"
EOF

make_repo "$HK_ROOT/repos/alpha/repo-same"
make_repo "$HK_ROOT/repos/beta/repo-same"
make_repo "$HK_ROOT/repos/deep/level-one/level-two/nested-repo"
make_repo "$HK_ROOT/repos/direct/open-me"
git -C "$HK_ROOT/repos/direct/open-me" branch feature
git -C "$HK_ROOT/repos/direct/open-me" branch plain
mkdir -p "$HK_ROOT/existing-worktrees"
git -C "$HK_ROOT/repos/direct/open-me" worktree add -q \
    "$HK_ROOT/existing-worktrees/feature" feature

printf 'building plugin...\n'
(cd "$PROJECT_ROOT" && env PATH="$CARGO_PATH" cargo build --release)
h plugin link "$PROJECT_ROOT" >/dev/null
PLUGIN_CONFIG_DIR=$(h plugin config-dir thomasschafer.herdr-kiosk)
mkdir -p "$PLUGIN_CONFIG_DIR"
cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [
  { path = "$HK_ROOT/repos/alpha", depth = 1 },
  { path = "$HK_ROOT/repos/beta", depth = 1 },
  { path = "$HK_ROOT/repos/deep/level-one", depth = 2 },
  { path = "$HK_ROOT/repos/direct", depth = 1 },
]
EOF

printf 'starting herdr...\n'
t new-session -d -s "$SESSION" -x 200 -y 50 \
    "env HOME='$HK_HOME_DIR' '$HERDR_BIN'"
sleep 2
t send-keys -t "$SESSION" Enter
sleep 0.5
t send-keys -t "$SESSION" Escape
sleep 0.5

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "repo-same (…/alpha)"
assert_screen_contains "repo-same (…/beta)"
assert_screen_contains "nested-repo"
assert_screen_contains "open-me"
wait_screen_absent "scanning…"
printf 'discovery and collision display: ok\n'

t send-keys -t "$SESSION" nested
wait_screen_contains "1 of 4 repos"
assert_screen_contains "nested-repo"
assert_screen_absent "open-me"
printf 'fuzzy filtering: ok\n'

t send-keys -t "$SESSION" Escape
sleep 0.2
wait_screen_contains "4 of 4 repos"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
assert_screen_contains "open-me"
t send-keys -t "$SESSION" Enter
wait_screen_absent "herdr-kiosk — select repo" 120

WORKSPACES=$(h workspace list)
printf '%s' "$WORKSPACES" | grep -Fq "$HK_ROOT/repos/direct/open-me" \
    || fail "workspace list did not contain opened repository"
printf '%s' "$WORKSPACES" | grep -Fq '"focused":true' \
    || printf '%s' "$WORKSPACES" | grep -Fq '"focused": true' \
    || fail "opened repository workspace was not focused"
printf 'repo open and focus: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "● open"
assert_screen_contains "open-me"
printf 'open indicator: ok\n'

t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
wait_screen_contains "feature"
wait_screen_contains "plain"
wait_screen_contains "master"
wait_screen_absent "loading…"
assert_screen_line_contains_all "feature" "(worktree)"
assert_screen_line_contains_all "master" "(worktree)" "*" "(default)"
printf 'branch listing and markers: ok\n'

t send-keys -t "$SESSION" plain
wait_screen_contains "1 of 3 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120

PLAIN_WORKTREE="$HK_ROOT/worktrees/open-me/plain"
[ -d "$PLAIN_WORKTREE" ] || fail "plain worktree was not created under sandbox worktrees"
assert_focused_checkout "$PLAIN_WORKTREE"
printf 'plain branch create and focus: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" plain
wait_screen_contains "1 of 3 branches"
wait_screen_contains "● open"
assert_screen_line_contains_all "plain" "(worktree)" "● open"
WORKSPACE_COUNT_BEFORE=$(workspace_count)
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120
assert_focused_checkout "$PLAIN_WORKTREE"
WORKSPACE_COUNT_AFTER=$(workspace_count)
[ "$WORKSPACE_COUNT_AFTER" = "$WORKSPACE_COUNT_BEFORE" ] \
    || fail "reopening plain created a duplicate workspace"
printf 'plain branch reopen focuses existing workspace: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" master
wait_screen_contains "1 of 3 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120
assert_focused_checkout "$HK_ROOT/repos/direct/open-me"
printf 'main checkout branch focuses source workspace: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" Escape
wait_screen_contains "herdr-kiosk — select repo"
assert_screen_absent "open-me — select branch"
printf 'branch escape returns to repo picker: ok\n'

t send-keys -t "$SESSION" C-c
wait_screen_absent "herdr-kiosk — select repo"
printf 'e2e: PASS\n'
