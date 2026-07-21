# Herdr plugin: fuzzy repo/branch finder ported from kiosk

Research handoff. This document captures external research (herdr source, herdr docs,
official plugin examples, and the three most mature Rust plugins in the ecosystem) so
that implementation work doesn't have to rediscover it. Implementation details are
deliberately left open — this is the ground truth to build on, not a spec.

Research date: 21 July 2026. Herdr plugin API v1, `min_herdr_version = "0.7.0"`.

---

## 1. Goal

Build a **new** herdr plugin (new repo, not a fork) that reproduces the core of
[`thomasschafer/kiosk`](https://github.com/thomasschafer/kiosk):

1. **Repo picker.** Fuzzy search across git repos discovered under a configured set of
   directories. Discovery is async and streams — results populate as they're found, so the
   picker is usable immediately. This snappiness is the whole point of the tool; it is a
   hard requirement, not a nice-to-have.
2. **Enter on a repo** → open it as a herdr workspace (focus the existing workspace if the
   repo is already open).
3. **Tab on a repo** → branch picker for that repo. Also async: local branches first,
   remote branches streamed in afterwards and rendered greyed out but still selectable.
4. **Enter on a branch** → open a herdr workspace on a worktree for that branch, creating
   the worktree (and the branch) if it doesn't exist.

### Explicitly out of scope

- **Agent detection.** All of it. (~4,350 lines in kiosk.) Herdr does this natively and far
  better; the plugin must not duplicate or compete with it.
- **Recent sessions picker.** Kiosk's `--sessions` view. Not wanted.
- **Layout bootstrapping.** See §7 — this belongs to other plugins in the ecosystem.

### Decisions already made

| Question | Decision |
|---|---|
| Fork kiosk or new repo? | **New repo**, copy over only what's needed. |
| Repo already open as a workspace? | **Focus it.** Kiosk is the reference implementation. |
| Remote branch behaviour? | **Create a new local tracking branch from the remote and make a worktree.** Never change an existing checkout's branch — a different branch always means a new worktree. Kiosk is the reference implementation. |
| Config format? | **Build from scratch, idiomatically for herdr.** Do not reuse kiosk's config file or its path. See §6. |

---

## 2. How herdr plugins work (the parts that matter here)

A plugin is a directory with a `herdr-plugin.toml` manifest plus commands herdr can launch
via argv. There is no SDK. From the docs:

> There is no separate plugin SDK or restricted command set. The entire Herdr CLI is the
> plugin API [...] Most plugins should call Herdr through `HERDR_BIN_PATH`, which points at
> the running Herdr binary.

Commands are argv arrays; **herdr does not run them through a shell**, so there is no shell
expansion unless the command itself starts a shell.

Runtime commands receive: `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ENV=1`,
`HERDR_PLUGIN_ID`, `HERDR_PLUGIN_ROOT`, `HERDR_PLUGIN_CONFIG_DIR`, `HERDR_PLUGIN_STATE_DIR`,
`HERDR_PLUGIN_CONTEXT_JSON`, and where available `HERDR_WORKSPACE_ID`, `HERDR_TAB_ID`,
`HERDR_PANE_ID`. Actions additionally get `HERDR_PLUGIN_ACTION_ID`; pane entrypoints get
`HERDR_PLUGIN_ENTRYPOINT_ID`.

Manifest surface: `[[build]]`, `[[actions]]`, `[[events]]`, `[[panes]]`, `[[link_handlers]]`.
All are **manifest-only** — runtime action registration and runtime argv pane creation are
not part of v1.

### Popups are the right surface for the picker

Kiosk today runs as a tmux popup:

```
bind-key f popup -xC -yC -w90% -h90% -E "kiosk"
```

Herdr's `placement = "popup"` is an almost exact analogue:

> `placement = "popup"` opens a session-modal terminal popup without changing the tiled
> layout. It accepts optional `width` and `height` fields [...] use numbers for outer
> terminal-cell dimensions, or use strings like `"80%"` for a percentage of the terminal
> area. It receives all terminal input, including Escape, and closes when the command exits
> or a `popup.close` request is sent.

Consequences to design around:

- A popup **is not a herdr pane**. No pane ID, no `HERDR_PANE_ID` exported to its process,
  outside all `pane.*` and agent APIs, emits no pane lifecycle events, and does not change
  plugin focus context.
- `plugin.pane.open` returns `ui_busy` if Settings, Copy mode, or another herdr modal is
  active. Handle it.
- Dimensions below the popup minimum are clamped.

Neither of the first two is a problem for this plugin — it only needs to shell out to the
herdr CLI — but see §8 for the focus-lifecycle question.

---

## 3. Herdr's worktree API — verified against source

This is the single most important section. It was verified by reading
`src/app/api/worktrees.rs` and `src/worktree.rs` in `ogulcancelik/herdr`, not just the docs.

### `worktree create --cwd` works on a repo with no open workspace

This was an open question and the answer is favourable. `resolve_worktree_source` builds a
source from the path's git metadata, then `ensure_source_parent_membership` auto-creates the
parent workspace (unfocused) when none exists:

```rust
// src/app/api/worktrees.rs
fn ensure_source_parent_membership(
    &mut self,
    source: &mut WorktreeSource,
    emit_created_event: bool,
) -> Result<bool, ApiFailure> {
    if source.workspace_idx.is_none() {
        source.workspace_idx = self.find_parent_workspace_by_key(&source.repo_key);
    }
    let mut created_parent = false;
    if source.workspace_idx.is_none() {
        let ws_idx = self
            .create_workspace_with_options(source.source_checkout_path.clone(), false)
            .map_err(|err| ApiFailure::new("worktree_open_failed", err.to_string()))?;
        source.workspace_idx = Some(ws_idx);
        created_parent = true;
    }
    // ...
}
```

So Enter-on-branch is a single herdr call even when the repo has never been opened.

### Hard constraint: never pass a linked worktree path

```rust
if space.is_linked_worktree {
    return Err(ApiFailure::new(
        "linked_worktree_source",
        "New and open worktree actions start from the repo parent workspace.",
    ));
}
```

Always resolve to the repo root before calling. Kiosk already dedupes linked worktrees
during scanning — there is a test `test_scan_repos_streaming_deduplicates_linked_worktree_paths`
in `kiosk-core/src/git/cli.rs`. **Preserve that behaviour**; it is load-bearing here in a way
it wasn't in kiosk.

### The exact git commands herdr runs

From `src/worktree.rs`:

```rust
// New branch:
["-C", repo_root, "worktree", "add", "-b", branch, path, base]

// Existing local branch:
["-C", repo_root, "worktree", "add", path, branch]
```

Selection logic is `local_branch_exists(repo_root, branch)`. This gives create-or-checkout
semantics for free — matching kiosk's behaviour without reimplementing it.

**There is no `--track` equivalent exposed.** See §5.

### Checkout paths are herdr's business

> Without `--path`, Herdr creates the checkout under
> `<worktrees.directory>/<repo>/<branch-slug>`.

Delete kiosk's `.kiosk_worktrees/` convention entirely and let herdr own paths. Slug logic
lives in `branch_to_path_slug` / `default_checkout_path` in `src/worktree.rs` if you need to
predict a path, but prefer reading it back from the API response.

### Relevant CLI surface

```
herdr workspace list [--json]
herdr workspace create [--cwd PATH] [--label TEXT] [--focus] [--no-focus]
herdr workspace focus <workspace_id>

herdr worktree list   [--workspace ID | --cwd PATH] [--json]
herdr worktree create [--workspace ID | --cwd PATH] [--branch NAME] [--base REF]
                      [--path PATH] [--label TEXT] [--focus] [--no-focus] [--json]
herdr worktree open   [--workspace ID | --cwd PATH] (--path PATH | --branch NAME)
                      [--focus] [--no-focus] [--json]
```

Use at most one of `--workspace` / `--cwd`. Raw socket `cwd`/`path` values must be absolute;
the CLI expands relative ones. `worktree.open` "opens an existing checkout or returns the
already-open workspace" — server-side idempotency, which is strictly better than kiosk's
inferring-from-session-names approach.

Prefer `HERDR_BIN_PATH` + CLI over the raw socket: it's portable across Unix sockets and
Windows named pipes, and the docs steer plugins that way.

---

## 4. Mapping kiosk → herdr

Kiosk's model is: derive a deterministic tmux session name, check existence, attach or
create. Herdr owns workspaces and worktree provenance itself, so that responsibility is
handed over rather than reimplemented.

| Kiosk behaviour | Herdr equivalent |
|---|---|
| Enter on repo → attach/create `<repo>` session | `workspace list --json`, match on cwd → `workspace focus <id>`; else `workspace create --cwd <repo> --label <name> --focus` |
| Enter on branch → create worktree, then session | `worktree create --cwd <repo> --branch <name> --focus` |
| Attach if worktree session exists | `worktree open --cwd <repo> --branch <name> --focus` |
| `apply_repo_name_collision_resolution` | **delete** — no session namespace to disambiguate |
| `tmux_session_name_for_worktree` | **delete** |
| `normalize_session_base`, `repo_matches_active_session` | **delete** |
| `.kiosk_worktrees/` path logic | **delete** — herdr's `worktrees.directory` |

Kiosk already has a `GitProvider` / `TmuxProvider` trait split with mocks
(`kiosk-core/src/git/`, `kiosk-core/src/tmux/`). The natural shape is a `HerdrProvider`
trait in the same form, replacing `TmuxProvider`. The seam is already cut.

The provider is thinner than it first appears, because branch-exists-or-create and
parent-workspace-creation are both handled server-side. Realistically:
`workspace list` + cwd match, `workspace focus`, `workspace create`, `worktree create`,
`worktree open`, plus the `git branch --track` pre-step from §5.

### What to copy from kiosk (~3k lines)

- `kiosk-core/src/git/` — `scan_repos_streaming`, `list_branches`,
  `list_remote_branches_for_remote`, `list_remotes`, `list_worktrees`,
  `parse_worktree_porcelain`. Pure git, zero tmux/herdr awareness.
- `kiosk-core/src/event.rs` — the `AppEvent` enum and the mpsc-into-the-event-loop pattern.
  This is what produces the streaming feel. Keep `ReposFound`, `RepoEnriched`,
  `ScanComplete`, `BranchesLoaded`, `RemoteBranchesLoaded`, `GitFetchCompleted`,
  `WorktreeCreated`, `WorktreeRemoved`, `GitError`. Drop the session/agent variants
  (`SessionRuntimeUpdate`, `AgentStatesUpdated`, `SessionsDiscovered`,
  `SessionMetadataPatched`, `SessionAgentStatesPatched`, `SessionActivityLoaded`).
- From `kiosk-tui/src/app/spawn.rs`: `spawn_repo_discovery`, `spawn_branch_loading`,
  `spawn_remote_branch_loading`, `spawn_git_fetch`, and the bounded-rayon-pool helper
  `spawn_work_parallel`. Note `ENRICHMENT_POOL_SIZE = 8` and `FETCH_POOL_SIZE = 4` — bounded
  pools exist to avoid thread explosion with hundreds of repos.
- TUI components: `repo_list`, `branch_picker`, `search_bar`, `list_row`, `new_branch`,
  `dialog`, `error_toast`, `path_input`, `help`, `theme`.
- `kiosk-core/src/keyboard.rs` and the keymap/config-key machinery, slimmed.

Stack, for reference: `ratatui` 0.30, `crossterm` 0.29, `fuzzy-matcher` 0.3, `rayon` 1,
`unicode-width`, `unicode-segmentation`, `serde` + `toml`.

### What to delete (~5.5k lines)

- `kiosk-core/src/agent/` — `mod.rs` (1,682) + `detect.rs` (2,668).
- `kiosk-core/src/tmux/` — `cli.rs` (778), `mock.rs` (243), `provider.rs` (100), `mod.rs`.
- `kiosk-tui/src/components/sessions_view.rs`.
- Session/agent fields threaded through `state.rs`.

`BranchEntry` loses `has_session`, `session_activity_ts`, `agent_statuses`. It keeps `name`,
`worktree_path`, `is_current`, `is_default`, `remote`.

---

## 5. Remote branches and tracking

Kiosk is explicit about tracking:

```rust
// kiosk-core/src/git/cli.rs — create_tracking_branch_and_worktree
["worktree", "add", &worktree_path, "-b", branch, "--track", &format!("origin/{branch}")]
```

Herdr exposes no `--track`. Git's default `branch.autoSetupMerge` *would* set upstream when
the base is a remote-tracking ref, but that is user config and can't be relied on. To match
kiosk exactly, use two steps:

```
git -C <repo> branch --track <branch> <remote>/<branch>
herdr worktree create --cwd <repo> --branch <branch> --focus
```

The second call finds an existing local branch and runs `git worktree add <path> <branch>`.

**Bug to fix while porting:** kiosk hardcodes `origin/` in
`create_tracking_branch_and_worktree` despite `BranchEntry` carrying `remote: Option<String>`.
Use the field.

Everything else about remote branches is pure git and carries over untouched: the
`git branch -r --format=%(refname:short) --list <remote>/*` listing, the `->` HEAD-pointer
filter, dedup against local names, and the greyed-out rendering (`branch.remote` is already
`Option<String>` and rendered as a ` (remote)` suffix in `branch_picker.rs`).

---

## 6. Config — the established idiom

Both mature Rust plugins converged on the same pattern: `$HERDR_PLUGIN_CONFIG_DIR/config.toml`
with an XDG fallback for standalone use. From `smarzban/herdr-file-viewer/src/config.rs`:

```rust
pub fn config_path(get: impl Fn(&str) -> Option<String>) -> std::path::PathBuf {
    if let Some(dir) = get("HERDR_PLUGIN_CONFIG_DIR").filter(|s| !s.is_empty()) {
        return std::path::PathBuf::from(dir).join("config.toml");
    }
    let base = if let Some(xdg) = get("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        std::path::PathBuf::from(xdg)
    } else if let Some(home) = get("HOME").filter(|s| !s.is_empty()) {
        std::path::PathBuf::from(home).join(".config")
    } else {
        std::path::PathBuf::from(".config")
    };
    base.join("herdr-file-viewer").join("config.toml")
}
```

Two details worth copying:

- **Env resolution via an injected getter**, so it's unit-testable without touching process
  env. Same for the filesystem in `load_config`. Their comment notes this mirrors herdr's
  own `resolve_program` / `parse_context` idiom.
- **The relative fallback is treated as untrusted.** With no `HERDR_PLUGIN_CONFIG_DIR` /
  `XDG_CONFIG_HOME` / `HOME`, the path is cwd-relative, and a browsed repo could plant a
  `.config/<plugin>/config.toml` to inject values. They only trust absolute paths.

`herdr plugin config-dir <id>` prints (and creates) this directory — use it in setup docs.
Docs are explicit that `HERDR_PLUGIN_ROOT` is a managed source checkout: do not store
durable state or user config there. Runtime state goes in `HERDR_PLUGIN_STATE_DIR`.

### What the config should contain

| Kiosk key | Decision |
|---|---|
| `search_dirs` (with per-entry `depth`, `~` expansion) | **Keep.** The one thing herdr genuinely can't tell us. |
| `[session] split_command` | **Delete.** See §7. |
| worktree directory | **Delete.** Herdr's `worktrees.directory`. |
| `[theme]` | Slim right down. Consider defaulting to terminal ANSI so the picker inherits the user's herdr theme — herdr supports `theme.name = "terminal"` for exactly this reason. |
| `[keys]` | Keep, but **in-TUI keys only**. The launch keybinding lives in herdr's own `config.toml`. |
| `[agent]`, `[agent.labels]` | **Delete.** |

There is no herdr-managed plugin storage API in v1 — plugins own their own files, schemas,
migrations, and cleanup.

---

## 7. Ecosystem: what not to build

**Do not build layout bootstrapping.** Several existing plugins already hook
`[[events]] on = "worktree.created"` and apply declarative tab/pane layouts:

- `cloudmanic/herdr-plus` — `worktrees/` dir of TOML auto-layouts that fire on matching
  worktree creation
- `razajamil/herdr-plugin-workspace-manager` — declarative tab/pane layouts with
  per-workspace defaults applied when a worktree is created
- `persiyanov/herdr-reviewr` — auto-opens its sidebar on `worktree.created`

This plugin creates the worktree; those lay out the panes. Staying out of it is both the
idiomatic move and the composable one. It's also why `[session] split_command` should go.

Other plugins reviewed for API patterns (none does streaming multi-directory repo discovery
with async remote branch loading, which remains the differentiator):

- `ogulcancelik/herdr-plugin-examples` — official cookbook: `rust-release-check` (Rust +
  install-time build), `github-link-preview` (action-opens-pane shim), `agent-telegram-notify`,
  `dev-layout-bootstrap`
- `smarzban/herdr-file-viewer` — Rust/ratatui, split pane, best-documented manifest in the
  ecosystem, prebuilt-or-build distribution
- `persiyanov/herdr-reviewr` — Rust TUI sidebar, prebuilt download, config validation
- `NathanFlurry/herdr-plugin-jj-workspace` — Rust, creates herdr workspaces, overlay wizard

---

## 8. Manifest, and the patterns behind each choice

```toml
id = "thomasschafer.<name>"
name = "<Name>"
version = "0.1.0"
min_herdr_version = "0.7.0"
description = "Fuzzy-find git repos and branches, open them as Herdr workspaces."
platforms = ["linux", "macos"]

[[build]]
command = ["/bin/sh", "scripts/fetch-or-build.sh"]

[[panes]]
id = "picker"
title = "Repos"
placement = "popup"
width = "90%"
height = "90%"
command = ["sh", "-c", "exec \"$HERDR_PLUGIN_ROOT/bin/<name>\""]

[[actions]]
id = "open-picker"
title = "Open repo picker"
contexts = ["workspace", "global"]
command = ["bash", "scripts/open-picker.sh"]
```

### Why `platforms = ["linux", "macos"]` — skip Windows for v1

`herdr-file-viewer`'s manifest documents this, verified on real hardware against herdr
0.7.1-preview (their GH #58):

> herdr can NOT spawn the manifest's RELATIVE pane command on Windows — it passes the
> relative program to CreateProcessW, which resolves it against herdr's OWN directory (not
> any `--cwd` we set), failing with ERROR_PATH_NOT_FOUND (os error 3). herdr also reports the
> plugin root as a `\\?\` verbatim path and does NOT append `.exe`.

Their workaround is a parallel set of PowerShell actions that locate the plugin root via
`plugin list --json`, strip the `\\?\` prefix, and spawn by absolute path with
`pane split` / `tab create` + `pane run`, with deliberately **no** Windows `[[panes]]` entry.
Not worth it on day one. Kiosk's tmux dependency meant Windows wasn't really shipping anyway.

### Why the absolute binary path

`herdr-reviewr` uses `sh -c 'exec "$HERDR_PLUGIN_ROOT/bin/herdr-reviewr"'` because the pane's
cwd is not guaranteed to be the plugin root (a launcher may pass `--cwd`). More robust than
`./target/release/...`, which is what the simpler examples use.

### Why the action is a shim

Keybindings bind to **actions**, not panes. The official `github-link-preview` example
confirms the pattern — its action is a bash script whose entire body is:

```bash
exec "$herdr_bin" plugin pane open \
  --plugin examples.github-link-preview \
  --entrypoint preview \
  --placement split \
  --direction right \
  --env "GITHUB_URL=$url" \
  --focus
```

So `scripts/open-picker.sh` is the direct analogue with `--placement popup`.

### Other manifest constraints

- Action / pane / link-handler ids are local to the plugin, may use ASCII letters, digits,
  colon, underscore, hyphen — **but not dots**. Each id type must be unique within the plugin.
  herdr-file-viewer's comment notes duplicates are rejected at load time *regardless of
  platform gating*, which is why their Windows variants carry `-windows`-suffixed ids.
- Plugin ids may use letters, digits, dot, colon, underscore, hyphen. Owner-prefixed dotted
  form is the ecosystem convention (`examples.foo`, `cloudmanic.herdr-plus`,
  `nathanflurry.jj-workspace`, `persiyanov.reviewr`).
- `contexts = ["workspace", "global"]` — the picker must work when nothing git-related is
  focused. `jj-workspace` uses exactly this.
- Note: as of herdr 0.7.0, `contexts` is accepted but the built-in right-click menu does not
  render plugin actions, so keybindings are the only surfacing mechanism today.

### User-side keybinding (one-time, manual, document it in the README)

```toml
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "thomasschafer.<name>.open-picker"
description = "open repo picker"
```

Then `herdr server reload-config`.

---

## 9. Distribution

Both mature Rust plugins converged on the same `[[build]]` pattern, and it should be
considered best practice:

> Fast path: download the prebuilt binary that matches THIS source's declared version +
> platform from the GitHub release, verify its SHA-256, and install it at
> `target/release/<name>`. The match is by VERSION, not by exact commit [...] Fallback: on
> ANY miss (no asset for this version, network/download error, checksum mismatch, unmapped
> platform, no curl/wget) print a clear notice and build from source with cargo — identical
> to the pre-prebuilt behavior, so installing never gets harder than before.

Details worth carrying over from `herdr-file-viewer/scripts/fetch-or-build.sh` (142 lines):

- Source `~/.cargo/env` before checking for cargo, guarded by `[ -f ]`, because herdr may
  have been launched without `~/.cargo/bin` on `PATH` (GUI / login-less launch).
- Make paths and the release base URL overridable via env (`FV_REPO_ROOT`, `FV_OUT`,
  `FV_BASE_URL`, ...) so the script can be tested hermetically with stubbed
  `uname`/`curl`/`cargo`.
- Emit a clear install message pointing at rustup when cargo is genuinely absent.

Kiosk already has a cross-compile release matrix in `.github/`, so this is mostly a port.

Herdr-side facts:

- `[[build]]` runs on GitHub `plugin install` after confirmation, before registration. A
  failing build aborts install.
- `plugin link` does **not** run build commands — local devs build their own tree first.
  Document this.
- Build commands do not receive runtime plugin context or herdr socket env.
- Changing `herdr-plugin.toml` after the install preview aborts the install.
- There is no `plugin update` in v1; reinstall from GitHub to refresh.

Marketplace listing is automatic: add the GitHub topic `herdr-plugin` to a public repo with
a `herdr-plugin.toml`. The index refreshes every 30 minutes, is unreviewed, and does not
parse the manifest in v1 (so repo description and topics are what make the listing useful).

---

## 10. Gotchas and things to verify early

### cwd is not what you think

Runtime commands run with the **plugin directory** as their working directory.
`std::env::current_dir()` will not give the user's repo. Kiosk uses cwd to mark the current
branch (`BranchEntry::build_sorted_with_activity` takes a `cwd: Option<&Path>`), so this
matters.

Read it from `HERDR_PLUGIN_CONTEXT_JSON` instead. The official Rust example does exactly
this:

```rust
let context = env::var("HERDR_PLUGIN_CONTEXT_JSON").unwrap_or_default();
let Some(repo) = json_string_field(&context, "focused_pane_cwd")
    .or_else(|| json_string_field(&context, "workspace_cwd"))
else { /* ... */ };
```

`HERDR_PLUGIN_CONTEXT_JSON` can include workspace, tab, focused pane, worktree, agent,
selected text, clicked URL, and link handler fields when available. `herdr pane get` also
exposes `foreground_cwd` when herdr can resolve the cwd of the foreground process.

### Popup lifecycle vs. focus changes — TEST THIS FIRST

A popup is session-modal and *"does not change plugin focus context"*. On Enter, the plugin
shells out to `workspace focus` / `worktree create --focus`, i.e. asks herdr to change focus
while a modal popup owns terminal input.

Design for it: fire the herdr call and exit the TUI immediately so the popup tears down.
`popup.close` exists on the socket API if an explicit close is needed. Worth validating
by hand before building around it.

### `ui_busy`

`plugin.pane.open` returns `ui_busy` while Settings, Copy mode, or another herdr modal is
active. The launcher script should surface that legibly rather than failing silently.

### Event-hook name validation

`[[events]]` `on` values are validated against known herdr event names at link time. An
unrecognised name is not a hard error — the link succeeds — but the response carries a
warning (e.g. `"unknown event 'worktree.craeted'"`). Check the `warnings` field in
`plugin.link` / `plugin.list` responses. (Not needed for v1 if there are no event hooks,
but relevant if any get added.)

### Trust framing for the README

Herdr does not sandbox or review plugins; build and runtime commands run as the user with
their environment and full CLI access. `herdr plugin install` shows a preview of source and
commands in interactive terminals. Worth a short honest note in the README, as the mature
plugins do.

---

## 11. Local development loop

```sh
cargo build --release
herdr plugin link /path/to/plugin          # does NOT run [[build]]
herdr plugin action list --plugin thomasschafer.<name>
herdr plugin action invoke thomasschafer.<name>.open-picker
herdr plugin pane open --plugin thomasschafer.<name> --entrypoint picker --placement popup
herdr plugin log list --plugin thomasschafer.<name>
herdr plugin config-dir thomasschafer.<name>
herdr plugin unlink thomasschafer.<name>
```

`plugin link` accepts a plugin directory containing `herdr-plugin.toml` or a direct manifest
path. Installing over a locally linked plugin is refused — unlink first. Linked and installed
plugins persist across restarts via a `plugins.json` registry written alongside `session.json`.

`herdr api schema --json` prints the full JSON Schema for the socket protocol bundled with
the installed binary — useful for checking exact request/response shapes rather than trusting
the prose docs.

---

## 12. Reference links

- herdr: https://github.com/ogulcancelik/herdr
- Plugin authoring: https://herdr.dev/docs/plugins/
- CLI reference: https://herdr.dev/docs/cli-reference/
- Socket API: https://herdr.dev/docs/socket-api/
- Configuration: https://herdr.dev/docs/configuration/
- Marketplace: https://herdr.dev/plugins/
- Official examples: https://github.com/ogulcancelik/herdr-plugin-examples
- kiosk (source of the port): https://github.com/thomasschafer/kiosk
- herdr-file-viewer (best-documented Rust plugin): https://github.com/smarzban/herdr-file-viewer
- herdr-reviewr: https://github.com/persiyanov/herdr-reviewr
- herdr-plugin-jj-workspace: https://github.com/NathanFlurry/herdr-plugin-jj-workspace

---

## 13. Suggested first milestone

1. New repo, workspace with a single binary crate (kiosk's three-crate split exists to
   support a CLI surface that isn't in scope here — reassess later if a CLI is wanted).
2. Copy `git/` + `event.rs` + the rayon spawn helpers; strip session/agent variants.
3. Get the repo picker rendering with streaming discovery — this is the load-bearing UX and
   should be proven first.
4. Add `HerdrProvider` with `focus_or_create_workspace(repo_path)`.
5. Wire the manifest + shim script, `plugin link`, and validate the popup focus lifecycle
   by hand (§10).
6. Branch picker with local branches.
7. Remote branch streaming + greying + `git branch --track` two-step.
8. Config loading (§6), then `fetch-or-build.sh` and release CI.
