#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
HK_ROOT=${HK_E2E_HOME:-/tmp/hk-m7}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-m7
LAST_SCREEN="$HK_ROOT/last-screen.txt"
TMUX_BIN=${HK_TMUX:-$(command -v tmux || true)}
CARGO_PATH=/Users/tomschafer/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin
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
if [ ! -x "$TMUX_BIN" ]; then
    printf 'tmux binary is not executable: %s\n' "$TMUX_BIN" >&2
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
make_repo "$HK_ROOT/repos/UpperCase/open-me-upper"
mkdir -p "$HK_ROOT/plain-search/notes-folder"
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
(cd "$PROJECT_ROOT" && h plugin link . >/dev/null)
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
t send-keys -t "$SESSION" "$HK_ROOT/repos/"
t send-keys -t "$SESSION" -l "UpperCase"
wait_screen_contains "$HK_ROOT/repos/UpperCase"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Depth"
assert_screen_contains "Search directories"
assert_screen_contains "$HK_ROOT/repos/UpperCase"
assert_screen_contains "No directories added yet"
t send-keys -t "$SESSION" 1 0
t send-keys -t "$SESSION" Enter
wait_screen_contains "Folder inclusion"
assert_screen_contains "Git repositories only (default)"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Search directories"
wait_screen_contains "$HK_ROOT/repos/UpperCase  depth 10"
t send-keys -t "$SESSION" Enter
wait_screen_contains "Confirm setup"
t send-keys -t "$SESSION" Enter
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "open-me"
[ -f "$PLUGIN_CONFIG_DIR/config.toml" ] || fail "wizard did not create config.toml"
grep -Fq "path = \"$HK_ROOT/repos/UpperCase\"" "$PLUGIN_CONFIG_DIR/config.toml" \
    || fail "wizard config did not contain fixture search directory"
grep -Fq "depth = 10" "$PLUGIN_CONFIG_DIR/config.toml" \
    || fail "wizard config did not contain selected depth"
t send-keys -t "$SESSION" C-c
wait_screen_absent "herdr-kiosk — select repo"

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
assert_screen_absent "Welcome to herdr-kiosk"
wait_screen_contains "open-me"
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
  { path = "$HK_ROOT/plain-search", depth = 1, include_non_git = true },
]
EOF
FOLDER_WORKSPACE_COUNT_BEFORE=$(workspace_count)
h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "notes-folder"
wait_screen_absent "scanning…"
assert_screen_line_contains_all "notes-folder" "dir"
t send-keys -t "$SESSION" notes-folder
wait_screen_contains "1 of 1 repos"
t send-keys -t "$SESSION" Enter
wait_screen_absent "herdr-kiosk — select repo" 120
wait_pane_cwd "$HK_ROOT/plain-search/notes-folder"
FOLDER_WORKSPACE_ID=$(workspace_id_for_cwd "$HK_ROOT/plain-search/notes-folder")
[ -n "$FOLDER_WORKSPACE_ID" ] || fail "plain folder workspace id was empty"
assert_focused_workspace "$FOLDER_WORKSPACE_ID"
FOLDER_WORKSPACE_COUNT_CREATED=$(workspace_count)
[ "$FOLDER_WORKSPACE_COUNT_CREATED" = "$((FOLDER_WORKSPACE_COUNT_BEFORE + 1))" ] \
    || fail "opening a plain folder did not create exactly one workspace"

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" notes-folder
wait_screen_contains "1 of 1 repos"
wait_screen_contains "● open"
t send-keys -t "$SESSION" Enter
wait_screen_absent "herdr-kiosk — select repo" 120
assert_focused_workspace "$FOLDER_WORKSPACE_ID"
FOLDER_WORKSPACE_COUNT_REOPENED=$(workspace_count)
[ "$FOLDER_WORKSPACE_COUNT_REOPENED" = "$FOLDER_WORKSPACE_COUNT_CREATED" ] \
    || fail "reopening a plain folder created a duplicate workspace"
printf 'plain folder discovery marker, create, focus, and reopen idempotence: ok\n'

write_picker_config() {
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
}

ON_OPEN_SENTINEL="$HK_ROOT/on-open-sentinel"
write_picker_config
cat >>"$PLUGIN_CONFIG_DIR/config.toml" <<EOF

[on_open]
panes = [
  { command = "printf ONOPEN_OK > $ON_OPEN_SENTINEL", direction = "right" },
]
EOF

h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "repo-same (…/alpha)"
assert_screen_contains "repo-same (…/beta)"
assert_screen_contains "nested-repo"
assert_screen_contains "open-me"
wait_screen_absent "scanning…"
printf 'discovery and collision display: ok\n'

t send-keys -t "$SESSION" -l "é界"
wait_screen_contains "é界"
t send-keys -t "$SESSION" Escape
wait_screen_contains "4 of 4 repos"
printf 'unicode picker input: ok\n'

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

wait_path_exists "$ON_OPEN_SENTINEL"
[ "$(cat "$ON_OPEN_SENTINEL")" = "ONOPEN_OK" ] \
    || fail "on_open split pane did not write the expected sentinel"
