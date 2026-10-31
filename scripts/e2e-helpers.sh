#!/usr/bin/env bash

# Shared tmux harness helpers. Callers set HK_ROOT, HK_HOME_DIR, TMUX_SOCKET,
# SESSION, LAST_SCREEN, HERDR_BIN and TMUX_BIN before sourcing this file.

PLUGIN_ID=thomasschafer.herdr-kiosk
# How long the wait_* helpers poll before failing, in seconds.
WAIT_TIMEOUT=${HK_E2E_TIMEOUT:-15}

h() {
    env HOME="$HK_HOME_DIR" "$HERDR_BIN" "$@"
}

t() {
    "$TMUX_BIN" -f /dev/null -S "$TMUX_SOCKET" "$@"
}

capture() {
    t capture-pane -p -t "$SESSION" >"$LAST_SCREEN"
}

# Types text into the focused pane literally, so text such as "Enter" stays text.
type_text() {
    t send-keys -t "$SESSION" -l "$1"
}

press() {
    t send-keys -t "$SESSION" "$@"
}

print_diagnostics() {
    printf '%s\n' '--- captured screen ---' >&2
    if capture 2>/dev/null; then
        sed -n '1,160p' "$LAST_SCREEN" >&2
    else
        printf '%s\n' '(the tmux session is gone)' >&2
    fi
    printf '%s\n' '--- workspaces ---' >&2
    h workspace list 2>&1 | /usr/bin/python3 -c '
import json
import sys

raw = sys.stdin.read()
try:
    workspaces = json.loads(raw)["result"]["workspaces"]
except (ValueError, KeyError):
    sys.exit(raw)
for workspace in workspaces:
    print(
        workspace["workspace_id"],
        "focused" if workspace.get("focused") else "-",
        repr(workspace.get("label")),
        workspace.get("worktree", {}).get("checkout_path", "(plain workspace)"),
    )
' >&2 || true
    printf '%s\n' '--- panes ---' >&2
    h pane list 2>&1 | /usr/bin/python3 -c '
import json
import sys

raw = sys.stdin.read()
try:
    panes = json.loads(raw)["result"]["panes"]
except (ValueError, KeyError):
    sys.exit(raw)
for pane in panes:
    print(pane["pane_id"], pane["workspace_id"], pane.get("cwd"))
' >&2 || true
    printf '%s\n' '--- last plugin runs ---' >&2
    h plugin log list --plugin "$PLUGIN_ID" 2>&1 | /usr/bin/python3 -c '
import json
import sys

raw = sys.stdin.read()
try:
    logs = json.loads(raw)["result"]["logs"]
except (ValueError, KeyError):
    sys.exit(raw)
for log in logs[-5:]:
    print(log["action_id"], log["status"], "exit", log.get("exit_code"))
    for stream in ("stdout", "stderr"):
        if log.get(stream):
            print(f"  {stream}: {log[stream].strip()}")
' >&2 || true
    printf '%s\n' '--- herdr server log (tail) ---' >&2
    tail -n 30 "$HK_HOME_DIR/.config/herdr/herdr-server.log" >&2 2>/dev/null || true
}

fail() {
    printf 'e2e failure: %s\n' "$1" >&2
    print_diagnostics
    exit 1
}

cleanup() {
    h server stop >/dev/null 2>&1 || true
    t kill-server >/dev/null 2>&1 || true
}

# Writes the sandbox herdr config: checkouts go in $HK_ROOT/worktrees, and there is no
# first-run onboarding, whose dialogs make herdr refuse to open plugin popups.
write_herdr_config() {
    mkdir -p "$HK_HOME_DIR/.config/herdr" "$HK_ROOT/worktrees"
    cat >"$HK_HOME_DIR/.config/herdr/config.toml" <<EOF
onboarding = false

[worktrees]
directory = "$HK_ROOT/worktrees"
EOF
}

herdr_answers() {
    h workspace list >/dev/null 2>&1
}

# Starts the herdr client in tmux, with its first workspace in an empty sandbox folder,
# and waits until the server answers and the sidebar is drawn.
start_herdr() {
    local width=$1
    local height=$2
    mkdir -p "$HK_ROOT/start"
    t new-session -d -s "$SESSION" -x "$width" -y "$height" -c "$HK_ROOT/start" \
        "env HOME='$HK_HOME_DIR' '$HERDR_BIN'"
    wait_for "herdr to answer" herdr_answers
    wait_screen_contains "spaces"
}

# Runs `command...` until it succeeds, failing with `description` once $WAIT_TIMEOUT
# seconds have passed.
wait_for() {
    local description=$1
    shift
    local deadline=$((SECONDS + WAIT_TIMEOUT))
    until "$@"; do
        if ((SECONDS >= deadline)); then
            fail "timed out after ${WAIT_TIMEOUT}s waiting for $description"
        fi
        sleep 0.1
    done
}

screen_contains() {
    capture && grep -Fq -- "$1" "$LAST_SCREEN"
}

screen_lacks() {
    capture && ! grep -Fq -- "$1" "$LAST_SCREEN"
}

# Whether a screen line containing the first argument also contains all the others.
screen_line_contains_all() {
    local anchor=$1
    shift
    capture || return 1
    local line pattern
    while IFS= read -r line; do
        for pattern in "$@"; do
            [[ $line == *"$pattern"* ]] || continue 2
        done
        return 0
    done < <(grep -F -- "$anchor" "$LAST_SCREEN")
    return 1
}

wait_screen_contains() {
    wait_for "the screen to show: $1" screen_contains "$1"
}

wait_screen_absent() {
    wait_for "the screen to stop showing: $1" screen_lacks "$1"
}

