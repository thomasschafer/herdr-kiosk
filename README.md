# herdr-kiosk

Fuzzy-find Git repositories and branches, then open them as Herdr workspaces and
worktrees.

![Demo of fuzzy-finding a repository and branch, then opening its Herdr worktree](media/preview.gif)

## Install

Install from the public GitHub repository:

```sh
herdr plugin install thomasschafer/herdr-kiosk
```

On Linux and macOS, installation downloads the version-matched release binary,
verifies its SHA-256 checksum, and falls back to `cargo build --release` if the
download cannot be used. Windows builds from source with Cargo for now.

Herdr 0.7.4 does not render plugin actions in its menus. Keybindings are the only
way to surface the picker, so add this once to your Herdr config:

```toml
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "thomasschafer.herdr-kiosk.open-picker"
description = "open repo picker"
```

Then reload the configuration:

```sh
herdr server reload-config
```

The first time the picker opens, a setup wizard asks for the directories to scan
and the scan depth. It writes the plugin's `config.toml`; find its directory at any
time with:

```sh
herdr plugin config-dir thomasschafer.herdr-kiosk
```

Herdr v1 has no `plugin update` command. To refresh the plugin, reinstall it from
GitHub.

## Usage

Type to fuzzy-search repositories and branches. `enter` opens the selected checkout,
`tab` opens the selected repository's branch view, and `ctrl+h` shows all active
bindings for the current view.

## Configuration

`search_dirs` accepts simple paths, paths with a per-directory depth, or both.
Simple paths use depth `1`; `~` and `~/...` are expanded from your home directory.

```toml
search_dirs = [
  "~/Code",
  { path = "~/Work", depth = 3 },
  "/opt/company/repos",
]
```

Optional `on_open` panes are created in order after any workspace is opened or
created. Each pane runs its command from the checkout directory without taking
focus from the primary pane. `direction` supports `right` or `down`. `ratio` is the
fraction of the split occupied by the new pane that runs the command, defaults to
`0.5`, and must be greater than `0` and less than `1`.

```toml
[on_open]
panes = [
  { command = "hx", direction = "right" },
  { command = "cargo test", direction = "down", ratio = 0.35 },
]
```

### `[keys]` section

Bindings are configured under `[keys.<section>]`; defaults are shown below, and to
unbind an inherited mapping, assign it to `noop`.

<!-- KEYS:START -->
```toml
[keys.general]
"ctrl+c" = "quit"
"ctrl+h" = "help"
"ctrl+x" = "dismiss_toast"

[keys.text_edit]
"alt+backspace" = "delete_word"
"backspace" = "backspace"
"ctrl+w" = "delete_word"
"esc" = "clear"
"left" = "cursor_left"
"right" = "cursor_right"

[keys.list_navigation]
"ctrl+n" = "move_down"
"ctrl+p" = "move_up"
"down" = "move_down"
"up" = "move_up"

[keys.modal]
"enter" = "open"
"esc" = "back"

[keys.repo_select]
"enter" = "open"
"q" = "quit"
"tab" = "branches_view"

[keys.branch_select]
"ctrl+o" = "new_branch"
"ctrl+x" = "delete"
"enter" = "open"
"esc" = "back"

```
<!-- KEYS:END -->

Chords use the lowercase `ctrl+`, `alt+`, and `shift+` modifier syntax.

For example, this moves `new_branch` to `ctrl+b` and unbinds its inherited key:

```toml
[keys.branch_select]
"ctrl+b" = "new_branch"
"ctrl+o" = "noop"
```

The theme uses terminal palette colors rather than RGB values. Every field is
optional; these are the defaults:

```toml
[theme]
accent = "magenta"
secondary = "cyan"
tertiary = "green"
error = "red"
warning = "yellow"
muted = "dark_gray"
border = "dark_gray"
hint = "blue"
highlight_fg = "black"
open = "green"
```

`accent` identifies the repository picker, `secondary` identifies branch and
new-branch flows, and `tertiary` identifies help.

Accepted colors are `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`,
`white`, `gray`, `dark_gray`, and `reset`. On a detected light terminal background,
the untouched `muted` and `border` defaults are adjusted from `dark_gray` to `gray`.

## Windows support

Windows is supported and uses PowerShell launch shims plus a native
`x86_64-pc-windows-msvc` binary. Installation currently needs Rust and Cargo because
the PowerShell fetch path is not implemented yet.

Automated Windows CI covers formatting, compilation, clippy, tests, and PowerShell
syntax. Before relying on it in a critical workflow, hand-test popup opening and
install/link paths, drive-letter and UNC search paths, Git-for-Windows error text,
remote authentication with prompts disabled, and linked worktree creation/deletion.
Herdr's verbatim `\\?\` and `\\?\UNC\` plugin paths are normalized by the launchers,
but those paths remain part of the manual release check.

## Trust and security

Herdr does not sandbox or review plugins: their build and runtime commands run as
your user with your environment and full Herdr CLI access. During installation,
this plugin downloads and verifies a release binary or runs Cargo. At runtime it
executes `git` to inspect repositories and branches and the `herdr` CLI to open,
focus, create, and remove Herdr worktrees and workspaces. Review the manifest,
scripts, and source before installing if that access is not acceptable.

## Development

Build the release binary before linking the working tree. `herdr plugin link` does
not run `[[build]]` commands.

```sh
cargo build --release
herdr plugin link /path/to/herdr-kiosk
```

`just link` combines those steps. Run the full popup integration suite with
`just e2e`; it needs a built Herdr checkout next to this repository unless `HERDR`
points elsewhere. The harness design and manual-driving details are in
[`docs/VERIFYING.md`](docs/VERIFYING.md).
