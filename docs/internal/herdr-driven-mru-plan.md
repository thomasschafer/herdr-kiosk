# Real herdr-driven MRU — feasibility and design plan

Research date: 2026-07-23. This is a design/feasibility plan — the feature is NOT
implemented. It was produced by reading the herdr source and surveying peer plugins.

## Status and orientation (read this first)

**What this is.** A plan to upgrade herdr-kiosk's recency ordering from "track only the
opens kiosk itself performs" to "track the user's actual workspace focus history across all
of herdr" — so "jump back to where I last was" works even for workspaces opened outside
kiosk.

**Current state (what already exists to build on):**

- Opt-in recency sort has shipped. Config `sort = "recency"` (default `"alphabetical"`).
  The own-opens MRU store is `src/recency.rs`: a bounded, defensive JSON file
  (`recency.json`) in `HERDR_PLUGIN_STATE_DIR` recording successful repo/folder/branch
  opens (keyed by canonical path, or repo path + `BranchId`). The pickers order by it in
  recency mode (`src/screens/repo.rs`, `src/screens/branch.rs`). Toggle key `ctrl+r`.
- **Hard invariant to preserve:** alphabetical mode NEVER consults recency — the store is
  not even passed to the alphabetical sort/filter path. Any MRU work must keep this
  (focus history is consulted only when `SortOrder::Recency` is active).
- Reusable persistence: the JSON-store boilerplate (state-dir resolution + absolute-only
  guard, corruption-safe load returning `(data, warning)`, atomic temp+rename save,
  warnings) is a shared helper `src/state_store.rs`. A `focus-history.json` store should be
  built on it.
- Prior art in-repo: `src/folder_bindings.rs` already persists a canonical-path →
  `workspace_id` map and reconciles dead ids against live panes — a very similar shape to
  what a focus recorder needs.
- Provider seam: herdr calls go through the `HerdrProvider` trait in `src/herdr.rs` (CLI
  impl + mock). A new headless `record-herdr-event` subcommand would live alongside the
  interactive picker entrypoint in `src/main.rs`; the manifest is `herdr-plugin.toml`.

**Why it's worth doing.** Every peer sessionizer/launcher makes "toggle to previous" the
core motion; the landscape review ranked recency the #1 gap. Own-opens MRU (shipped) is the
baseline; real focus-driven MRU is the upgrade this doc specifies.

**How to proceed (project workflow).** Claude orchestrates + reviews; Codex implements via
`codex exec` (gpt-5.6-sol, high reasoning); work lands via PRs (`main` is branch-protected).
Every change must pass: `cargo fmt --all --check`, workspace clippy `-D warnings`, workspace
tests, Windows cross-target clippy (isolated `CARGO_TARGET_DIR`), `cargo xtask readme
--check`, and the full tmux e2e (`scripts/e2e.sh`). See `docs/PLAN.md` and the project
memory for details. The "Phased implementation outline" at the end maps onto a sequence of
PRs; **Phase 0 (the contract/performance spike) should happen before committing to the
design**, because it validates the event payloads and hook latency on a real herdr.

**Recommended answers to the open decisions** are in the section of that name below
(before "Questions for human review"); a fresh session can proceed on those defaults unless
Tom redirects. Source line numbers in this doc are approximate (captured at research time)
and may have drifted.

## Verdict

**Feasible with today's Herdr; medium implementation difficulty.**

The event-capture part is easy. `workspace.focused` and `workspace.closed` are both valid manifest `[[events]]` hooks, and Herdr gives a focus hook enough target context to record the workspace id, label, focused pane cwd, workspace cwd, and Herdr worktree provenance. Herdr launches a one-shot plugin command for each event, so Kiosk does not need a daemon. This surface has existed since plugin v1 in Herdr 0.7.0, while Kiosk already requires Herdr 0.7.4 (`../herdr/CHANGELOG.md:199-206`; `herdr-plugin.toml:1-6`).

The medium part is not receiving focus; it is:

