# Verifying herdr-kiosk end to end

How to drive a real herdr — popup included — programmatically. The harness has been
validated against herdr 0.7.4 built from source and is the end-to-end loop used before
reporting runtime work as done.

## The pattern

Run the herdr client inside a dedicated tmux server. From tmux's perspective herdr is
just a TUI app, so `tmux send-keys` drives it and `tmux capture-pane -p` reads the real
screen — including session-modal popups, which herdr's own pane APIs cannot reach
(a popup is deliberately not a pane: no pane id, no `pane send-keys`, no
`terminal session observe`).

Isolation comes from a sandbox `$HOME`: herdr derives config, session/plugins registry,
logs, and the API socket from `$HOME`/XDG, so every herdr invocation (server, CLI,
`plugin link`) exported with the sandbox HOME is fully isolated from the developer's
real herdr.

Constraints discovered:

- The sandbox HOME must be a short path (e.g. `/tmp/hk-e2e`). The API socket lives at
  `$HOME/.config/herdr/herdr.sock` and macOS caps `sun_path` at ~104 bytes; long
  temp paths fail with "local socket name length exceeds capacity of sun_path".
  Fixtures and the worktrees directory can live anywhere (no socket constraint).
- `herdr workspace list` prints JSON by default and rejects `--json`; `worktree list`
  takes `--json`.
- Building herdr from source requires its nix dev shell (`nix develop --command cargo
  build --release` in the herdr checkout): build.rs invokes zig 0.15 for vendored
  libghostty-vt, and a system zig 0.16 fails.

## Skeleton

```sh
SANDBOX=/tmp/hk-e2e            # short path: socket length limit
HERDR=/path/to/herdr           # 0.7.4+ binary
h() { env HOME="$SANDBOX/home" "$HERDR" "$@"; }
t() { tmux -S "$SANDBOX/tmux.sock" "$@"; }

mkdir -p "$SANDBOX/home/.config/herdr" "$SANDBOX/worktrees"
printf 'onboarding = false\n\n[worktrees]\ndirectory = "%s"\n' "$SANDBOX/worktrees" \
  > "$SANDBOX/home/.config/herdr/config.toml"

h plugin link /path/to/plugin                  # works with no server running
t new-session -d -s e2e -x 200 -y 50 "env HOME=$SANDBOX/home $HERDR"
until h workspace list >/dev/null 2>&1; do sleep 0.1; done   # wait for the server

h plugin action invoke <plugin-id>.<action>    # opens the popup
t send-keys -t e2e <keys>                      # drive the picker
t capture-pane -p -t e2e                       # read the screen, popup included

h workspace list                               # assert outcomes (JSON)
h worktree list --cwd <repo> --json            # open_workspace_id per checkout
h plugin log list --plugin <plugin-id>         # shim/action stdout+stderr+exit codes

h server stop 2>/dev/null; t kill-server       # teardown (killing tmux also kills a
                                               # server that was auto-started by the client)
```

## What this harness validates

1. Popup opens via action invoke; env injection is as documented
   (`HERDR_BIN_PATH`, `HERDR_PLUGIN_CONTEXT_JSON` with `workspace_cwd` /
   `focused_pane_cwd`, config/state dirs; no `HERDR_PANE_ID`).
2. All input reaches the popup process, including Escape (byte 27) — the plugin TUI
   owns Escape handling.
3. The fire-and-exit pattern works: calling
   `worktree open --cwd <repo> --path <repo> --focus` from inside the popup creates
   and focuses the workspace while the popup is still up; when the popup process
   exits, the popup tears down and the user lands on the newly focused workspace.
4. `worktree create --branch <new> --focus` from the popup: creates the checkout under
   the configured `worktrees.directory`, opens a workspace grouped under the parent
   repo in the sidebar, focuses it.
5. Re-opening an already-open checkout focuses the existing workspace
   (`already_open: true`), no duplicate.
6. Invoking the pane-open action while the popup is already up fails cleanly with
   `plugin_pane_open_failed: "popup already open"`, visible (with stderr and exit
   code) in `plugin log list` — which is the debugging channel for shim failures
   generally, `ui_busy` included.

7. A fresh sandbox HOME makes herdr show its first-run welcome dialog. Invoking the
   picker action while that or another modal is open fails with `ui_busy: "popup panes
   can only open from the normal workspace view"` — captured in `plugin log list` like
   every other shim failure. Set `onboarding = false` in the sandbox's herdr
   `config.toml` to skip onboarding, or dismiss it (Enter through welcome, Escape out
   of Settings) before invoking popup actions.

## The Herdr app

The app is driven entirely by its own CLI, so no tmux is needed. Isolate it with a
scratch app home and a separate herdr-core session, and give both a sandbox `HOME` so
checkouts, kiosk's config and state, and the core's state stay out of your own:

```sh
SANDBOX=/tmp/kt                                 # short path: socket length limit
export HOME=$SANDBOX/home HERDR_APP_HOME=$SANDBOX/apphome HC_SESSION=kiosktest
export XDG_STATE_HOME=$SANDBOX/xdg/state XDG_CONFIG_HOME=$SANDBOX/xdg/config
export XDG_RUNTIME_DIR=$SANDBOX/xdg/run
export PATH=/path/to/herdr-app-and-hc:$PATH     # the app finds hc on PATH
for v in $(env | sed -n 's/^\(HERDR_[A-Z_]*\)=.*/\1/p'); do
  [ "$v" = HERDR_APP_HOME ] || unset "$v"       # a herdr TUI pane sets these
done

herdr-app &                                     # a second window, on this home
ln -s "$PWD/herdr-app/kiosk" "$HERDR_APP_HOME/mods/kiosk"
herdr-app check "$HERDR_APP_HOME/mods/kiosk"
herdr-app do mod.open:kiosk                     # opens the picker
hc ls --json                                    # kiosk's terminal has no "where"
hc keys <terminal> down tab enter               # drive the picker
hc read <terminal>                              # read its screen
herdr-app ls; herdr-app status                  # assert spaces, panes and overlays
herdr-app shot shot.png                         # look at the window

herdr-app do app.quit; hc server stop           # teardown
```

Use `hc keys` to drive kiosk: `hc send` pastes its text and then presses Enter.

On macOS the window stops drawing while another window covers it, so `shot` can
return a stale frame. Toggling `herdr-app do window.maximize` twice before a shot
makes it draw a fresh one.

What to check: keys typed right after `mod.open:kiosk` reach kiosk without a click;
opening a repository makes a space in it with the `[on_open]` panes and focus on the
first pane, and quitting an `[on_open]` command leaves its shell; opening it again selects that space without new panes; a
branch without a checkout gets one in `~/.herdr/worktrees/<repo>/<branch>`, nested
under its repository in the sidebar; deleting an open, dirty worktree asks to force,
then removes the checkout and closes its space; `[on_open]` failures show as a
notification; a failing kiosk (an invalid config) stays on screen until Enter; and a
missing binary shows a notification.
