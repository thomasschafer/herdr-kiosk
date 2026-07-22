#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
HK_ROOT=${HK_E2E_HOME:-/tmp/hk-m7}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-m7
LAST_SCREEN="$HK_ROOT/last-screen.txt"
CARGO_PATH=/Users/tomschafer/.cargo/bin:/etc/profiles/per-user/tomschafer/bin:/usr/bin:/bin:/usr/sbin:/sbin
export PATH="$CARGO_PATH"

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

# shellcheck source=scripts/e2e-helpers.sh
source "$PROJECT_ROOT/scripts/e2e-helpers.sh"
trap cleanup EXIT

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
printf 'feature tip\n' >"$HK_ROOT/existing-worktrees/feature/FEATURE.md"
git -C "$HK_ROOT/existing-worktrees/feature" add FEATURE.md
git -C "$HK_ROOT/existing-worktrees/feature" \
    -c user.name=E2E -c user.email=e2e@example.invalid commit -qm 'feature tip'
git -C "$HK_ROOT/repos/direct/open-me" branch closed-checkout
git -C "$HK_ROOT/repos/direct/open-me" branch delete-open
git -C "$HK_ROOT/repos/direct/open-me" branch dirty-open
git -C "$HK_ROOT/repos/direct/open-me" worktree add -q \
    "$HK_ROOT/existing-worktrees/closed-checkout" closed-checkout

mkdir -p "$HK_ROOT/remote.git"
git -C "$HK_ROOT/remote.git" init -q --bare
make_repo "$HK_ROOT/remote-seed"
git -C "$HK_ROOT/remote-seed" branch remote-only
git -C "$HK_ROOT/remote-seed" remote add upstream "$HK_ROOT/remote.git"
git -C "$HK_ROOT/remote-seed" push -q upstream master remote-only

printf 'building plugin...\n'
(cd "$PROJECT_ROOT" && env PATH="$CARGO_PATH" cargo build --release)
h plugin link "$PROJECT_ROOT" >/dev/null
PLUGIN_CONFIG_DIR=$(h plugin config-dir thomasschafer.herdr-kiosk)
mkdir -p "$PLUGIN_CONFIG_DIR"

printf 'starting herdr...\n'
t new-session -d -s "$SESSION" -x 200 -y 50 \
    "env HOME='$HK_HOME_DIR' '$HERDR_BIN'"
sleep 2
t send-keys -t "$SESSION" Enter
sleep 0.5
t send-keys -t "$SESSION" Escape
sleep 0.5

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "Welcome to herdr-kiosk"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Search directories"
t send-keys -t "$SESSION" "$HK_ROOT/repos/direct"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Scan depth"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Search directories"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Confirm setup"
t send-keys -t "$SESSION" Enter
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "open-me"
[ -f "$PLUGIN_CONFIG_DIR/config.toml" ] || fail "wizard did not create config.toml"
grep -Fq "path = \"$HK_ROOT/repos/direct\"" "$PLUGIN_CONFIG_DIR/config.toml" \
    || fail "wizard config did not contain fixture search directory"
grep -Fq "depth = 1" "$PLUGIN_CONFIG_DIR/config.toml" \
    || fail "wizard config did not contain selected depth"
t send-keys -t "$SESSION" C-c
wait_screen_absent "herdr-kiosk — select repo"

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
assert_screen_absent "Welcome to herdr-kiosk"
assert_screen_contains "open-me"
t send-keys -t "$SESSION" C-c
wait_screen_absent "herdr-kiosk — select repo"
printf 'first-run wizard writes config, continues, and does not reappear: ok\n'

printf 'search_dirs = []\n' >"$PLUGIN_CONFIG_DIR/config.toml"
h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "No search directories configured"
assert_screen_contains "$PLUGIN_CONFIG_DIR/config.toml"
assert_screen_absent "Welcome to herdr-kiosk"
t send-keys -t "$SESSION" C-c
wait_screen_absent "No search directories configured"
printf 'existing empty search_dirs keeps the explicit empty-config screen: ok\n'

cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [
  { path = "$HK_ROOT/repos/alpha", depth = 1 },
  { path = "$HK_ROOT/repos/beta", depth = 1 },
  { path = "$HK_ROOT/repos/deep/level-one", depth = 2 },
  { path = "$HK_ROOT/repos/direct", depth = 1 },
]

[keys.branch_select]
"C-b" = "new_branch"
"C-o" = "noop"
EOF

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

t send-keys -t "$SESSION" C-h
wait_screen_contains "Help — active key bindings"
assert_screen_contains "Ctrl+B"
assert_screen_contains "Create a new branch"
t send-keys -t "$SESSION" Escape
wait_screen_absent "Help — active key bindings"
assert_screen_contains "open-me — select branch"
printf 'help overlay uses remapped bindings and Esc returns: ok\n'

t send-keys -t "$SESSION" plain
wait_screen_contains "1 of 6 branches"
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
wait_screen_contains "1 of 6 branches"
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
wait_screen_contains "1 of 6 branches"
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