- making concurrent one-shot writers safe on Unix and Windows;
- mapping a focused workspace back to Kiosk's path-based repo/branch/folder keys;
- defining cleanup and named-session semantics; and
- layering real focus history over the existing Kiosk-only history without regressing the explicitly pure alphabetical mode.

The recommended first version is one lightweight `workspace.focused` hook, one `workspace.closed` hook, a separate bounded `focus-history.json`, and a read-time merge in which mapped real focus history is authoritative and today's `recency.json` remains a fallback. Do not replace `recency.json` initially.

## Evidence from Herdr

### Which events may invoke plugin commands

The plugin-hook allowlist is narrower than the socket subscription list. Today it includes:

- workspace: `workspace.created`, `workspace.updated`, `workspace.closed`, `workspace.renamed`, `workspace.moved`, `workspace.focused`;
- worktree: `worktree.created`, `worktree.opened`, `worktree.removed`;
- tab: create/close/rename/move/focus;
- pane: create/close/focus/move/exit/agent-detected/agent-status-changed.

It intentionally excludes high-volume or presentation events including `workspace.metadata_updated`, `pane.updated`, `pane.output_changed`, and `layout.updated` (`../herdr/src/api/schema/events.rs:281-316`, with exclusion tests at `:345-351`). Manifest event names are validated at link/load time; an unknown name produces a warning rather than a hard link failure (`../herdr/docs/next/website/src/content/docs/socket-api.mdx:460-462`).

One important emission detail makes the minimal design sufficient: Herdr compares the current `(workspace, focused pane)` with the previous tuple and, whenever it changes, emits `workspace.focused`, `tab.focused`, and `pane.focused` for the new target (`../herdr/src/app/api.rs:794-859`). Therefore a `workspace.focused` hook runs not only when changing workspaces, but also when a different tab/pane becomes focused within the same workspace. For Kiosk's workspace-level MRU, subscribing to all three would be redundant.

### Exactly what an event command receives

Every event command receives:

- `HERDR_PLUGIN_EVENT`, using the dotted manifest name, e.g. `workspace.focused`;
- `HERDR_PLUGIN_EVENT_JSON`, the serialized event envelope;
- `HERDR_PLUGIN_CONTEXT_JSON`, an event-target invocation context;
- the normal plugin paths and socket/binary variables, plus available workspace/tab/pane ids.

The environment is assembled in `../herdr/src/app/api/plugins/runtime.rs:32-80`; the documented list is at `../herdr/docs/next/website/src/content/docs/plugins.mdx:252-284`.

The event envelope's `event` and `data.type` values use underscores, even though the manifest/env event name uses dots. A focus envelope is:

```json
{
  "event": "workspace_focused",
  "data": {
    "type": "workspace_focused",
    "workspace_id": "w3"
  }
}
```

The relevant raw `data` payloads are defined at `../herdr/src/api/schema/events.rs:414-458`:

| Hook | `HERDR_PLUGIN_EVENT_JSON.data` |
| --- | --- |
| `workspace.focused` | `type`, `workspace_id` only |
| `workspace.created` | `type`, full `workspace` snapshot |
| `workspace.closed` | `type`, `workspace_id`, optional final `workspace` snapshot |
| `worktree.created` | `type`, full `workspace`, full `worktree` |
| `worktree.opened` | `type`, full `workspace`, full `worktree`, `already_open` |
| `worktree.removed` | `type`, `workspace_id`, optional final `workspace`, full `worktree`, `forced` |

`WorkspaceInfo` contains id, number, label, focus/count/status data, active tab id, and optional `worktree`, but **no cwd**. Its worktree provenance contains `repo_key`, `repo_name`, `repo_root`, `checkout_path`, and `is_linked_worktree` (`../herdr/src/api/schema/workspaces.rs:45-69`). `WorktreeInfo` additionally contains path and optional branch plus bare/detached/prunable/linked/open-workspace metadata (`../herdr/src/api/schema/worktrees.rs:52-74`).

The bare focus event is enriched through `HERDR_PLUGIN_CONTEXT_JSON`. Its schema includes:

