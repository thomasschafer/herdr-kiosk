# herdr-kiosk design and decisions

This is the durable design and decisions reference for the shipped plugin. Future work
and deferred items live in [the backlog](internal/backlog.md); the real-MRU design lives
in [the herdr-driven MRU plan](internal/herdr-driven-mru-plan.md).

## 1. Goal

herdr-kiosk ports the core of [kiosk](https://github.com/thomasschafer/kiosk) to herdr:
a fast, streaming fuzzy picker for repositories and branches that opens or creates the
appropriate herdr workspace and worktree. Agent detection and the sessions view remain
herdr's responsibility.

## 2. Decisions log

These decisions shaped v1:

| # | Decision | Detail |
|---|---|---|
| D1 | New repo, not a fork | Copy only what's needed from kiosk |
| D2 | Repo already open | Focus it (server-side via `worktree open`, see §4) |
| D3 | Remote branch | Create local tracking branch + new worktree; never switch an existing checkout's branch. Fix kiosk's hardcoded `origin/` — use `BranchEntry.remote` |
| D4 | Config | Built fresh, idiomatic for herdr (`$HERDR_PLUGIN_CONFIG_DIR/config.toml`, XDG fallback, absolute-paths-only trust rule). `search_dirs` with per-entry depth is the core of it |
| D5 | New-branch flow | In scope: type a new name → pick base branch → branch + worktree + workspace |
| D6 | Worktree deletion | In scope, with confirmation dialog. Open workspaces via `herdr worktree remove`; checkouts with no open workspace via direct `git worktree remove`. Branches are never deleted |
| D7 | Open indicators | In scope: show which repos/branches are already open as herdr workspaces (from `worktree list --json` `open_workspace_id`, `workspace list --json` for the repo view) |
| D8 | Background git fetch | Keep kiosk's behaviour: fetch all remotes on entering the branch view, bounded pool, results streamed in |
| D9 | Setup wizard | Keep. First run with no config gets the wizard, ported from kiosk. UX priority over code size |
| D10 | Naming | Repo/dir/binary: `herdr-kiosk`. Plugin id: `thomasschafer.herdr-kiosk`. One name everywhere |
| D11 | Theme | ANSI-16 + terminal default fg/bg so the picker visually matches everything else rendered in the user's terminal/herdr (see §6). Slim `[theme]` override section kept |
| D12 | Windows | Added before release after an initial Unix-only scope; full parity is maintained |
| D13 | Crate layout | Single binary crate. Kiosk's three-crate split served its CLI surface, which herdr replaces |
| D14 | Collision display | Repos sharing a name are disambiguated with the shortest distinguishing parent-path suffix (see §5) |
| D15 | Process | Claude orchestrates/plans/verifies, Codex (gpt-5.6 sol, high reasoning) implements, Tom decides product questions and hand-tests (see §7) |
| D16 | Version control | Commit each verified chunk and push to `origin main` (github.com/thomasschafer/herdr-kiosk) as we go; no co-author lines. Pause for Tom only when something genuinely needs review |
| D17 | Performance | Users may have very large repo/branch counts. Correctness first, but weigh allocation, process-spawn, and traversal costs in reviews as a standing concern; pragmatic, not premature |
| D18 | Help esc | `esc` always closes the help overlay, even with a non-empty help query; the overlay owns and discards its own query, leaving the underlying picker query untouched |
| D19 | Editor-on-open | After opening or creating a workspace, optionally split a pane and run a command (e.g. `hx`), configured via `[on_open]`. Uses herdr `pane split --direction` + `pane run` |

## 3. Source-verified herdr facts (0.7.4)

1. `min_herdr_version = "0.7.4"`, not 0.7.0. Session-modal popup panes for plugins
   were added in 0.7.4 (changelog #1125). Popups are the launch surface, so 0.7.4 is
   the floor.
2. Enter-on-repo is one call, not list-and-match. `workspace list --json` has no cwd
   field, so cwd matching is unavailable. Instead:
   `herdr worktree open --cwd <repo> --path <repo-root> --focus` is idempotent
   server-side. `open_workspace_idx_for_checkout` (herdr `src/app/api/worktrees.rs`)
   matches open workspaces by worktree membership, cached git-space metadata, and live
   terminal cwd — so it also catches repos opened as plain workspaces outside the
   worktree API. It focuses if open and auto-creates the parent workspace if not.
3. Enter-on-branch needs an explicit open-vs-create decision. `worktree create` on an
   existing local branch runs `git worktree add <path> <branch>`, which git refuses if
   the branch is checked out anywhere. Rule: checkout already exists for the branch →
   `worktree open --branch X`; otherwise → `worktree create --branch X`. The picker
   knows which from its own `git worktree list --porcelain` enrichment.
4. Worktree create/remove are internally async in 0.7.4 (`worktrees/deferred.rs`). The
   CLI still blocks until completion, but handle the new error codes:
   `worktree_operation_in_progress` (e.g. double-Enter on a slow create) and
   `stale_worktree_operation`.
5. Default checkout path collisions: herdr's `<worktrees.directory>/<repo-name>/<branch-slug>`
   is keyed by repo directory name. Original intent was to pass an explicit `--path`
   when two repos share a name — but herdr exposes `worktrees.directory` through no
   API or CLI (verified: `herdr config` only has check/reset-keys), so the plugin
   cannot compute a correct sibling path without fragile parsing of herdr's own
   config file. Deferred: the double collision (same repo name AND same branch) makes
   herdr's `git worktree add` fail on the existing directory, which we surface as a
   clean error toast. Watch item: request upstream exposure (e.g. `worktree create
   --path-suffix` or config in an API response) if this bites in practice.

## 4. Architecture

Single Rust binary crate. Stack matches kiosk: ratatui 0.30, crossterm 0.29,
fuzzy-matcher (SkimMatcherV2), rayon, unicode-width/segmentation, serde + toml.

### Module map (ported from kiosk unless marked new)

| Module | Source | Notes |
|---|---|---|
| `git/` | kiosk-core `git/` | `scan_repos_streaming`, `walk_repos`, branch/remote/worktree listing, porcelain parsing, linked-worktree dedup (load-bearing: never hand a linked worktree path to herdr — `linked_worktree_source` error). Drop tmux-session helpers |
| `event.rs` | kiosk-core | Keep `ReposFound`, `RepoEnriched`, `ScanComplete`, `BranchesLoaded`, `RemoteBranchesLoaded`, `GitFetchCompleted`, `WorktreeCreated`, `WorktreeRemoved`, `WorktreeRemoveFailed`, `GitError`. Drop all session/agent variants. Add open-state events (D7) |
| `spawn.rs` | kiosk-tui | `spawn_repo_discovery`, `spawn_branch_loading`, `spawn_remote_branch_loading`, `spawn_git_fetch`, `spawn_work_parallel`; keep bounded pools (`ENRICHMENT_POOL_SIZE = 8`, `FETCH_POOL_SIZE = 4`) |
| `state.rs` | kiosk-core | Slimmed. `BranchEntry` keeps `name`, `worktree_path`, `is_current`, `is_default`, `remote`; drops `has_session`/`session_activity_ts`/`agent_statuses`; gains `open_workspace_id: Option<String>` (D7) |
| `herdr.rs` | new | `HerdrProvider` trait + CLI impl (via `HERDR_BIN_PATH`) + mock, in the same seam style as kiosk's `GitProvider`/`TmuxProvider`. Surface: `worktree_open`, `worktree_create`, `worktree_remove`, `worktree_list`, `workspace_list`. Parse `--json` responses; surface herdr error codes as typed errors |
| `context.rs` | new | Parse `HERDR_PLUGIN_CONTEXT_JSON` (cwd for current-repo/branch highlighting — never `std::env::current_dir()`, which is the plugin dir) |
| `config/` | kiosk-core | `search_dirs` (+depth, `~` expansion), `[keys]` (in-TUI only), slim `[theme]`. Env/fs access via injected getters for testability. Trust only absolute config paths. Drop `[session]`, `[agent]`, worktree-dir |
| `keyboard.rs`, `keymap` | kiosk-core/tui | Slimmed to surviving actions |
| components | kiosk-tui | `repo_list`, `branch_picker`, `search_bar`, `list_row`, `new_branch`, `dialog`, `error_toast`, `path_input`, `help`, `setup`, `theme` (slimmed per D11). Drop `sessions_view` |

### Herdr interaction map (exact calls)

| User action | Call(s) |
|---|---|
| Enter on repo | `worktree open --cwd <repo> --path <repo-root> --focus` |
| Enter on branch with existing checkout | `worktree open --cwd <repo> --branch <name> --focus` |
| Enter on branch without checkout | `worktree create --cwd <repo> --branch <name> --focus` (+ `--path` on repo-name collision) |
| Enter on remote-only branch | `git -C <repo> branch --track <branch> <remote>/<branch>` (guard: branch may have appeared since listing), then `worktree create` as above |
| New branch flow | `worktree create --cwd <repo> --branch <new> --base <base> --focus` |
| Delete worktree (workspace open) | `worktree remove --workspace <id>` (handle `dirty_worktree_requires_force` → confirm → `--force`) |
| Delete worktree (not open) | `git worktree remove` (+ prune fallback, ported from kiosk) |
| Open indicators | `worktree list --cwd <repo> --json` (`open_workspace_id` per entry); `workspace list --json` for the repo view |

After a successful open/create call the TUI exits immediately so the popup tears down and
the focus change lands (verified: `switch_workspace` has no popup guard; the popup is
session-modal until its process exits). During the blocking create, show kiosk's Loading
spinner state.

### Launch surface

- `[[panes]] id = "picker"`, `placement = "popup"`, width/height 90%, command
  `["sh", "-c", "exec \"$HERDR_PLUGIN_ROOT/bin/herdr-kiosk\""]` (absolute path — pane
  cwd is not guaranteed to be the plugin root).
- `[[actions]] id = "open-picker"`, `contexts = ["workspace", "global"]`, shim script
  exec-ing `"$HERDR_BIN_PATH" plugin pane open --plugin thomasschafer.herdr-kiosk
  --entrypoint picker`. Surface `ui_busy` legibly.
- User keybinding (README, one-time): `[[keys.command]] type = "plugin_action"`.
- Manifest supports Linux, macOS, and Windows, with platform-specific pane, action, and
  build entries.

## 5. UX spec deltas vs kiosk

Kiosk's picker UX is the reference; differences only:

- No sessions view, no agent badges. Open-workspace indicator instead (D7): a marker on
  repos with any open workspace and on branches whose checkout is open.
- Collision display (D14): repos sharing a display name render as
  `name (disambiguator)` where the disambiguator is the shortest contiguous suffix of
  the parent path that makes the pair unique, always ending at the immediate parent —
  `foo/bar/baz` vs `qux/bar/baz` → `baz (foo/bar)` and `baz (qux/bar)`. Elide deeper
  prefixes as `…/`. Fuzzy search matches the visible disambiguator too. (Replaces
  kiosk's `apply_collision_resolution`, which existed for session naming.)
- Worktree paths belong to herdr (`worktrees.directory`); the `.kiosk_worktrees/`
  convention and `[session] split_command` are gone.
- Current repo/branch highlighting is driven by `HERDR_PLUGIN_CONTEXT_JSON`
  (`focused_pane_cwd` → `workspace_cwd` fallback).

## 6. Theme approach (D11 rationale)

Herdr's `[theme]` styles herdr's own chrome only. Pane content is not remapped: herdr
answers a pane's OSC 10/11 queries with the host terminal's fg/bg (passthrough) and
OSC 4 with the pane's emulated palette (verified in `src/pane/terminal.rs`). There is no
API that exposes the active herdr theme, so literally reusing e.g. tokyo-night is not
possible without vendoring herdr's theme tables (brittle; rejected).

What we do instead: render exclusively with ANSI-16 + terminal default fg/bg. Every pane
in herdr (nvim, htop, …) renders in the host palette, so this is exactly what "matching
herdr" looks like in practice, and it is what herdr's own `theme.name = "terminal"` mode
is designed around. A slim `[theme]` config section provides explicit overrides;
light-terminal users can set `muted`, `border`, and other colors there.

## 7. Working process and future work

Working process: see the project memory; work now lands via PRs into a protected `main`.

Future work and deferred items: see [the backlog](internal/backlog.md).