OPEN_WORKSPACE_ID=$(printf '%s' "$WORKSPACES" | /usr/bin/python3 -c '
import json
import os
import sys

checkout = os.path.realpath(sys.argv[1])
for workspace in json.load(sys.stdin)["result"]["workspaces"]:
    if os.path.realpath(workspace.get("worktree", {}).get("checkout_path", "")) == checkout:
        print(workspace["workspace_id"])
        break
' "$HK_ROOT/repos/direct/open-me")
[ -n "$OPEN_WORKSPACE_ID" ] || fail "could not find on_open workspace id"
h pane list --workspace "$OPEN_WORKSPACE_ID" | /usr/bin/python3 -c '
import json
import sys

panes = json.load(sys.stdin)["result"]["panes"]
if len(panes) != 2:
    raise SystemExit(f"expected 2 panes, got {len(panes)}")
if sum(bool(pane.get("focused")) for pane in panes) != 1:
    raise SystemExit("expected exactly one focused pane")
' || fail "on_open pane count or focus was incorrect"

rm -- "$ON_OPEN_SENTINEL"
h plugin action invoke open-picker --plugin thomasschafer.herdr-kiosk >/dev/null
wait_screen_contains "herdr-kiosk — select repo"
t send-keys -t "$SESSION" open-me
wait_screen_contains "1 of 4 repos"
t send-keys -t "$SESSION" Enter
wait_screen_absent "herdr-kiosk — select repo" 120
sleep 0.2
[ ! -e "$ON_OPEN_SENTINEL" ] \
    || fail "on_open command ran again when refocusing the repository"
h pane list --workspace "$OPEN_WORKSPACE_ID" | /usr/bin/python3 -c '
import json
import sys

panes = json.load(sys.stdin)["result"]["panes"]
if len(panes) != 2:
    raise SystemExit(f"expected 2 panes after reopen, got {len(panes)}")
' || fail "on_open split was duplicated when refocusing the repository"
write_picker_config
printf 'on_open split, command, popup teardown, focus, and reopen idempotence: ok\n'

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
assert_screen_contains "ctrl+b"
assert_screen_contains "Create a new branch"
t send-keys -t "$SESSION" -l "delete"
wait_screen_absent "Create a new branch"
assert_screen_contains "(delete)"
assert_screen_contains "Delete the selected checkout"
t send-keys -t "$SESSION" Escape
wait_screen_absent "Help — active key bindings"
assert_screen_contains "open-me — select branch"
printf 'help overlay uses remapped lowercase bindings, filters, and Esc returns: ok\n'

t send-keys -t "$SESSION" plain
wait_screen_contains "1 of 6 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120

PLAIN_WORKTREE="$HK_ROOT/worktrees/open-me/plain"
wait_path_exists "$PLAIN_WORKTREE"
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

git -C "$HK_ROOT/repos/direct/open-me" remote add origin "$HK_ROOT/remote.git"
git -C "$HK_ROOT/repos/direct/open-me" remote add upstream "$HK_ROOT/remote.git"
t send-keys -t "$SESSION" Tab
wait_screen_contains "open-me — select branch"
wait_screen_contains "upstream/remote-only" 120
wait_screen_contains "origin/remote-only" 120
assert_screen_contains "upstream/remote-only"
capture
[ "$(grep -Fc "master (worktree)" "$LAST_SCREEN")" = 1 ] \
    || fail "local master branch row was not unique"
if grep -Eq '(origin|upstream)/master' "$LAST_SCREEN"; then
    fail "branch present locally and remotely was duplicated"
fi
printf 'same-named remote branch streaming and local dedup: ok\n'

t send-keys -t "$SESSION" upstream/remote-only
wait_screen_contains "1 of 8 branches"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120

REMOTE_WORKTREE="$HK_ROOT/worktrees/open-me/remote-only"
wait_path_exists "$REMOTE_WORKTREE"
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
assert_screen_contains "open-me — select branch"
t send-keys -t "$SESSION" featurx
wait_screen_contains "0 bases"
t send-keys -t "$SESSION" BSpace
wait_screen_contains "1 bases"
assert_screen_contains "feature"
t send-keys -t "$SESSION" Escape
wait_screen_contains "7 bases"
t send-keys -t "$SESSION" Down
t send-keys -t "$SESSION" C-n
t send-keys -t "$SESSION" Down
t send-keys -t "$SESSION" C-n
t send-keys -t "$SESSION" C-p
t send-keys -t "$SESSION" Up
t send-keys -t "$SESSION" C-n
sleep 0.2
assert_screen_line_contains_all "feature" "▸"
t send-keys -t "$SESSION" Enter
wait_screen_absent "open-me — select branch" 120

NEW_BRANCH_WORKTREE="$HK_ROOT/worktrees/open-me/feat-new-branch"
wait_path_exists "$NEW_BRANCH_WORKTREE"
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
printf 'new branch base editing, arrow/Ctrl navigation, worktree, and focus: ok\n'

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
wait_path_exists "$DELETE_OPEN_WORKTREE"
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
wait_path_exists "$DIRTY_OPEN_WORKTREE"
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