- `workspace_id`, `workspace_label`, `workspace_cwd`, optional `worktree`;
- active `tab_id` and `tab_label`;
- `focused_pane_id`, `focused_pane_cwd`, agent, and status.

See `../herdr/src/api/schema/plugins.rs:363-395`. For `workspace.focused`, Herdr looks up the event's workspace id rather than blindly using whichever workspace is otherwise active (`../herdr/src/app/api/plugins/context.rs:39-74`). When it can resolve the live workspace, `workspace_cwd` is the focused pane's `PaneInfo.cwd`, falling back to Herdr's default/identity cwd for the workspace; `focused_pane_cwd` is specifically that pane's `cwd` (`../herdr/src/app/api/plugins/context.rs:292-374`). It does **not** expose `PaneInfo.foreground_cwd` in plugin context.

For `workspace.closed`, the event always has the id. The optional final workspace snapshot can retain label and worktree provenance, but a closed plain workspace may have no recoverable cwd because `WorkspaceInfo` itself has no cwd (`../herdr/src/app/api/plugins/context.rs:52-65,210-224`). Cleanup must therefore key on `(session scope, workspace_id)`, not cwd.

### Runtime model and limits

An event hook is a fresh child process, not an in-process callback or supervised daemon:

1. Herdr refreshes the installed-plugin registry.
2. It builds event-target context and serialized event JSON.
3. For each matching `[[events]]` entry, it starts the manifest argv asynchronously in the plugin root.
4. A worker thread waits for that child and reports completion into Herdr's command log.

The implementation is at `../herdr/src/app/api/plugins/runtime.rs:218-266` and `:103-180`. There is no event-command timeout in this path, so a hook must exit promptly.

Limits are global across in-flight plugin actions/startup/event commands:

- maximum 32 concurrent plugin commands;
- stdout and stderr are each retained only up to 64 KiB;
- the in-memory plugin command log keeps the newest 200 records.

Evidence: `../herdr/src/app/api/plugins/runtime.rs:11-13,82-101,121-178,268-273`. A 33rd command is failed and logged; it is not queued or retried.

For a tiny native `record-herdr-event` subcommand that parses two environment variables, takes a file lock, rewrites at most 200 small records, and prints nothing, one process per human focus change is reasonable. Two real plugins already use this model successfully (see Prior art). The risk appears only under rapid programmatic focus loops, slow storage, a stuck recorder, or many other simultaneous plugin hooks. This should still be measured during implementation because Herdr documents the mechanism and cap, not a latency budget.

### `session.snapshot` is current state, not history

`session.snapshot` has only the current `focused_workspace_id`, `focused_tab_id`, and `focused_pane_id`, followed by current workspace/tab/pane/layout/agent arrays (`../herdr/src/api/schema/session.rs:6-24`). The docs describe it as a one-time bootstrap that must be followed by event subscriptions; it contains no focus timestamps or prior ids (`../herdr/docs/next/website/src/content/docs/socket-api.mdx:116-126`).

It is useful for:

- reconciling recorded workspace ids against live workspaces;
- seeding the current workspace after first install or a missing hook;
- re-resolving today's workspace/worktree/pane metadata.

It cannot reconstruct focus order. True history must be accumulated as focus events happen.

### No public Herdr MRU can replace plugin state

Herdr does keep small private toggle state: `previous_workspace` and `previous_pane_focus` implement last-workspace/last-pane behavior (`../herdr/src/app/state.rs:1395-1417`; `../herdr/src/app/actions.rs:1900-1973`). This is not a public list, timestamped history, socket field, or CLI result. It is absent from the public `SessionSnapshot` above and from the private persisted session shape, which saves workspace order plus current `active`/`selected`, but not the previous-workspace/pane fields (`../herdr/src/persist/snapshot.rs:14-29,250-279`).

Workspace `number`/array order is UI order, not recency. Agent `state_change_seq` is agent-state recency, not workspace focus. Reading Herdr's private `session.json` would therefore be both unsupported and insufficient.

Use the public event/context/CLI surface. Herdr explicitly says plugin v1 has no managed storage API and plugins own their durable state (`../herdr/docs/next/website/src/content/docs/plugins.mdx:358-361`).

