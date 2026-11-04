#!/usr/bin/env bash
# The kiosk mod end to end in the app sandbox: real key presses and clicks on the released
# Herdr app, checked against what kiosk showed (its message trace) and what the app did
# (`herdr-app ls`, the checkouts on disk). Run through `run.sh e2e`.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
sb() { "$here/sandbox" "$@"; }
trace=$HOME/sb/trace.jsonl
shots=${E2E_SHOTS:-/src/target/app-e2e}
mkdir -p "$shots"

step=0
fail() {
  sb shot "$shots/failure.png" >/dev/null 2>&1 || true
  echo "FAIL ($step): $*" >&2
  echo "last view: $(last_view | head -c 2000)" >&2
  exit 1
}
check() {
  step=$((step + 1))
  echo "- $1"
}

last_view() { grep '^< {"t":"view"' "$trace" 2>/dev/null | tail -1 | cut -c3- | jq -c .view; }

# Waits until the latest view satisfies the jq filter `$1`.
wait_view() {
  for _ in $(seq 100); do
    if [[ $(last_view | jq -r "$1" 2>/dev/null) == true ]]; then return; fi
    sleep 0.1
  done
  fail "the view never satisfied $1"
}

open_overlays() { sb status | jq -r '.overlays | join(" ")'; }

wait_closed() {
  for _ in $(seq 100); do
    [[ " $(open_overlays) " != *" kiosk "* ]] && return
    sleep 0.1
  done
  fail "the popup stayed open"
}

open_kiosk() {
  sb open
  wait_view '.screen != null'
}

rows='[.main.rows[].name.text]'
space_folders() { sb ls | jq -r '.spaces[].folder'; }

sb down >/dev/null 2>&1 || true
EXTRA_REPOS=16 sb fixtures
sb up
sb install >/dev/null

check "the picker lists the repositories"
open_kiosk
wait_view '.main.state == "list" and (.main.total == 23)'
[[ $(last_view | jq -r "$rows | .[0:3] | join(\",\")") == "api,cli,design-system" ]] || fail "unexpected order"
sb shot "$shots/repos.png" >/dev/null

check "typing filters, alt+backspace (a picker chord) clears the word"
sb type infr
wait_view ".main.query.before == \"infr\" and ($rows == [\"infra\"])"
sb key alt+BackSpace
wait_view '.main.query.before == "" and .main.total == 23'

check "the wheel scrolls the list and the selection stays on screen"
sb move 600 300
sb wheel down 6
wait_view '.main.offset > 0 and .main.selected != null'

check "help opens over the picker and Esc closes only the help"
sb key ctrl+h
wait_view '.overlay.kind == "help"'
sb shot "$shots/help.png" >/dev/null
sb key Escape
wait_view '.overlay == null'
[[ " $(open_overlays) " == *" kiosk "* ]] || fail "Esc closed the popup"

check "tab browses a repository's branches, the back button returns"
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
wait_view '.main.kind == "branches" and .main.title == "api — select branch"'
wait_view '[.main.rows[] | select(.name.text == "feat/auth") | .actions.delete] == [true]'
wait_view '[.main.rows[] | select(.name.text == "main") | .actions.delete] == [false]'
sb shot "$shots/branches.png" >/dev/null
sb click 291 71
wait_view '.main.kind == "repos" and .main.query.before == "api"'
sb key Escape
wait_view '.main.query.before == ""'

check "a new branch gets a checkout and a space"
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
wait_view '.main.kind == "branches"'
sb type feat/e2e
wait_view '.main.rows == [] and (.main.empty | test("creates feat/e2e"))'
sb key Return
wait_view '.dialog.kind == "base_branch"'
sb shot "$shots/new-branch.png" >/dev/null