git -C "$HK_ROOT/repos/direct/open-me" remote add upstream "$HK_ROOT/remote.git"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
wait_screen_contains "remote-only (remote)" 120
assert_screen_line_contains_all "remote-only" "(remote)"
capture
[ "$(grep -Fc "master (worktree)" "$LAST_SCREEN")" = 1 ] \
    || fail "local master branch row was not unique"
if grep -Fq "master (remote)" "$LAST_SCREEN"; then
    fail "branch present locally and remotely was duplicated"
fi
printf 'remote branch streaming and local dedup: ok\n'

t send-keys -t "$SESSION" remote-only
wait_screen_contains "1 of 7 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120

REMOTE_WORKTREE="$HK_ROOT/worktrees/open-me/remote-only"
[ -d "$REMOTE_WORKTREE" ] || fail "remote-only worktree was not created"
assert_focused_checkout "$REMOTE_WORKTREE"
UPSTREAM=$(git -C "$HK_ROOT/repos/direct/open-me" rev-parse --abbrev-ref 'remote-only@{upstream}')
[ "$UPSTREAM" = "upstream/remote-only" ] \
    || fail "remote-only did not track upstream/remote-only: $UPSTREAM"
printf 'remote tracking checkout and focus: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" feat/new-branch
wait_screen_contains "0 of 7 branches"
t send-keys -t "$SESSION" C-b
wait_screen_contains 'New branch "feat/new-branch" — pick base'
t send-keys -t "$SESSION" feature
wait_screen_contains "feature"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120

NEW_BRANCH_WORKTREE="$HK_ROOT/worktrees/open-me/feat-new-branch"
[ -d "$NEW_BRANCH_WORKTREE" ] || fail "new branch worktree was not created"
assert_focused_checkout "$NEW_BRANCH_WORKTREE"
FEATURE_TIP=$(git -C "$HK_ROOT/repos/direct/open-me" rev-parse feature)
NEW_BRANCH_TIP=$(git -C "$HK_ROOT/repos/direct/open-me" rev-parse feat/new-branch)
MASTER_TIP=$(git -C "$HK_ROOT/repos/direct/open-me" rev-parse master)
[ "$NEW_BRANCH_TIP" = "$FEATURE_TIP" ] \
    || fail "feat/new-branch was not created from feature"
[ "$NEW_BRANCH_TIP" != "$MASTER_TIP" ] \
    || fail "feat/new-branch unexpectedly used master as its base"
git -C "$HK_ROOT/repos/direct/open-me" merge-base --is-ancestor feature feat/new-branch \
    || fail "feature was not an ancestor of feat/new-branch"
printf 'new branch base selection, worktree, and focus: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" closed-checkout
wait_screen_contains "1 of 8 branches"
t send-keys -t "$SESSION" C-x
wait_screen_contains "$HK_ROOT/existing-worktrees/closed-checkout"
t send-keys -t "$SESSION" Enter
wait_path_absent "$HK_ROOT/existing-worktrees/closed-checkout"
assert_branch_exists "$HK_ROOT/repos/direct/open-me" closed-checkout
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" C-c
wait_screen_absent "open-me — select branch"
printf 'closed checkout deletion preserves branch: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" delete-open
wait_screen_contains "1 of 8 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120
DELETE_OPEN_WORKTREE="$HK_ROOT/worktrees/open-me/delete-open"
[ -d "$DELETE_OPEN_WORKTREE" ] || fail "delete-open worktree was not created"
assert_focused_checkout "$DELETE_OPEN_WORKTREE"

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" delete-open
wait_screen_contains "● open"
t send-keys -t "$SESSION" C-x
wait_screen_contains "Its herdr workspace will also be closed."
t send-keys -t "$SESSION" Enter
wait_path_absent "$DELETE_OPEN_WORKTREE"
assert_workspace_absent_checkout "$DELETE_OPEN_WORKTREE"
assert_branch_exists "$HK_ROOT/repos/direct/open-me" delete-open
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" C-c
wait_screen_absent "open-me — select branch"
printf 'open workspace deletion preserves branch: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" dirty-open
wait_screen_contains "1 of 8 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120
DIRTY_OPEN_WORKTREE="$HK_ROOT/worktrees/open-me/dirty-open"
[ -d "$DIRTY_OPEN_WORKTREE" ] || fail "dirty-open worktree was not created"
printf 'untracked\n' >"$DIRTY_OPEN_WORKTREE/untracked.txt"

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" dirty-open
wait_screen_contains "● open"
t send-keys -t "$SESSION" C-x
wait_screen_contains "Its herdr workspace will also be closed."
t send-keys -t "$SESSION" Enter
wait_screen_contains "This checkout has uncommitted changes."
[ -d "$DIRTY_OPEN_WORKTREE" ] || fail "dirty checkout disappeared before force confirmation"
t send-keys -t "$SESSION" Enter
wait_path_absent "$DIRTY_OPEN_WORKTREE"
assert_workspace_absent_checkout "$DIRTY_OPEN_WORKTREE"
assert_branch_exists "$HK_ROOT/repos/direct/open-me" dirty-open
wait_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" C-c
wait_screen_absent "open-me — select branch"
printf 'dirty herdr checkout force confirmation and deletion: ok\n'

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" C-c
wait_screen_absent "herdr-kiosk — select repo"
printf 'e2e: PASS\n'