## Recommended design

### Manifest additions

Use one recorder subcommand for both events. Platform-specific argv avoids relying on Windows executable-extension behavior:

```toml
[[events]]
on = "workspace.focused"
platforms = ["linux", "macos"]
command = ["./target/release/herdr-kiosk", "record-herdr-event"]

[[events]]
on = "workspace.closed"
platforms = ["linux", "macos"]
command = ["./target/release/herdr-kiosk", "record-herdr-event"]

[[events]]
on = "workspace.focused"
platforms = ["windows"]
command = ["target\\release\\herdr-kiosk.exe", "record-herdr-event"]

[[events]]
on = "workspace.closed"
platforms = ["windows"]
command = ["target\\release\\herdr-kiosk.exe", "record-herdr-event"]
```

Do not make `workspace.created`, `worktree.created`, or `worktree.opened` recency triggers: they can occur with `focus=false`, so treating an open/create as focus would make the history less truthful. A focus-producing operation already causes `workspace.focused`.

If later testing shows branch identity cannot be recovered cheaply at picker time, `worktree.created`, `worktree.opened`, and `worktree.removed` could be added as **metadata-only** hooks that enrich/delete workspace-to-checkout/branch bindings without changing focus order. That is not needed for phase 1.

Do not require a `[[startup]]` hook initially. Startup hooks were added only in Herdr 0.7.5 (`../herdr/CHANGELOG.md:5-13`), which would force Kiosk's minimum version up from 0.7.4. Picker-time reconciliation provides the needed repair without a version bump.

### State file and recorder behavior

Add a separate file under `HERDR_PLUGIN_STATE_DIR`, for example:

```json
{
  "version": 1,
  "entries": [
    {
      "session_key": "sha256:...",
      "workspace_id": "w3",
      "focused_at_unix_ms": 1784840000000,
      "workspace_label": "kiosk",
      "workspace_cwd": "/work/herdr-kiosk",
      "focused_pane_id": "w3:p2",
      "focused_pane_cwd": "/work/herdr-kiosk/src",
      "worktree": {
        "repo_key": "/work/herdr/.git",
        "repo_name": "herdr-kiosk",
        "repo_root": "/work/herdr-kiosk",
        "checkout_path": "/work/herdr-kiosk",
        "is_linked_worktree": false
      }
    }
  ]
}
```

`session_key` should be a stable hash of `HERDR_SOCKET_PATH`, not just a workspace id. As of Herdr 0.7.5, plugins and their state directories are global to the user (`../herdr/CHANGELOG.md:5-13`), and `HERDR_PLUGIN_STATE_DIR` is built from the global state dir plus plugin id (`../herdr/src/plugin_paths.rs:21-25`). Named Herdr sessions have separate sockets but can reuse ids such as `w1`; without a scope they can corrupt one another's MRU and cleanup.

On `workspace.focused`, the recorder should:

1. Parse `HERDR_PLUGIN_CONTEXT_JSON`; obtain the event id from context and/or verify it against the event envelope.
2. Capture the flattened context fields above. Preserve raw strings; use Kiosk's existing canonical/equivalent path helpers only when mapping later.
3. Under an exclusive cross-process lock, remove the prior record for the same `(session_key, workspace_id)`, insert the new record at the front, and cap at 200 records per session scope (plus a conservative total cap if multiple scopes are retained).
4. Atomically replace the JSON file with a same-directory temporary file.
5. Emit no stdout. On recoverable corruption, warn concisely, preserve/rename the bad file once, and start empty rather than crashing Herdr.

On `workspace.closed`, parse `workspace_id` from `HERDR_PLUGIN_EVENT_JSON` and remove that session-scoped entry under the same lock. Do not depend on closed-event context having a cwd.

The lock is required even with one focus hook because a close can overlap a focus, and rapid focus requests can overlap child processes. Use a portable file-locking library and test real Windows replacement semantics. Keep `focus-history.json` separate from `recency.json` in the first version so the new event writer never races today's open-success writer.

