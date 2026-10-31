#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=scripts/harness-env.sh
source "$PROJECT_ROOT/scripts/harness-env.sh"
HK_ROOT=${HK_E2E_HOME:-/tmp/hk-m7}
HERDR_BIN=${HERDR:-"$PROJECT_ROOT/../herdr/target/release/herdr"}
HK_HOME_DIR="$HK_ROOT/home"
TMUX_SOCKET="$HK_ROOT/tmux.sock"
SESSION=hk-m7
LAST_SCREEN="$HK_ROOT/last-screen.txt"
TMUX_BIN=${HK_TMUX:-$(command -v tmux || true)}
CARGO_PATH=$HARNESS_CARGO_BIN:/usr/bin:/bin:/usr/sbin:/sbin
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
mkdir -p "$HK_HOME_DIR"

printf 'building plugin...\n'
(cd "$PROJECT_ROOT" && cargo build --release)

# Cargo needed the real HOME for its toolchains. Everything from here on, git included,
# sees only the sandbox.
export HOME="$HK_HOME_DIR"
write_herdr_config

REPO="$HK_ROOT/repos/direct/open-me"
make_repo "$HK_ROOT/repos/alpha/repo-same"
make_repo "$HK_ROOT/repos/beta/repo-same"
make_repo "$HK_ROOT/repos/deep/level-one/level-two/nested-repo"
make_repo "$REPO"
make_repo "$HK_ROOT/repos/UpperCase/open-me-upper"
mkdir -p "$HK_ROOT/plain-search/notes-folder"
git -C "$REPO" branch feature
git -C "$REPO" branch plain
mkdir -p "$HK_ROOT/existing-worktrees"
git -C "$REPO" worktree add -q "$HK_ROOT/existing-worktrees/feature" feature
printf 'feature tip\n' >"$HK_ROOT/existing-worktrees/feature/FEATURE.md"
git -C "$HK_ROOT/existing-worktrees/feature" add FEATURE.md
git -C "$HK_ROOT/existing-worktrees/feature" \
    -c user.name=E2E -c user.email=e2e@example.invalid commit -qm 'feature tip'
git -C "$REPO" branch closed-checkout
git -C "$REPO" branch delete-open
git -C "$REPO" branch dirty-open
git -C "$REPO" worktree add -q "$HK_ROOT/existing-worktrees/closed-checkout" closed-checkout

mkdir -p "$HK_ROOT/remote.git"
git -C "$HK_ROOT/remote.git" init -q --bare
make_repo "$HK_ROOT/remote-seed"
git -C "$HK_ROOT/remote-seed" branch remote-only
git -C "$HK_ROOT/remote-seed" remote add upstream "$HK_ROOT/remote.git"
git -C "$HK_ROOT/remote-seed" push -q upstream master remote-only

(cd "$PROJECT_ROOT" && h plugin link . >/dev/null)
PLUGIN_CONFIG_DIR=$(h plugin config-dir "$PLUGIN_ID")
mkdir -p "$PLUGIN_CONFIG_DIR"

printf 'starting herdr...\n'
start_herdr 200 50

open_picker() {
    invoke_picker
    wait_screen_contains "herdr-kiosk — select repo"
}

# Quits the picker with ctrl+c and waits for the given title to leave the screen.
quit_picker() {
    press C-c
    wait_screen_absent "$1"
}

# Waits for the branch list to finish loading local and remote branches.
wait_branches_settled() {
    wait_screen_absent "loading…"
    wait_screen_absent "fetching…"
}

# Opens the branch view of the named repository and waits for its branches.
show_branches() {
    open_picker
    type_text "$1"
    wait_screen_contains "1 of 4 repos"
    press Tab
    wait_screen_contains "$1 — select branch"
    wait_branches_settled
}

# Opens open-me's branch view and waits until it knows which checkouts herdr shows,
# which deleting a checkout needs. The main checkout is open by the time this is used.
open_branch_view() {
    show_branches open-me
    wait_screen_line_contains_all "master" "● open"
}

focus_checkout() {
    h workspace focus "$(query_workspaces checkout "$1")" >/dev/null
    wait_focused_checkout "$1"
}

# Opens the named branch from open-me's branch view and waits for the picker to go.
open_branch() {
    open_branch_view
    type_text "$1"
    wait_screen_line_contains_all "▸" "$1"
    press Enter
    wait_screen_absent "open-me — select branch"
}

