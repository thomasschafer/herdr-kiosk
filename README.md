# herdr-kiosk

Fuzzy-find Git repositories and branches, then open them as Herdr workspaces and
worktrees.

![Demo of fuzzy-finding a repository and branch, then opening its Herdr worktree](media/preview.gif)

## Install

Install from GitHub:

```sh
herdr plugin install thomasschafer/herdr-kiosk
```

On Linux and macOS, the installer downloads the matching release binary, verifies
its SHA-256 checksum, and falls back to `cargo build --release` when necessary.
Windows builds from source with Cargo for now.

Herdr 0.7.4 does not render plugin actions in its menus. Keybindings are the only
way to surface the picker, so add this once to your Herdr config:

```toml
[[keys.command]]
key = "prefix+f"
type = "plugin_action"
command = "thomasschafer.herdr-kiosk.open-picker"
description = "open repo picker"
```

Reload Herdr's configuration:

```sh
herdr server reload-config
```

On first use, a setup wizard writes the directories and scan depth to the plugin's
`config.toml`. Locate it with:

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

<!-- CONFIG:START -->
User configuration loaded from the plugin's `config.toml`.

#### `search_dirs`

Directories searched recursively for Git repositories. Entries can be simple
strings such as `"~/Code"` or inline tables such as
`{ path = "~/Work", depth = 3 }`, and both forms can be mixed.

A repository search root, written either as a path string or an inline table.

Accepted forms:

- String form.
  - Directory to scan with the default depth of 1. `~` and paths beginning with `~/` expand from the user's home directory.
- A path with an optional per-directory scan depth.
  - `path` — Directory to scan. `~` and paths beginning with `~/` expand from the user's home directory; other relative paths are accepted as written.
  - `depth` — Maximum directory depth to scan. The value must be at least 1 and defaults to 1 when omitted.

### `[theme]`

Customize terminal-palette colors used by the picker.

Every field is optional and falls back to the default shown below.

Accepted colors are `black`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`, `dark_gray`, `reset`. Values use the terminal's ANSI palette, not RGB colors.

#### `accent`

Repository-picker accent, used for its active border and selected row.

Default: `"magenta"`

#### `secondary`

Branch and new-branch accent, used for active borders, selections, and
branch status markers.

Default: `"cyan"`

#### `tertiary`

Help-view accent, used for its border and selected row.

Default: `"green"`

#### `error`

Error notification title color.

Default: `"red"`

#### `warning`

Warning notification title and setup-warning color.

Default: `"yellow"`

#### `muted`

De-emphasized text color for metadata, counts, inactive content, and
explanatory labels. The untouched default becomes `gray` when a light
terminal background is detected.

Default: `"dark_gray"`

#### `border`

Inactive list-border color. The untouched default becomes `gray` when a
light terminal background is detected.

Default: `"dark_gray"`

#### `hint`

Keyboard-hint color in dialogs, setup, and notifications.

Default: `"blue"`

#### `highlight_fg`

Foreground color for text on selected-row accent backgrounds in every view.

Default: `"black"`

#### `open`

Color of the marker shown beside an open repository or worktree.

Default: `"green"`

### `[on_open]`

Configure command panes created after opening a new workspace.

The section is optional and contains no pane definitions by default.

#### `panes`

Pane definitions, created in order without moving focus from the primary
pane. Commands run from the opened repository or worktree. They run only
when a workspace is newly opened, not when an existing workspace is focused.

A command pane created after a new workspace is opened.

Each entry is an inline table with:

- `command` — Shell command Herdr runs in the opened checkout. The command must not be empty.
- `direction` — Split direction: `right` or `down`.
- `ratio` — Fraction of the resulting split occupied by the new command pane. The value must be greater than 0 and less than 1, and defaults to 0.5 when omitted.

### `[keys]`

Customize key bindings grouped by where they are active.

Layered key bindings for the picker and its dialogs. Keys use lowercase
`ctrl+`, `alt+`, and `shift+` modifiers with a character or a named key such as
`enter`, `esc`, `tab`, `backspace`, `delete`, an arrow name, `home`, `end`,
`pageup`, `pagedown`, or `space`. User bindings extend the defaults; assign
`"noop"` to unbind an inherited key.

#### `general`

Bindings active everywhere. Accepted actions are `quit`, `help`,
`dismiss_toast`, and `noop`.

#### `text_edit`

Bindings active while editing search or name text. Accepted actions are
`clear`, `backspace`, `delete_word`, `cursor_left`, `cursor_right`, and `noop`.

#### `list_navigation`

Bindings active in navigable lists. Accepted actions are `move_up`,
`move_down`, and `noop`.

#### `modal`

Bindings active in confirmation dialogs and the base-branch picker. Accepted
actions are `open`, `back`, and `noop`.

#### `repo_select`

Bindings specific to the repository picker. Accepted actions are `open`,
`branches_view`, `quit`, and `noop`.

#### `branch_select`

Bindings specific to the branch picker. Accepted actions are `open`, `back`,
`new_branch`, `delete`, and `noop`.

Defaults:

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

<!-- CONFIG:END -->

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

Build before linking because `herdr plugin link` does not run `[[build]]` commands:

```sh
cargo build --release
herdr plugin link .
```

`just link` combines these steps. `just e2e` runs the popup integration suite; it
uses a built Herdr checkout next to this repository unless `HERDR` points elsewhere.
Harness and manual-testing details are in
[`docs/VERIFYING.md`](docs/VERIFYING.md).