check "a click beside a dialog, on the dimmed list, only closes the dialog"
sb click 300 160
wait_view '.dialog == null and .main.kind == "branches"'
[[ ! -d $HOME/.herdr/worktrees/api/feat-e2e ]] || fail "the click behind the dialog created the branch"
sb key Return
wait_view '.dialog.kind == "base_branch"'
sb key Return
wait_closed
[[ -d $HOME/.herdr/worktrees/api/feat-e2e ]] || fail "no checkout for feat/e2e"
space_folders | grep -qx "$HOME/.herdr/worktrees/api/feat-e2e" || fail "no space for feat/e2e"

check "the new space shows as open, and removing its checkout asks first"
open_kiosk
wait_view '[.main.rows[] | select(.name.text == "api") | .open] == [true]'
sb shot "$shots/open.png" >/dev/null
sb type api
wait_view "$rows == [\"api\"]"
sb key Tab
sb type e2e
wait_view "$rows == [\"feat/e2e\"]"
sb key ctrl+x
wait_view '.dialog.kind == "confirm_delete" and (.dialog.lines[0] | test("feat-e2e"))'
sb shot "$shots/remove.png" >/dev/null
sb key Return
wait_view '.dialog == null'
for _ in $(seq 50); do [[ -d $HOME/.herdr/worktrees/api/feat-e2e ]] || break; sleep 0.1; done
[[ ! -d $HOME/.herdr/worktrees/api/feat-e2e ]] || fail "the checkout is still there"

check "a branch only the remote has opens as a tracking checkout"
sb key Escape Escape
wait_view '.main.kind == "repos" and .main.query.before == "api"'
sb key Escape
wait_view '.main.query.before == ""'
sb type web
wait_view "$rows == [\"web\"]"
sb key Tab
wait_view '[.main.rows[] | select(.name.text == "feat/from-review") | .badges] == [[{"kind": "remote", "name": "origin"}]]'
sb type from-review
wait_view "$rows == [\"feat/from-review\"]"
sb key Return
wait_closed
git -C "$HOME/code/web" rev-parse --verify -q feat/from-review >/dev/null || fail "no local tracking branch"

check "clicking a row opens that repository"
open_kiosk
wait_view '.main.total == 23'
sb click 400 187
wait_closed
space_folders | grep -qx "$HOME/code/cli" || fail "no space for cli"

check "Esc with an empty query quits, and the picker exits with the popup"
open_kiosk
wait_view '.main.state == "list"'
sb key Escape
wait_closed
for _ in $(seq 50); do pgrep -x herdr-kiosk >/dev/null || break; sleep 0.1; done
! pgrep -x herdr-kiosk >/dev/null || fail "a picker is still running"

check "closing the popup from outside ends the picker too"
open_kiosk
wait_view '.main.state == "list"'
sb "do" overlay.close >/dev/null
wait_closed
for _ in $(seq 50); do pgrep -x herdr-kiosk >/dev/null || break; sleep 0.1; done
! pgrep -x herdr-kiosk >/dev/null || fail "a picker outlived its popup"

check "without a config, the setup wizard writes one and the picker starts"
rm "$HOME/.config/herdr-kiosk/config.toml"
open_kiosk
wait_view '.screen == "setup" and .step == "welcome"'
sb shot "$shots/setup.png" >/dev/null
sb key Return
wait_view '.step == "directories"'
# The wizard is given the path as a person types it.
# shellcheck disable=SC2088
sb type '~/code'
wait_view '.path.query.before == "~/code"'
sb shot "$shots/setup-directories.png" >/dev/null
sb key Return
wait_view '.depth.query.before == "1"'
sb key Return
wait_view '.step == "folder_mode"'
sb key Down
wait_view '.options[1].selected == true'
sb shot "$shots/setup-folders.png" >/dev/null
sb key Return
wait_view '.step == "directories" and (.added | length) == 1'
sb key Return
wait_view '.step == "confirm"'
sb shot "$shots/setup-confirm.png" >/dev/null
sb key Return
wait_view '.screen == "picker" and .main.state == "list" and .main.total > 0'
grep -q 'include_non_git = true' "$HOME/.config/herdr-kiosk/config.toml" || fail "the config does not include plain folders"
sb key ctrl+c
wait_closed

echo "all $step checks passed; screenshots in $shots"