invoke_picker
wait_screen_contains "Welcome to herdr-kiosk"
press Enter
wait_screen_contains "Search directories"
type_text "$HK_ROOT/repos/UpperCase"
wait_screen_contains "$HK_ROOT/repos/UpperCase"
press Enter
wait_screen_contains "Depth"
assert_screen_contains "Search directories"
assert_screen_contains "$HK_ROOT/repos/UpperCase"
assert_screen_contains "No directories added yet"
type_text 10
press Enter
wait_screen_contains "Folder inclusion"
assert_screen_contains "Git repositories only (default)"
press Enter
wait_screen_contains "Search directories"
wait_screen_contains "$HK_ROOT/repos/UpperCase  depth 10"
press Enter
wait_screen_contains "Confirm setup"
press Enter
wait_screen_contains "herdr-kiosk — select repo"
wait_screen_contains "open-me"
[ -f "$PLUGIN_CONFIG_DIR/config.toml" ] || fail "wizard did not create config.toml"
grep -Fq "path = \"$HK_ROOT/repos/UpperCase\"" "$PLUGIN_CONFIG_DIR/config.toml" \
    || fail "wizard config did not contain fixture search directory"
grep -Fq "depth = 10" "$PLUGIN_CONFIG_DIR/config.toml" \
    || fail "wizard config did not contain selected depth"
quit_picker "herdr-kiosk — select repo"

open_picker
assert_screen_absent "Welcome to herdr-kiosk"
wait_screen_contains "open-me"
quit_picker "herdr-kiosk — select repo"
printf 'first-run wizard writes config, continues, and does not reappear: ok\n'

printf 'search_dirs = []\n' >"$PLUGIN_CONFIG_DIR/config.toml"
invoke_picker
wait_screen_contains "No search directories configured"
assert_screen_contains "$PLUGIN_CONFIG_DIR/config.toml"
assert_screen_absent "Welcome to herdr-kiosk"
quit_picker "No search directories configured"
printf 'existing empty search_dirs keeps the explicit empty-config screen: ok\n'

cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [
  { path = "$HK_ROOT/absent-optional", optional = true },
]
EOF
open_picker
wait_screen_contains "No repos found (1 optional search directory not found)"
assert_screen_absent "does not exist"
quit_picker "herdr-kiosk — select repo"

cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [
  { path = "$HK_ROOT/absent-required" },
]
EOF
open_picker
wait_screen_contains "No repos found in the configured search directories"
wait_screen_contains "does not exist"
quit_picker "herdr-kiosk — select repo"
printf 'missing optional search directories are silent; required ones warn: ok\n'

cat >"$PLUGIN_CONFIG_DIR/config.toml" <<EOF
search_dirs = [
  { path = "$HK_ROOT/plain-search", depth = 1, include_non_git = true },
]
EOF
FOLDER_WORKSPACE_COUNT_BEFORE=$(workspace_count)
open_picker
wait_screen_contains "notes-folder"
wait_screen_absent "scanning…"
assert_screen_line_contains_all "notes-folder" "dir"
type_text notes-folder
wait_screen_contains "1 of 1 repos"
press Enter
wait_screen_absent "herdr-kiosk — select repo"
wait_pane_cwd "$HK_ROOT/plain-search/notes-folder"
FOLDER_WORKSPACE_ID=$(workspace_id_for_cwd "$HK_ROOT/plain-search/notes-folder")
wait_focused_workspace "$FOLDER_WORKSPACE_ID"
FOLDER_WORKSPACE_COUNT_CREATED=$(workspace_count)
[ "$FOLDER_WORKSPACE_COUNT_CREATED" = "$((FOLDER_WORKSPACE_COUNT_BEFORE + 1))" ] \
    || fail "opening a plain folder did not create exactly one workspace"

open_picker
type_text notes-folder
wait_screen_contains "1 of 1 repos"
wait_screen_contains "● open"
press Enter
wait_screen_absent "herdr-kiosk — select repo"
wait_focused_workspace "$FOLDER_WORKSPACE_ID"
[ "$(workspace_count)" = "$FOLDER_WORKSPACE_COUNT_CREATED" ] \
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