At picker startup, reconcile the current session's records against `workspace list` or `session.snapshot`, removing ids that are no longer live. This repairs missed close hooks, server crashes, and old state. A freshly linked plugin cannot know focus history from before installation; it may seed the current focused workspace from snapshot/context, but must not invent older order.

List order should be the authoritative MRU order; the timestamp is useful for diagnostics and a future schema merge. There is a narrow ordering race if two focus processes overlap and acquire the lock out of event order because Herdr supplies no event sequence/timestamp. At human switching frequency this is unlikely. If programmatic focus loops matter, a later recorder can check the current focused id via `session.snapshot` before committing, at the cost of one Herdr call per focus, or Herdr can expose an event sequence upstream.

### Mapping focus records to Kiosk entries

Kiosk's current stable sort keys are path-based:

- repo/folder: canonical path;
- branch: canonical repo path plus `BranchId`.

(`src/recency.rs:14-39`.) Keep these keys; translate focus records into them when the picker has completed enough discovery to know its visible entries.

Use this priority:

1. **Herdr worktree provenance (strongest).**
   - Match `worktree.repo_root` to the Kiosk repo entry and give that repo a focus rank.
   - Match `worktree.checkout_path` to the repo's discovered Git worktree paths. If that worktree has a local branch, also give `(repo_root, Local(branch))` the same focus rank.
   - This correctly handles a focused pane that has `cd`'d into a subdirectory or elsewhere: the Herdr workspace's worktree membership stays the identity.
   - If the checkout is detached/bare, absent from the latest Git listing, or its branch is unavailable, rank only the repo.

2. **Known checkout/path containment (best effort).**
   - For records without provenance, try `focused_pane_cwd`, then `workspace_cwd`.
   - First find the deepest known worktree path containing the cwd; this can recover repo + local branch.
   - Otherwise find the deepest visible repo or configured plain-folder entry containing the cwd and rank that repo/folder.
   - Use existing Kiosk canonicalization/equivalence rules for symlinks, missing paths, and platform case behavior.

3. **No safe match.**
   - Keep the observation until it is closed/pruned, but do not let it affect visible ordering.
   - Do not use `workspace_label` as identity. Labels are user-renamable and collide.

As with today's branch-success recording, a branch match should contribute both a branch rank and its containing repo rank (`src/recency.rs:163-166`).

Lossy/ambiguous cases are unavoidable on the public surface:

- A plain workspace has no stable public root cwd. The hook's `workspace_cwd` is normally the focused pane cwd, not a separately pinned creation path.
- A user can `cd` a plain workspace's focused pane into another repo, a sibling folder, or outside all Kiosk search roots.
- Different panes in one plain workspace can intentionally have unrelated cwds. The most recently focused pane is a reasonable “where I was” signal, but not an unambiguous workspace/project identity.
- A non-repo cwd outside configured Kiosk candidates cannot be shown.
- Focus provenance has checkout path but no branch. Branch recovery depends on the picker/Git worktree listing at read time.
- Remote-only branch entries cannot be the identity of a checked-out workspace; a checked-out branch maps to its local branch entry. Detached checkouts are repo-only.

These limits are mapping loss, not focus-history loss. The raw event still records the correct workspace and pane context.

### Coexistence with today's `recency.json`

Recommended first release: **layer, do not replace**.

1. Load and map `focus-history.json` into a deduplicated sequence of existing `RecencyKey`s, newest focus first.
2. Append keys from today's `recency.json` that were not already produced by focus history.
3. Use the resulting ranks only in recency mode.

This makes actual Herdr focus authoritative for live, mappable workspaces while retaining:

- the current behavior if event hooks were dropped by the 32-command cap;
- Kiosk-open history for closed workspaces;
- useful history on upgrade before enough real focus events have accumulated;
- a no-data-loss rollback path.

Kiosk opens will normally appear in both sources: the successful open remains in `recency.json`, and Herdr's resulting focus change appears in focus history. Dedup makes that harmless. After one or two releases, decide whether usage warrants consolidating both sources into a timestamped version-2 file.

