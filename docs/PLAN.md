# herdr-kiosk plan

Living document. Update as work progresses: mark tasks done, record decisions, add
discoveries. The companion research doc is [herdr-plugin-research.md](herdr-plugin-research.md);
where this plan and that doc disagree, this plan wins (it incorporates source-verified
corrections against herdr 0.7.4).

## 1. Goal

A herdr plugin that ports the core of [kiosk](https://github.com/thomasschafer/kiosk):
a fuzzy repo picker with streaming discovery, Tab into a branch picker (local first,
remotes streamed in), Enter opens a herdr workspace — on a worktree when a branch is
selected, creating the worktree/branch as needed. Snappiness of the streaming pickers is
a hard requirement. Agent detection and the sessions view are out of scope (herdr owns
those). Layout bootstrapping is out of scope (other plugins own that).

## 2. Decisions log

Product decisions made so far. Add new entries as they're made; don't relitigate silently.

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
| D12 | Windows | Not in v1, but an early milestone (M8) with full parity maintained afterwards — not a someday item |
| D13 | Crate layout | Single binary crate. Kiosk's three-crate split served its CLI surface, which herdr replaces |
| D14 | Collision display | Repos sharing a name are disambiguated with the shortest distinguishing parent-path suffix (see §5) |
| D15 | Process | Claude orchestrates/plans/verifies, Codex (gpt-5.6 sol, high reasoning) implements, Tom decides product questions and hand-tests (see §8) |
| D16 | Version control | Commit each verified chunk and push to `origin main` (github.com/thomasschafer/herdr-kiosk) as we go; no co-author lines. Pause for Tom only when something genuinely needs review |
| D17 | Performance | Users may have very large repo/branch counts. Correctness first, but weigh allocation, process-spawn, and traversal costs in reviews as a standing concern; pragmatic, not premature |

## 3. Corrections to the research doc (source-verified against herdr 0.7.4)

These override the research doc:

1. `min_herdr_version = "0.7.4"`, not 0.7.0. Session-modal popup panes for plugins
   were added in 0.7.4 (changelog #1125). Popups are the launch surface, so 0.7.4 is
   the floor.
2. Enter-on-repo is one call, not list-and-match. `workspace list --json` has no cwd
   field, so the research doc's "match on cwd" is unimplementable. Instead:
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
- Manifest: `platforms = ["linux", "macos"]` until M8.

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
is designed around. Optional polish (M7): query OSC 11 for the background, infer
light/dark, and pick dim-shade variants accordingly — herdr answers this query. A slim
`[theme]` config section remains for overrides.

## 7. Milestones

Statuses: `[ ]` todo, `[~]` in progress, `[x]` done. Add follow-up tasks inline as
they're discovered.

### M0 — de-risk and harness
- [x] Probe plugin validated the full popup lifecycle against herdr 0.7.4: env
  injection as documented, all input delivered including Escape, fire-and-exit
  teardown with focus landing on the opened/created workspace, `worktree create`
  honouring `worktrees.directory` with sidebar grouping, idempotent re-open
  (`already_open`, no duplicate), and clean `plugin_pane_open_failed: "popup already
  open"` on double-open (surfaced via `plugin log list`). Findings and harness
  constraints recorded in [VERIFYING.md](VERIFYING.md).
- [x] Headless e2e harness proven (Tom's tmux approach): herdr client inside a
  dedicated tmux server, `send-keys` to drive, `capture-pane` to read the real screen
  popup included; sandbox `$HOME` isolates config/registry/socket/worktrees. Gotchas:
  sandbox HOME must be a short path (macOS `sun_path` ~104-byte socket limit);
  `workspace list` is JSON-by-default with no `--json` flag; herdr source builds need
  its nix dev shell (zig 0.15 pin). See [VERIFYING.md](VERIFYING.md).
- [x] Codex headless loop confirmed: `codex exec` (resume support for feedback rounds);
  `~/.codex/config.toml` already pins `gpt-5.6-sol` + `model_reasoning_effort = "high"`.
- [x] Local herdr 0.7.4 binary built from `../herdr` via
  `nix develop --command cargo build --release`.
- [ ] Check whether plugin actions appear in herdr 0.7.4 right-click menus (research doc
  says not as of 0.7.0); document the finding in the README keybinding section.
  (Needs mouse-event scripting or a hand-check; deferred to M7 polish.)
- [x] `ui_busy` from herdr's own modals: observed live during M1 verification (the
  first-run welcome dialog blocks popup opening; error surfaced in `plugin log list`).
  Harness must dismiss onboarding before invoking popup actions — see VERIFYING.md.

### M1 — scaffold
- [x] Cargo project (single crate, edition 2024, kiosk's lint posture), `herdr-plugin.toml`
  (id `thomasschafer.herdr-kiosk`, `min_herdr_version = "0.7.4"`, linux+macos), action
  shim script, `.gitignore`, justfile (build/lint/test/link/unlink, `HERDR` env
  override). Implemented by Codex; placeholder TUI with panic-safe terminal restore
  and tested quit handling. Verified independently: fmt/clippy/build/test clean, and
  the linked plugin's popup opened, rendered, and quit cleanly inside a real herdr via
  the tmux harness.
- [x] CI skeleton: fmt, clippy, test on ubuntu-latest + macos-latest.
- [x] `[[build]]` builds from source with cargo (prebuilt fast path comes in M9).
- Environment note: this machine mixes a nix cargo (1.94) with rustup clippy (1.89);
  Codex and the orchestrator both pin `PATH` to `~/.cargo/bin` first for consistent
  toolchain runs. `just` is not currently on PATH; recipes were verified as raw
  commands.

### M2 — core port (foundations only; TUI-coupled state/keyboard/keymap move to M3
where they are exercised)
- [x] Port `git/` with tests (including linked-worktree dedup tests), `event.rs`
  slimmed, the `spawn_work_parallel` helper. `Repo` loses `session_name`;
  `BranchEntry` per §4.
- [x] `HerdrProvider` trait + CLI implementation + mock; typed errors for the herdr
  error codes in §3.4 and §4 (plus `ui_busy`, `plugin_pane_open_failed`).
- [x] `context.rs` for `HERDR_PLUGIN_CONTEXT_JSON`.
- [x] Config loading with injected env/fs, absolute-path trust rule, `search_dirs` + depth.
- Review round 1 applied: scan error containment (loud for unreadable top-level
  search dirs; `ScanWarning`s for nested/per-repo failures so one bad directory or
  repo cannot blank the picker) and forward-tolerant `AgentStatus`
  (`#[serde(other)]`) so future herdr status values cannot break `workspace list`
  parsing. Carried into the M3 spec: render `ScanWarning`s and config warnings as
  in-TUI toasts (stderr is invisible under the alternate screen), and warn visibly
  when a configured search dir does not exist (currently filtered silently).

### M3 — repo picker (the load-bearing UX)
- [x] Streaming repo discovery rendering as results arrive; fuzzy search; collision
  display per §5; current-repo highlight from context.
- [x] Enter → `worktree open` → exit. Loading state while the call runs; error toast on
  failure.
- [x] Open-workspace indicators (D7).
- [x] E2e: fixture repos discovered, filtered, opened; workspace focused/created asserted
  via harness. Harness promoted to `scripts/e2e.sh` + `just e2e` (env-overridable
  sandbox/binary) — the standing e2e gate for every milestone from here on.
- Reviewed and independently verified (commit 5acd139): 69 tests, clean lints, e2e
  PASS in a fresh sandbox. Notable implementation upgrades over kiosk, accepted in
  review: fuzzy filtering on a coalescing worker thread with generation-guarded
  results (keeps typing responsive at large repo counts, per D17), bounded fallback
  in `spawn_work_parallel`, and non-blocking toast queue instead of kiosk's modal
  errors. Awaiting Tom's hand-test of picker feel in a real herdr.

### M4 — branch picker
- [ ] Tab on repo → branch view: local branches + worktree enrichment; current/default
  markers; Enter open-vs-create rule from §3.3; back navigation.
- [ ] Open indicators per branch.
- [ ] E2e: existing-checkout open, new-checkout create, branch-checked-out-in-main-checkout
  case (resolves to the source workspace).

### M5 — remotes
- [ ] `git fetch` per remote on entering branch view (pool of 4), streamed updates,
  spinner while fetching (D8).
- [ ] Remote-only branches streamed in, greyed, dedup against local names, ` (remote)`
  suffix; Enter → tracking two-step with `BranchEntry.remote` (not hardcoded origin) and
  the branch-already-exists guard.
- [ ] E2e with a local bare "remote" fixture.

### M6 — new branch + deletion
- [ ] New-branch flow (D5): name input → base branch picker → `worktree create --base`.
- [ ] Deletion (D6): confirmation dialog; open-workspace path via herdr (incl. dirty →
  force confirmation flow), closed-checkout path via git; `pending_delete` port.
- [ ] E2e for both, including dirty-worktree force path.

### M7 — wizard, config polish, UX finish
- [ ] Setup wizard port (D9): first-run flow writing config to
  `$HERDR_PLUGIN_CONFIG_DIR/config.toml` (path completion, dir management as in kiosk).
- [ ] `[keys]` in-TUI keybinding config; help overlay reflecting actual bindings.
- [ ] Optional OSC 11 light/dark refinement (§6). Error-toast and edge-case polish pass.

### M8 — Windows (early, then parity forever after)
- [ ] Re-verify the file-viewer Windows findings against current herdr: relative pane
  command spawn (CreateProcessW against herdr's dir), `\\?\` plugin root paths, no
  `.exe` appending. Herdr is moving fast — the 0.7.1-era findings may be stale.
- [ ] Whatever the findings, ship: platform-gated manifest entries (`-windows` suffixed
  ids where needed), PowerShell shim(s), pane spawn by absolute path.
- [ ] Windows CI (build + test at minimum; e2e if the harness ports).
- [ ] From here on, every milestone's work lands with Windows parity or a tracked
  exception.

### M9 — distribution and publishing
- [ ] `fetch-or-build.sh`: version-matched prebuilt download + SHA-256 verify, cargo
  fallback, `~/.cargo/env` sourcing, env-overridable paths for hermetic tests.
- [ ] Release CI: cross-compile matrix (port from kiosk's `.github/`), checksums.
- [ ] README: install, keybinding setup, config reference, trust note (herdr doesn't
  sandbox plugins), `plugin link` dev workflow, "no `plugin update` in v1 — reinstall
  to refresh" note for users.

Publishing to herdr.dev/plugins (verified against herdr's marketplace doc; the index
is automatic and unreviewed):

1. Repo must be public on GitHub at `thomasschafer/herdr-kiosk` with
   `herdr-plugin.toml` at the root (already true locally). `herdr plugin install
   thomasschafer/herdr-kiosk` works from that alone — no registry, no submission.
   The local repo currently has no commits/remote; first push is Tom's call.
2. Install runs `[[build]]` on the user's machine after a confirmation preview, and a
   build failure aborts the install — so publish-readiness means a fresh clone builds
   with nothing but cargo (fetch-or-build makes that fast; plain `cargo build
   --release` is acceptable but slow before prebuilts exist).
3. Pre-publish verification (harness): in a sandbox HOME, run
   `herdr plugin install thomasschafer/herdr-kiosk --yes` against the real GitHub
   repo and drive the installed (not linked) plugin end to end. Note: installing over
   a locally linked plugin is refused — unlink in the sandbox first.
4. Listing: add the GitHub topic `herdr-plugin` to the repo. That topic is the only
   signal the index uses; it refreshes every ~30 minutes. Dropping the topic delists
   on the next refresh. Forks and archived repos are excluded.
5. The card shows only GitHub metadata — repo name/owner, description, stars,
   primary language, last push. The index does not parse the manifest in v1, so the
   GitHub repo description is the marketing surface; write it deliberately (e.g.
   "Fuzzy-find git repos and branches and open them as Herdr workspaces/worktrees").
6. Gate: don't add the topic until install-from-GitHub passes (3) and the README
   covers setup + keybinding + trust note. Tagged releases: manifest `version` must
   match the release assets fetch-or-build looks for (match is by version, not
   commit).

## 8. Working process (D15)

Three roles:

- Tom: product decisions, end-to-end hand-testing (especially popup feel/keybindings,
  which automation can't fully cover), final arbiter.
- Claude (this assistant): planning, task specification, orchestration, independent
  verification, plan upkeep. Writes no implementation code.
- Codex (gpt-5.6 sol, high reasoning, headless): all implementation.

Loop per task batch:

1. Claude writes a task spec from this plan: scope, files, acceptance criteria, exact
   verification commands (tests + harness invocations), and relevant context (e.g. the
   §3 corrections so Codex doesn't rediscover or contradict them).
2. Codex implements and must verify before reporting: fmt, clippy, `cargo test`, and the
   M0 harness end-to-end where the task touches runtime behaviour. A report without
   verification evidence goes back.
3. Claude independently verifies: clean build, tests, runs the harness itself, reviews
   the diff for correctness and for drift from this plan, and pushes feedback to Codex.
   Iterate until green.
4. Claude updates this plan (statuses, discoveries, decisions needed) and reports to Tom:
   what landed, what was verified and how, what needs hand-testing, any product
   questions. Git commits only when Tom asks.

Notes: popup-placement behaviour is the one area automation can't reach (not a pane), so
each report flags whether popup-relevant behaviour changed and needs a hand-test.
Product questions found mid-implementation get parked in §9 and batched to Tom rather
than guessed at, unless trivially reversible.

## 9. Open questions

- (none currently — add here as they arise; move to §2 when decided)

## 10. Risks and watch items

- Herdr moves fast (0.7.0 → 0.7.4 during the research window alone). Re-verify
  behavioural assumptions when bumping the herdr version; the §3 corrections show why.
- Popup behaviour is validated manually only (not a pane). Keep the M0 manual checklist
  short and repeatable.
- `worktree create` blocks the CLI call; very slow filesystems/repos could make the
  Loading state linger. Acceptable for v1; revisit if it bites.
- Fetch-on-open (D8) can hang on interactive SSH auth. Kiosk runs it in the background
  with a spinner, which mostly hides this; carry the behaviour over and revisit if
  reports come in (e.g. `GIT_TERMINAL_PROMPT=0` for the background fetches).
- The ecosystem-plugin claims in the research doc (file-viewer's Windows findings,
  fetch-or-build details) were not locally verified; re-check them at M8/M9 time.