open_picker
wait_screen_contains "repo-same (…/alpha)"
assert_screen_contains "repo-same (…/beta)"
assert_screen_contains "nested-repo"
assert_screen_contains "open-me"
wait_screen_absent "scanning…"
printf 'discovery and collision display: ok\n'

type_text "é界"
wait_screen_contains "é界"
press Escape
wait_screen_contains "4 of 4 repos"
printf 'unicode picker input: ok\n'

type_text nested
wait_screen_contains "1 of 4 repos"
assert_screen_contains "nested-repo"
assert_screen_absent "open-me"
printf 'fuzzy filtering: ok\n'

press Escape
wait_screen_contains "4 of 4 repos"
type_text open-me
wait_screen_contains "1 of 4 repos"
assert_screen_contains "open-me"
press Enter
wait_screen_absent "herdr-kiosk — select repo"
wait_focused_checkout "$REPO"
printf 'repo open and focus: ok\n'

on_open_pane_count_is() {
    h pane list --workspace "$OPEN_WORKSPACE_ID" | /usr/bin/python3 -c '
import json
import sys

panes = json.load(sys.stdin)["result"]["panes"]
sys.exit(0 if len(panes) == 2 and sum(bool(pane.get("focused")) for pane in panes) == 1 else 1)
'
}

wait_path_exists "$ON_OPEN_SENTINEL"
[ "$(cat "$ON_OPEN_SENTINEL")" = "ONOPEN_OK" ] \
    || fail "on_open split pane did not write the expected sentinel"
OPEN_WORKSPACE_ID=$(query_workspaces checkout "$REPO")
wait_for "the on_open split to leave two panes, one focused" on_open_pane_count_is

rm -- "$ON_OPEN_SENTINEL"
open_picker
type_text open-me
wait_screen_contains "1 of 4 repos"
press Enter
wait_screen_absent "herdr-kiosk — select repo"
wait_focused_checkout "$REPO"
# kiosk has exited once its popup closes, so a second split would already be there.
on_open_pane_count_is || fail "on_open split was duplicated when refocusing the repository"
[ ! -e "$ON_OPEN_SENTINEL" ] \
    || fail "on_open command ran again when refocusing the repository"
write_picker_config
printf 'on_open split, command, popup teardown, focus, and reopen idempotence: ok\n'

open_picker
type_text open-me
wait_screen_line_contains_all "open-me" "● open"
printf 'open indicator: ok\n'

press Tab
wait_screen_contains "open-me — select branch"
wait_branches_settled
wait_screen_contains "feature"
wait_screen_contains "plain"
assert_screen_line_contains_all "feature" "(worktree)"
assert_screen_line_contains_all "master" "(worktree)" "*" "(default)"
printf 'branch listing and markers: ok\n'

press C-h
wait_screen_contains "Help — active key bindings"
assert_screen_contains "ctrl+b"
assert_screen_contains "Create a new branch"
type_text delete
wait_screen_absent "Create a new branch"
assert_screen_contains "(delete)"
assert_screen_contains "Delete the selected checkout"
press Escape
wait_screen_absent "Help — active key bindings"
assert_screen_contains "open-me — select branch"
printf 'help overlay uses remapped lowercase bindings, filters, and Esc returns: ok\n'

type_text plain
wait_screen_contains "1 of 6 branches"
press Enter
wait_screen_absent "open-me — select branch"

PLAIN_WORKTREE="$HK_ROOT/worktrees/open-me/plain"
wait_path_exists "$PLAIN_WORKTREE"
wait_focused_checkout "$PLAIN_WORKTREE"
printf 'plain branch create and focus: ok\n'

open_branch_view
type_text plain
wait_screen_contains "1 of 6 branches"
wait_screen_line_contains_all "plain" "(worktree)" "● open"
WORKSPACE_COUNT_BEFORE=$(workspace_count)
press Enter
wait_screen_absent "open-me — select branch"
wait_focused_checkout "$PLAIN_WORKTREE"
[ "$(workspace_count)" = "$WORKSPACE_COUNT_BEFORE" ] \
    || fail "reopening plain created a duplicate workspace"
printf 'plain branch reopen focuses existing workspace: ok\n'

open_branch master
wait_focused_checkout "$REPO"
printf 'main checkout branch focuses source workspace: ok\n'