wait_screen_line_contains_all() {
    wait_for "a screen line with all of: $*" screen_line_contains_all "$@"
}

assert_screen_contains() {
    screen_contains "$1" || fail "screen did not contain: $1"
}

assert_screen_absent() {
    screen_lacks "$1" || fail "screen unexpectedly contained: $1"
}

assert_screen_line_contains_all() {
    screen_line_contains_all "$@" || fail "no screen line contained all of: $*"
}

# Queries `herdr plugin log list` for this plugin, by mode:
#   last-id      prints the number of the latest run, or 0 when there is none
#   after N      prints the outcome of the first run after run N, and fails while there
#                is no such run or it is still going
plugin_runs() {
    h plugin log list --plugin "$PLUGIN_ID" | /usr/bin/python3 -c '
import json
import sys

mode, *args = sys.argv[1:]
runs = {
    int(log["log_id"].rsplit("-", 1)[1]): log
    for log in json.load(sys.stdin)["result"]["logs"]
}

if mode == "last-id":
    print(max(runs, default=0))
elif mode == "after":
    later = sorted(number for number in runs if number > int(args[0]))
    if not later or runs[later[0]]["status"] == "running":
        sys.exit(1)
    status = runs[later[0]]["status"]
    stderr = runs[later[0]].get("stderr", "").strip()
    print(status if status == "succeeded" else f"{status}: {stderr}")
else:
    sys.exit(f"unknown plugin_runs mode: {mode}")
' "$@"
}

# Runs the open-picker action and waits for its shim to finish. A picker that has just
# done its work can take a moment to exit after it stops drawing, and herdr refuses a
# second popup until it has, so that refusal is retried.
invoke_picker() {
    local deadline=$((SECONDS + WAIT_TIMEOUT))
    local last_run outcome
    while true; do
        last_run=$(plugin_runs last-id)
        h plugin action invoke open-picker --plugin "$PLUGIN_ID" >/dev/null
        wait_for "the open-picker action to finish" plugin_runs after "$last_run"
        outcome=$(plugin_runs after "$last_run")
        case "$outcome" in
            succeeded) return 0 ;;
            *"a popup pane is already open"*) ;;
            *) fail "the open-picker action failed: $outcome" ;;
        esac
        if ((SECONDS >= deadline)); then
            fail "the previous popup was still open after ${WAIT_TIMEOUT}s"
        fi
        sleep 0.1
    done
}

# Queries `herdr workspace list`, by mode:
#   count                  prints the number of workspaces
#   checkout PATH          prints the herdr worktree workspaces showing PATH
#   focused-checkout PATH  the same, but only a focused one
#   focused ID             succeeds when workspace ID is focused
#   plain ID               succeeds when workspace ID exists and is not a herdr worktree
#   absent ID              succeeds when there is no workspace ID
# The modes that look something up fail when nothing matches.
query_workspaces() {
    h workspace list | /usr/bin/python3 -c '
import json
import os
import sys

mode, *args = sys.argv[1:]
workspaces = json.load(sys.stdin)["result"]["workspaces"]
by_id = {workspace["workspace_id"]: workspace for workspace in workspaces}

if mode == "count":
    print(len(workspaces))
elif mode in ("checkout", "focused-checkout"):
    path = os.path.realpath(args[0])
    found = [
        workspace["workspace_id"]
        for workspace in workspaces
        if "worktree" in workspace
        and os.path.realpath(workspace["worktree"]["checkout_path"]) == path
        and (mode == "checkout" or workspace.get("focused"))
    ]
    if not found:
        sys.exit(1)
    print("\n".join(found))
elif mode == "focused":
    sys.exit(0 if by_id.get(args[0], {}).get("focused") else 1)
elif mode == "plain":
    sys.exit(0 if args[0] in by_id and "worktree" not in by_id[args[0]] else 1)
elif mode == "absent":
    sys.exit(0 if args[0] not in by_id else 1)
else:
    sys.exit(f"unknown query_workspaces mode: {mode}")
' "$@"
}

workspace_count() {
    query_workspaces count
}

workspace_count_is() {
    [ "$(workspace_count)" = "$1" ]
}

checkout_is_focused() {
    query_workspaces focused-checkout "$1" >/dev/null
}

checkout_has_no_workspace() {
    ! query_workspaces checkout "$1" >/dev/null
}

wait_focused_checkout() {
    wait_for "a focused herdr worktree workspace for $1" checkout_is_focused "$1"
}

wait_checkout_closed() {
    wait_for "no herdr worktree workspace to show $1" checkout_has_no_workspace "$1"
}

wait_focused_workspace() {
    wait_for "workspace $1 to be focused" query_workspaces focused "$1"
}

wait_workspace_count() {
    wait_for "$1 workspaces" workspace_count_is "$1"
}

# Prints the id of the workspace with a pane whose cwd is the given folder.
workspace_id_for_cwd() {
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
' "$1"
}

pane_cwd_known() {
    workspace_id_for_cwd "$1" >/dev/null
}

wait_pane_cwd() {
    wait_for "a pane in $1" pane_cwd_known "$1"
}

wait_path_absent() {
    wait_for "$1 to be removed" test ! -e "$1"
}

wait_path_exists() {
    wait_for "$1 to exist" test -e "$1"
}

assert_branch_exists() {
    git -C "$1" show-ref --verify --quiet "refs/heads/$2" \
        || fail "branch no longer existed: $2"
}

make_repo() {
    local path=$1
    mkdir -p "$path"
    git -C "$path" init -q -b master
    printf 'fixture\n' >"$path/README.md"
    git -C "$path" add README.md
    git -C "$path" -c user.name=E2E -c user.email=e2e@example.invalid commit -qm initial
}
