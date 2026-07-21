# Verifying herdr-kiosk end to end

How to drive a real herdr — popup included — programmatically. Validated during M0
against herdr 0.7.4 built from source. Both the orchestrator and Codex run this same
loop before reporting work as done.

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
printf '[worktrees]\ndirectory = "%s"\n' "$SANDBOX/worktrees" \
  > "$SANDBOX/home/.config/herdr/config.toml"

h plugin link /path/to/plugin                  # works with no server running
t new-session -d -s e2e -x 200 -y 50 "env HOME=$SANDBOX/home $HERDR"
sleep 3                                        # wait for herdr UI

h plugin action invoke <plugin-id>.<action>    # opens the popup
t send-keys -t e2e <keys>                      # drive the picker
t capture-pane -p -t e2e                       # read the screen, popup included

h workspace list                               # assert outcomes (JSON)
h worktree list --cwd <repo> --json            # open_workspace_id per checkout
h plugin log list --plugin <plugin-id>         # shim/action stdout+stderr+exit codes

h server stop 2>/dev/null; t kill-server       # teardown (killing tmux also kills a
                                               # server that was auto-started by the client)
```

## What M0 validated with this harness

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

7. `ui_busy` observed in the wild during M1 verification: a fresh sandbox HOME makes
   herdr show its first-run welcome dialog, and invoking the picker action while any
   such modal is up fails with `ui_busy: "popup panes can only open from the normal
   workspace view"` — captured in `plugin log list` like every other shim failure.
   Harness note: dismiss first-run onboarding (Enter through welcome, Escape out of
   Settings) before invoking popup actions, or reuse a sandbox HOME that has already
   completed onboarding.