open_branch_view
press Escape
wait_screen_contains "herdr-kiosk — select repo"
assert_screen_absent "open-me — select branch"
printf 'branch escape returns to repo picker: ok\n'

git -C "$REPO" remote add origin "$HK_ROOT/remote.git"
git -C "$REPO" remote add upstream "$HK_ROOT/remote.git"
press Tab
wait_screen_contains "open-me — select branch"
wait_screen_contains "upstream/remote-only"
wait_screen_contains "origin/remote-only"
wait_branches_settled
assert_screen_contains "upstream/remote-only"
capture
[ "$(grep -Fc "master (worktree)" "$LAST_SCREEN")" = 1 ] \
    || fail "local master branch row was not unique"
if grep -Eq '(origin|upstream)/master' "$LAST_SCREEN"; then
    fail "branch present locally and remotely was duplicated"
fi
printf 'same-named remote branch streaming and local dedup: ok\n'

type_text upstream/remote-only
wait_screen_contains "1 of 8 branches"
press Enter
wait_screen_absent "open-me — select branch"

REMOTE_WORKTREE="$HK_ROOT/worktrees/open-me/remote-only"
wait_path_exists "$REMOTE_WORKTREE"
wait_focused_checkout "$REMOTE_WORKTREE"
UPSTREAM=$(git -C "$REPO" rev-parse --abbrev-ref 'remote-only@{upstream}')
[ "$UPSTREAM" = "upstream/remote-only" ] \
    || fail "remote-only did not track upstream/remote-only: $UPSTREAM"
printf 'remote tracking checkout and focus: ok\n'

open_branch_view
type_text feat/new-branch
wait_screen_contains "0 of 7 branches"
press C-b
wait_screen_contains 'New branch "feat/new-branch" — pick base'
assert_screen_contains "open-me — select branch"
type_text featurx
wait_screen_contains "0 bases"
press BSpace
wait_screen_contains "1 bases"
assert_screen_contains "feature"
press Escape
wait_screen_contains "7 bases"
press Down
press C-n
press Down
press C-n
press C-p
press Up
press C-n
wait_screen_line_contains_all "feature" "▸"
press Enter
wait_screen_absent "open-me — select branch"

NEW_BRANCH_WORKTREE="$HK_ROOT/worktrees/open-me/feat-new-branch"
wait_path_exists "$NEW_BRANCH_WORKTREE"
wait_focused_checkout "$NEW_BRANCH_WORKTREE"
FEATURE_TIP=$(git -C "$REPO" rev-parse feature)
NEW_BRANCH_TIP=$(git -C "$REPO" rev-parse feat/new-branch)
MASTER_TIP=$(git -C "$REPO" rev-parse master)
[ "$NEW_BRANCH_TIP" = "$FEATURE_TIP" ] \
    || fail "feat/new-branch was not created from feature"
[ "$NEW_BRANCH_TIP" != "$MASTER_TIP" ] \
    || fail "feat/new-branch unexpectedly used master as its base"
git -C "$REPO" merge-base --is-ancestor feature feat/new-branch \
    || fail "feature was not an ancestor of feat/new-branch"
printf 'new branch base editing, arrow/Ctrl navigation, worktree, and focus: ok\n'

open_branch_view
type_text closed-checkout
wait_screen_contains "1 of 8 branches"
press C-x
wait_screen_contains "$HK_ROOT/existing-worktrees/closed-checkout"
press Enter
wait_path_absent "$HK_ROOT/existing-worktrees/closed-checkout"
assert_branch_exists "$REPO" closed-checkout
wait_screen_contains "open-me — select branch"
quit_picker "open-me — select branch"
printf 'closed checkout deletion preserves branch: ok\n'

# Deleting from a picker opened over another workspace leaves the picker up, back on
# the branch list.
DELETE_OPEN_WORKTREE="$HK_ROOT/worktrees/open-me/delete-open"
open_branch delete-open
wait_path_exists "$DELETE_OPEN_WORKTREE"
wait_focused_checkout "$DELETE_OPEN_WORKTREE"

