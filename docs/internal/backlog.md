# Backlog and future work

Informal, non-committed list of things we might do next, deliberately parked, or want to
remember. Not a promise or a roadmap — revisit as needed. For the architecture and the
"why" behind decisions, see [`../PLAN.md`](../PLAN.md). For the working process (Codex
implements, Claude reviews, PRs into a protected `main`, the full gate suite), see the
project memory and `PLAN.md`.

Convention: keep this current — move an item to a PR/commit when it's done, delete it when
it's no longer worth doing.

## Upstream asks (herdr)

Things that would be cleaner if herdr exposed them. Captured here rather than filed as
issues on herdr for now; raise upstream when we decide to.

- **Expose the resolved theme palette to plugins.** herdr does not currently expose its
  active/resolved UI palette to plugin processes — not via `HERDR_PLUGIN_CONTEXT_JSON`, an
  env var, or any CLI/socket call (verified against herdr source; the palette is computed
  only in app state). So the picker uses terminal default fg/bg + ANSI-16, which is the
  correct and least-brittle option today but cannot match herdr's sidebar theme exactly.
  The clean fix is upstream: a read-only contract carrying the *resolved* `theme_name` +
  token palette (in the plugin context JSON, a `ui.theme.get` call, or `session.snapshot`,
  ideally with a `theme.updated` event). "Resolved" matters — exposing only the config name
  leaves `auto_switch` and `[theme.custom]` evaluation to every plugin. Until then, do NOT
  mirror herdr's config/palette locally (it drifts and can't resolve auto-switch); exact
  parity is only feasible today when herdr itself uses its built-in `terminal` theme.
- **(Only if we pursue scripted-focus MRU)** a monotonic event sequence/timestamp on focus
  events, so a focus recorder can order rapid programmatic focus changes deterministically.
  See the MRU plan's risks. Not needed for human-frequency use.

## Parked features

Discussed and worth considering; not scheduled.

- **Real focus-driven MRU.** Upgrade recency from "kiosk's own opens" to the user's actual
  herdr focus history (one-shot `workspace.focused`/`workspace.closed` hooks → a bounded
  focus-history file → layered over own-opens recency). Full feasibility verdict and phased
  plan: `docs/internal/herdr-driven-mru-plan.md` (lands with the recency PR). Verdict:
  feasible, medium effort.
- **Headless "jump to previous workspace" action.** A manifest action + `record`-style
  subcommand that focuses the previous workspace with one keystroke, no picker. The
  in-picker previous-selection (recency mode) covers the core need; this is the faster
  power-user version. Deferred from the recency PR.
- **`herdr api snapshot` for open-state.** Replace the separate `workspace list` +
  `pane list` calls that compute open indicators with a single `session.snapshot` call
  (one round-trip, race-free point-in-time view). Marginal perf/consistency win; verify the
  snapshot returns worktree `repo_root` + pane `cwd` first. No natural home in the current
  feature PRs (none expanded open-state reads).
- **Lazy preview panel.** A repo/branch preview (path, open/dirty/ahead-behind, last
  commit, maybe README), started only for the selected row, debounced/cancellable, never
  delaying streaming discovery. Parked — nice, not essential, and must not hurt snappiness.
- **on_open per-repo overrides keyed by path/glob.** Overrides are currently keyed by exact
  repo name (applies to all repos sharing that name). Path- or glob-keying would
  disambiguate; add if name-keying proves limiting.
- **Anchored split trees in on_open.** Splits currently chain off the previous pane. Named
  split-anchors (for L-shaped layouts) could come later if anyone needs them.

## Deferred fixes and watch-items

Low priority; mostly "revisit if it bites / if reports come in." Carried over from the
plan's risk list.

- **Same-name + same-branch collision.** Two repos sharing a name whose default checkout
  path also collides make herdr's `git worktree add` fail on the existing directory; we
  surface a clean error toast. Watch item: request upstream a way to pass an explicit
  checkout path/suffix (herdr exposes `worktrees.directory` through no API/CLI today).
- **`worktree create` blocks the CLI call.** Very slow filesystems/repos can make the
  Loading state linger. Acceptable for now; revisit if it bites.
- **Fetch-on-open + interactive SSH auth.** Background multi-remote fetch can hang on
  interactive SSH prompts; mitigated by running in the background with a spinner and
  `GIT_TERMINAL_PROMPT=0`. Revisit if reports come in.
- **`ctrl+c` semantics, detached-worktree handling, orphan cleanup.** Minor behaviours
  deferred by decision during the build; revisit only if they surface in real use.
- **Re-verify herdr assumptions on version bumps.** herdr moves fast; when raising the
  `min_herdr_version` floor or bumping the vendored herdr, re-check the behavioural
  assumptions (event payloads, CLI shapes, popup behaviour) — several source-verified
  corrections in `PLAN.md` exist because behaviour changed between releases.

## Docs and release

- **CHANGELOG.** Start one at the next release (the recency/pins/filter/folder-binding/
  on_open work is a clean v0.2.0) so users can see what changed.
- **Release flow** is documented in the project memory (tag → `release.yml` → verify
  checksums → install rehearsal → `gh repo edit`). Cut a tagged release once the feature
  PRs merge and hand-testing passes.
