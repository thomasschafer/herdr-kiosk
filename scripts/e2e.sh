#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
HK_ROOT=${HK_E2E_HOME:-/tmp/hk-m3}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-m3
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

make_repo() {
    local path=$1
    mkdir -p "$path"
    git -C "$path" init -q
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

t send-keys -t "$SESSION" C-c
wait_screen_absent "herdr-kiosk — select repo"
printf 'e2e: PASS\n'