focus_checkout "$REPO"
open_branch_view
type_text delete-open
wait_screen_line_contains_all "delete-open" "● open"
press C-x
wait_screen_contains "It will also be closed in Herdr."
press Enter
wait_path_absent "$DELETE_OPEN_WORKTREE"
wait_checkout_closed "$DELETE_OPEN_WORKTREE"
assert_branch_exists "$REPO" delete-open
wait_screen_contains "open-me — select branch"
wait_branches_settled
assert_screen_absent "delete-open (worktree)"
wait_focused_checkout "$REPO"
quit_picker "open-me — select branch"
printf 'open workspace deletion preserves branch and returns to the branch list: ok\n'

# herdr only removes checkouts of its worktree workspaces, so kiosk first has herdr take
# over a plain workspace showing the checkout. Taking it over opens the repository's
# main checkout in the background when no workspace shows it yet.
NESTED_REPO="$HK_ROOT/repos/deep/level-one/level-two/nested-repo"
ADOPTED_WORKTREE="$HK_ROOT/existing-worktrees/plain-open"
git -C "$NESTED_REPO" worktree add -q -b plain-open "$ADOPTED_WORKTREE"
ADOPTED_WORKSPACE_ID=$(h workspace create --cwd "$ADOPTED_WORKTREE" --no-focus \
    | /usr/bin/python3 -c 'import json, sys; print(json.load(sys.stdin)["result"]["workspace"]["workspace_id"])')
query_workspaces plain "$ADOPTED_WORKSPACE_ID" \
    || fail "herdr opened $ADOPTED_WORKTREE as a worktree workspace, so kiosk would not adopt it"
if query_workspaces checkout "$NESTED_REPO" >/dev/null; then
    fail "nested-repo's main checkout was already open"
fi
WORKSPACE_COUNT_BEFORE=$(workspace_count)
focus_checkout "$REPO"
show_branches nested-repo
type_text plain-open
wait_screen_line_contains_all "plain-open" "● open"
press C-x
wait_screen_contains "It will also be closed in Herdr."
press Enter
wait_path_absent "$ADOPTED_WORKTREE"
wait_for "plain workspace $ADOPTED_WORKSPACE_ID to close" \
    query_workspaces absent "$ADOPTED_WORKSPACE_ID"
assert_branch_exists "$NESTED_REPO" plain-open
wait_screen_contains "nested-repo — select branch"
wait_branches_settled
assert_screen_absent "plain-open (worktree)"
wait_for "nested-repo's main checkout to open in a workspace" \
    query_workspaces checkout "$NESTED_REPO"
if checkout_is_focused "$NESTED_REPO"; then
    fail "nested-repo's main checkout took focus from the picker's workspace"
fi
wait_focused_checkout "$REPO"
[ "$(workspace_count)" = "$WORKSPACE_COUNT_BEFORE" ] \
    || fail "expected the plain workspace to be replaced by one for the main checkout"
quit_picker "nested-repo — select branch"
printf 'checkout open in a plain workspace is adopted, deleted, and its branch kept: ok\n'

# Deleting the checkout of the workspace the picker was opened over closes the picker
# along with that workspace.
DIRTY_OPEN_WORKTREE="$HK_ROOT/worktrees/open-me/dirty-open"
open_branch dirty-open
wait_path_exists "$DIRTY_OPEN_WORKTREE"
wait_focused_checkout "$DIRTY_OPEN_WORKTREE"
printf 'untracked\n' >"$DIRTY_OPEN_WORKTREE/untracked.txt"

open_branch_view
type_text dirty-open
wait_screen_line_contains_all "dirty-open" "● open"
press C-x
wait_screen_contains "It will also be closed in Herdr."
press Enter
wait_screen_contains "This checkout has uncommitted changes."
[ -d "$DIRTY_OPEN_WORKTREE" ] || fail "dirty checkout disappeared before force confirmation"
press Enter
wait_path_absent "$DIRTY_OPEN_WORKTREE"
wait_checkout_closed "$DIRTY_OPEN_WORKTREE"
assert_branch_exists "$REPO" dirty-open
wait_screen_absent "open-me — select branch"
wait_focused_checkout "$REPO"

open_branch_view
type_text dirty-open
wait_screen_line_contains_all "▸" "dirty-open"
assert_screen_absent "dirty-open (worktree)"
quit_picker "open-me — select branch"
printf 'dirty herdr checkout force confirmation and deletion: ok\n'

open_picker
quit_picker "herdr-kiosk — select repo"
printf 'e2e: PASS\n'