This initial layer is intentionally not a perfect timestamp merge because v1 `recency.json` stores order but no timestamps (`src/recency.rs:41-45`). In steady state that does not matter because every successful focused Kiosk open also generates a real focus event. If reviewers require strict cross-source chronology even when a focus hook is missed, the implementation should instead migrate to a unified timestamped schema and define how legacy rank/file mtime becomes synthetic time.

Alphabetical purity remains trivial: the current alphabetical path ignores recency state entirely (`src/config/mod.rs:15-25`) and has explicit tests that it remains untouched (`src/app.rs:832-850`). The new history files must only be consulted when `SortOrder::Recency` is active.

One product decision remains: pruning a closed workspace removes its real-focus record, so an externally opened-and-closed repo will not stay recent unless it is also in Kiosk's own history. That matches the requested closed-workspace cleanup and avoids stale ids. If “recently focused even after close” is desired, split each observation into (a) a live workspace binding that is pruned and (b) a stable mapped target-history record that survives closure.

## Prior art

All links below are pinned to the researched commits.

### `beyondlex/herdr-recent-navigator`

This is direct proof that event-driven MRU works as a normal Herdr plugin:

- Its manifest subscribes to `workspace.focused`, `pane.focused`, and `tab.focused`, each launching a short-lived `track` subcommand: [manifest lines 45-59](https://github.com/beyondlex/herdr-recent-navigator/blob/6d9c7835184b46fdd8ca0394201136fb8060751a/herdr-plugin.toml#L45-L59).
- Its tracker keeps a move-to-front JSON list, caps it at 300, uses a cross-process lock, and performs temp-file + rename replacement: [tracker lines 35-129](https://github.com/beyondlex/herdr-recent-navigator/blob/6d9c7835184b46fdd8ca0394201136fb8060751a/src/tracker.rs#L35-L129).
- It documents and parses the underscored event-envelope form, then defensively polls current focus and records pane/tab/workspace together: [main lines 323-426](https://github.com/beyondlex/herdr-recent-navigator/blob/6d9c7835184b46fdd8ca0394201136fb8060751a/src/main.rs#L323-L426).

Useful lesson: multiple hooks for the same focus transition can run simultaneously, so locking is real, not theoretical. For Kiosk, only `workspace.focused` is needed, and context already contains the focused pane cwd, so the peer's three hooks and several IPC lookups would be unnecessary overhead. It also has no close hook; stale ids age out under the cap and are ignored when not present in the live node list.

### `nicolegros/herdr-launcher`

This is the closest structural prior art to the recommendation:

- It hooks exactly `workspace.focused` and `workspace.closed`: [manifest lines 31-37](https://github.com/nicolegros/herdr-launcher/blob/9bdc67b80fc212a132b4b86369902a46a8d2c5b5/herdr-plugin.toml#L31-L37).
- The focus handler moves a workspace id to the front; the close handler removes it: [event.go lines 9-89](https://github.com/nicolegros/herdr-launcher/blob/9bdc67b80fc212a132b4b86369902a46a8d2c5b5/event.go#L9-L89).
- It caps at 50, uses `flock`, and writes atomically: [history.go lines 11-103](https://github.com/nicolegros/herdr-launcher/blob/9bdc67b80fc212a132b4b86369902a46a8d2c5b5/history.go#L11-L103).

It proves the recommended lifecycle shape. Its state is workspace-id only, so it does not solve Kiosk's repo/branch/folder mapping.

### `salkhalil/herdr-sessionizer`

Sessionizer is the counterexample showing the current Kiosk limitation:

- It has no event hooks: [manifest](https://github.com/salkhalil/herdr-sessionizer/blob/218c87bfbfd36d5057a65f23394667c23ce359cb/herdr-plugin.toml).
- It reads `session.snapshot` to list current resources, but appends history only when Sessionizer itself focuses, creates, or jumps to an agent: [sessionizer lines 129-178 and 269-293](https://github.com/salkhalil/herdr-sessionizer/blob/218c87bfbfd36d5057a65f23394667c23ce359cb/bin/sessionizer#L129-L178).
- It keys a workspace by the first pane cwd and explicitly avoids an agent pane's subdirectory cwd: [sessionizer lines 282-293](https://github.com/salkhalil/herdr-sessionizer/blob/218c87bfbfd36d5057a65f23394667c23ce359cb/bin/sessionizer#L282-L293).

It therefore tracks only switches it performs, not arbitrary Herdr focus, and illustrates why a current snapshot cannot supply history.

### `marcoskichel/herdr-muster`

Muster is useful prior art for identity and cleanup, not MRU:

- It has no event hooks and no recency sort: [manifest](https://github.com/marcoskichel/herdr-muster/blob/9a1e42ed1d5dff2f6992a30993b17c1ce60cd05d/herdr-plugin.toml).
- For workspaces it creates, it stores `canonical project dir -> workspace_id`; on each run it reconciles that registry against live workspace ids: [registry lines 5-50](https://github.com/marcoskichel/herdr-muster/blob/9a1e42ed1d5dff2f6992a30993b17c1ce60cd05d/src/registry.rs#L5-L50).
- For foreign workspaces it falls back to the lowest-numbered/root pane cwd, and its ADR documents that this identity drifts after `cd`: [ADR](https://github.com/marcoskichel/herdr-muster/blob/9a1e42ed1d5dff2f6992a30993b17c1ce60cd05d/docs/adr/0001-workspace-identity-via-muster-registry.md).

Current Herdr worktree provenance gives Kiosk a stronger answer than Muster had for managed Git workspaces. The same ambiguity still applies to plain/foreign workspaces, and Muster's reconcile-on-open pattern is worth copying.

## Risks and mitigations

| Risk | Assessment / mitigation |
| --- | --- |
| Per-focus process spawn | Acceptable at human focus frequency if the native recorder does no CLI calls and exits immediately. Use only one focus hook. Measure median/p95 completion from Herdr plugin logs during a spike. |
| 32-command global cap | A dropped hook is possible during bursts or stuck plugins. Keep current own-open history as fallback; reconcile later; recorder must never linger. |
| Concurrent/corrupt writes | Exclusive cross-process lock, bounded file, same-directory atomic replacement, corruption recovery. Test on Windows, not only Unix. |
| Logical event reordering | Narrow race because events have no server timestamp/sequence. Accept for human usage initially; optionally verify current focus before commit or request a public event sequence upstream. |
| Closed-workspace cleanup | Handle `workspace.closed` immediately and reconcile live ids every picker run. Decide whether mapped target recency should survive closure. |
| Plain-workspace identity | Explicitly best effort. Prefer worktree provenance; then deepest checkout/repo/folder containment; never label identity. |
| Branch recovery | Focus context has checkout provenance, not branch. Join checkout path to Kiosk's current Git worktree listing; detached/unresolved becomes repo-only. |
| Named sessions | Namespace workspace ids by a hash of `HERDR_SOCKET_PATH`. Decide whether any path-level MRU should intentionally cross session boundaries. |
| Event allowlist/version | Required hooks are allowlisted in plugin v1/Herdr 0.7.0; Kiosk's 0.7.4 floor is sufficient. Avoid `[[startup]]` unless raising the floor to 0.7.5. |
| Hook feedback loop | Recorder performs only state-file I/O and emits no Herdr mutations, so it cannot recursively create focus events. |
| Privacy | State stores local paths and labels, as current recency already stores paths. Keep it local under the plugin state dir and document it. |

## Recommended decisions (Claude's steer, pending Tom's confirmation)

These answer the "Questions for human review" below. A fresh session may proceed on these
defaults unless Tom has redirected; each maps to the numbered question.

1. **Live workspaces only** to start — prune a mapped target when its workspace closes.
   Matches "recent = still around" and avoids stale ids. Revisit "stay recent after close"
   only if it's missed in real use.
2. **Scope MRU per named herdr session** (namespace by a hash of `HERDR_SOCKET_PATH`) — ids
   like `w1` collide across sessions. Do not make path-level MRU cross sessions initially.
3. **Plain workspace whose focused pane `cd`'d away → treat the current focused location as
   truth**, clearly best-effort (don't retain an earlier inferred identity).
4. **Detached/unresolved worktree → rank the repo only; do not guess a branch.**
5. **"Real focus wins" via simple layering** — focus-history ranks precede legacy own-open
   ranks; do NOT attempt strict cross-source chronological merging. Strict chronology needs
   a `recency.json` v2 timestamped migration that isn't worth it for v1.
6. **Rapid scripted focus loops: not a supported case** initially (human focus frequency
   only). Only pay the per-event snapshot check, or pursue an upstream event sequence, if
   this proves necessary.

Non-negotiable regardless: alphabetical mode stays pure (focus history read only in
`SortOrder::Recency`), and the recorder is a fast, best-effort, non-fatal one-shot that
never blocks or crashes herdr (a bad/locked/corrupt state file degrades to today's
own-opens behavior).

## Questions for human review

1. Should real-focus recency be limited to currently live workspaces (recommended first version), or should a mapped repo/folder remain recent after its workspace closes?
2. Is MRU scoped to the current named Herdr session, or should stable path-based recency merge across named sessions? Direct workspace ids cannot safely be global.
3. For a plain workspace whose focused pane `cd`s into another visible project, should Kiosk treat the focused pane location as truth, retain an earlier inferred workspace identity, or leave it unmapped? Recommended first version: current focused location, clearly best effort.
4. Is repo-only ranking acceptable for detached/unresolved worktrees? Recommended: yes; do not guess a branch.
5. Does “real focus wins” mean focus-history ranks always precede legacy own-open ranks, or is strict chronological merging across both sources required? The latter requires a v2 timestamp migration.
6. Are rapid scripted focus loops a supported use case? If yes, pay the extra snapshot check per event or pursue an upstream event sequence.

## Phased implementation outline

### Phase 0: contract/performance spike

- Link a throwaway local build with the two hooks.
- Capture representative `HERDR_PLUGIN_EVENT_JSON` and `HERDR_PLUGIN_CONTEXT_JSON` for managed main checkout, linked worktree, plain Git workspace, plain folder, pane `cd`, close, and named session.
- Confirm one `workspace.focused` hook fires for workspace, tab, and pane focus transitions on the supported Herdr floor.
- Measure child completion time and rapid-switch behavior from `plugin.log.list`.

Exit criterion: payload contract confirmed on macOS/Linux and at least a Windows CI/integration fixture; hot hook has no Herdr subprocess call.

### Phase 1: isolated recorder

- Add the `record-herdr-event` noninteractive subcommand and four manifest blocks.
- Implement versioned `focus-history.json`, socket-derived session scope, portable lock, atomic save, dedup, caps, corruption handling, focus insert, and close removal.
- Unit-test payload parsing and state transitions; process-level test concurrent writers.
- Keep current `recency.json` behavior untouched.

### Phase 2: picker mapping and layered consumption

- Extend the read model enough to retain workspace ids and full worktree provenance.
- Reconcile focus records against live workspace ids at picker startup.
- Resolve records to repo/folder/branch keys using the priority above after discovery/worktree enrichment.
- Build focus-first, own-open-second deduplicated ranks for both resting and fuzzy recency order.
- Add tests for managed worktrees, main checkout, linked branch, detached checkout, nested cwd, unrelated `cd`, non-repo folder, stale workspace, symlink/case behavior, and pure alphabetical mode.

### Phase 3: real-session verification

- Exercise opens/focuses from Herdr UI, Kiosk, each peer-style plugin route, workspace close, server restart/live handoff, named sessions, and fast repeated focus.
- Verify current-workspace exclusion/default selection still produces “jump to previous” rather than selecting the workspace already underneath the popup.
- Verify hooks stay quiet and do not create visible popup latency or command-log noise beyond expected records.

### Phase 4: policy/consolidation decision

- Decide closed-workspace retention and named-session policy from real usage.
- If layering is sufficient, keep the two files and document precedence.
- If strict chronology is required, design `recency.json` v2 with timestamped source-tagged records and an explicit, reversible migration from ordered v1 entries. Only then consider removing the old direct-write path.

