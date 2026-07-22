use std::{collections::HashMap, fmt, str::FromStr};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Deserializer, Serialize};

use crate::state::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyChord {
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    pub fn from_event(event: KeyEvent) -> Self {
        let (code, mut modifiers) = if event.code == KeyCode::BackTab {
            (KeyCode::Tab, event.modifiers | KeyModifiers::SHIFT)
        } else {
            (event.code, event.modifiers)
        };
        let code = match code {
            KeyCode::Char(character)
                if modifiers.contains(KeyModifiers::SHIFT) && character.is_ascii_alphabetic() =>
            {
                KeyCode::Char(character.to_ascii_uppercase())
            }
            KeyCode::Char(character) => {
                modifiers.remove(KeyModifiers::SHIFT);
                KeyCode::Char(if character.is_ascii_alphabetic() {
                    character.to_ascii_lowercase()
                } else {
                    character
                })
            }
            code => code,
        };
        Self { code, modifiers }
    }
}

impl FromStr for KeyChord {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() {
            return Err("missing key chord".into());
        }
        let delimiter = value
            .split_once('+')
            .filter(|(first, _)| is_modifier_token(first))
            .map_or('-', |_| '+');
        let mut tokens = value.split(delimiter).collect::<Vec<_>>();
        let key = tokens.pop().ok_or("missing key code")?;
        let mut modifiers = KeyModifiers::NONE;
        for token in tokens {
            let modifier = match token.to_ascii_lowercase().as_str() {
                "c" | "ctrl" | "control" => KeyModifiers::CONTROL,
                "a" | "alt" | "m" => KeyModifiers::ALT,
                "s" | "shift" => KeyModifiers::SHIFT,
                _ => return Err(format!("invalid key modifier '{token}-'")),
            };
            if modifiers.contains(modifier) {
                return Err(format!("repeated key modifier '{token}-'"));
            }
            modifiers.insert(modifier);
        }
        let lower = key.to_ascii_lowercase();
        let code = match lower.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backspace" => KeyCode::Backspace,
            "del" | "delete" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "page_up" => KeyCode::PageUp,
            "pagedown" | "page_down" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            _ if key.chars().count() == 1 => {
                let mut character = key.chars().next().expect("one character");
                if modifiers.contains(KeyModifiers::SHIFT) {
                    if !character.is_ascii_alphabetic() {
                        return Err(format!(
                            "shift modifier cannot be used with non-letter key '{key}'; bind the resulting character directly"
                        ));
                    }
                    character = character.to_ascii_uppercase();
                } else if character.is_ascii_alphabetic()
                    || modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    character = character.to_ascii_lowercase();
                }
                KeyCode::Char(character)
            }
            _ => return Err(format!("invalid key code '{key}'")),
        };
        Ok(Self { code, modifiers })
    }
}

fn is_modifier_token(token: &str) -> bool {
    matches!(
        token.to_ascii_lowercase().as_str(),
        "c" | "ctrl" | "control" | "a" | "alt" | "m" | "s" | "shift"
    )
}

impl fmt::Display for KeyChord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers.contains(KeyModifiers::CONTROL) {
            formatter.write_str("ctrl+")?;
        }
        if self.modifiers.contains(KeyModifiers::ALT) {
            formatter.write_str("alt+")?;
        }
        if self.modifiers.contains(KeyModifiers::SHIFT) {
            formatter.write_str("shift+")?;
        }
        match self.code {
            KeyCode::Char(' ') => formatter.write_str("space"),
            KeyCode::Char(character) => write!(formatter, "{}", character.to_ascii_lowercase()),
            KeyCode::Enter => formatter.write_str("enter"),
            KeyCode::Esc => formatter.write_str("esc"),
            KeyCode::Tab => formatter.write_str("tab"),
            KeyCode::Backspace => formatter.write_str("backspace"),
            KeyCode::Delete => formatter.write_str("delete"),
            KeyCode::Up => formatter.write_str("↑"),
            KeyCode::Down => formatter.write_str("↓"),
            KeyCode::Left => formatter.write_str("←"),
            KeyCode::Right => formatter.write_str("→"),
            KeyCode::Home => formatter.write_str("home"),
            KeyCode::End => formatter.write_str("end"),
            KeyCode::PageUp => formatter.write_str("pageup"),
            KeyCode::PageDown => formatter.write_str("pagedown"),
            _ => formatter.write_str("unknown"),
        }
    }
}

impl Serialize for KeyChord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    Noop,
    Quit,
    Help,
    DismissToast,
    MoveUp,
    MoveDown,
    Open,
    BranchesView,
    Back,
    NewBranch,
    Delete,
    Clear,
    Backspace,
    DeleteWord,
    CursorLeft,
    CursorRight,
}

impl Command {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Noop => "noop",
            Self::Quit => "quit",
            Self::Help => "help",
            Self::DismissToast => "dismiss_toast",
            Self::MoveUp => "move_up",
            Self::MoveDown => "move_down",
            Self::Open => "open",
            Self::BranchesView => "branches_view",
            Self::Back => "back",
            Self::NewBranch => "new_branch",
            Self::Delete => "delete",
            Self::Clear => "clear",
            Self::Backspace => "backspace",
            Self::DeleteWord => "delete_word",
            Self::CursorLeft => "cursor_left",
            Self::CursorRight => "cursor_right",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::Noop => "Unbound",
            Self::Quit => "Quit the picker",
            Self::Help => "Show active key bindings",
            Self::DismissToast => "Dismiss the visible notification",
            Self::MoveUp => "Move selection up",
            Self::MoveDown => "Move selection down",
            Self::Open => "Open or confirm the selection",
            Self::BranchesView => "Browse repository branches",
            Self::Back => "Go back or cancel",
            Self::NewBranch => "Create a new branch",
            Self::Delete => "Delete the selected checkout",
            Self::Clear => "Clear the search query",
            Self::Backspace => "Delete the previous character",
            Self::DeleteWord => "Delete the previous word",
            Self::CursorLeft => "Move the cursor left",
            Self::CursorRight => "Move the cursor right",
        }
    }
}

impl FromStr for Command {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "noop" | "none" | "unbound" => Ok(Self::Noop),
            "quit" => Ok(Self::Quit),
            "help" | "show_help" => Ok(Self::Help),
            "dismiss_toast" => Ok(Self::DismissToast),
            "move_up" => Ok(Self::MoveUp),
            "move_down" => Ok(Self::MoveDown),
            "open" => Ok(Self::Open),
            "branches_view" | "enter_repo" => Ok(Self::BranchesView),
            "back" | "go_back" => Ok(Self::Back),
            "new_branch" => Ok(Self::NewBranch),
            "delete" | "delete_worktree" => Ok(Self::Delete),
            "clear" | "clear_query" => Ok(Self::Clear),
            "backspace" => Ok(Self::Backspace),
            "delete_word" => Ok(Self::DeleteWord),
            "cursor_left" => Ok(Self::CursorLeft),
            "cursor_right" => Ok(Self::CursorRight),
            _ => Err(format!("unknown key action '{value}'")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeysConfig {
    general: HashMap<KeyChord, Command>,
    text_edit: HashMap<KeyChord, Command>,
    list_navigation: HashMap<KeyChord, Command>,
    modal: HashMap<KeyChord, Command>,
    repo_select: HashMap<KeyChord, Command>,
    branch_select: HashMap<KeyChord, Command>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawKeysConfig {
    general: HashMap<String, String>,
    text_edit: HashMap<String, String>,
    list_navigation: HashMap<String, String>,
    modal: HashMap<String, String>,
    repo_select: HashMap<String, String>,
    branch_select: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingMode {
    Repo,
    Branch,
    BaseBranch,
    Modal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub name: &'static str,
    pub bindings: Vec<(KeyChord, Command)>,
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            general: map(&[
                ("C-c", Command::Quit),
                ("C-h", Command::Help),
                ("C-x", Command::DismissToast),
            ]),
            text_edit: map(&[
                ("esc", Command::Clear),
                ("backspace", Command::Backspace),
                ("A-backspace", Command::DeleteWord),
                ("C-w", Command::DeleteWord),
                ("left", Command::CursorLeft),
                ("right", Command::CursorRight),
            ]),
            list_navigation: map(&[
                ("up", Command::MoveUp),
                ("C-p", Command::MoveUp),
                ("down", Command::MoveDown),
                ("C-n", Command::MoveDown),
            ]),
            modal: map(&[("enter", Command::Open), ("esc", Command::Back)]),
            repo_select: map(&[
                ("enter", Command::Open),
                ("tab", Command::BranchesView),
                ("q", Command::Quit),
            ]),
            branch_select: map(&[
                ("enter", Command::Open),
                ("esc", Command::Back),
                ("C-o", Command::NewBranch),
                ("C-x", Command::Delete),
            ]),
        }
    }
}

fn map(entries: &[(&str, Command)]) -> HashMap<KeyChord, Command> {
    entries
        .iter()
        .map(|(key, command)| (key.parse().expect("valid default chord"), *command))
        .collect()
}

impl KeysConfig {
    fn from_raw(raw: RawKeysConfig) -> Result<Self, String> {
        let mut keys = Self::default();
        extend_layer(&mut keys.general, raw.general, GENERAL)?;
        extend_layer(&mut keys.text_edit, raw.text_edit, TEXT_EDIT)?;
        extend_layer(
            &mut keys.list_navigation,
            raw.list_navigation,
            LIST_NAVIGATION,
        )?;
        extend_layer(&mut keys.modal, raw.modal, MODAL)?;
        extend_layer(&mut keys.repo_select, raw.repo_select, REPO_SELECT)?;
        extend_layer(&mut keys.branch_select, raw.branch_select, BRANCH_SELECT)?;
        Ok(keys)
    }

    pub fn command_for(&self, mode: BindingMode, chord: KeyChord) -> Option<Command> {
        let mut command = self.general.get(&chord).copied();
        if !matches!(mode, BindingMode::Modal) {
            command = self.text_edit.get(&chord).copied().or(command);
            command = self.list_navigation.get(&chord).copied().or(command);
        }
        command = match mode {
            BindingMode::Repo => self.repo_select.get(&chord).copied().or(command),
            BindingMode::Branch => self.branch_select.get(&chord).copied().or(command),
            BindingMode::BaseBranch | BindingMode::Modal => {
                self.modal.get(&chord).copied().or(command)
            }
        };
        command
    }

    pub fn is_dismiss_toast(&self, chord: KeyChord) -> bool {
        self.general.get(&chord) == Some(&Command::DismissToast)
    }

    pub fn dismiss_toast_key(&self) -> Option<KeyChord> {
        self.general
            .iter()
            .filter_map(|(key, command)| (*command == Command::DismissToast).then_some(*key))
            .min_by_key(ToString::to_string)
    }

    pub fn mode_for(mode: &Mode) -> BindingMode {
        match mode {
            Mode::RepoSelect => BindingMode::Repo,
            Mode::BranchSelect(_) => BindingMode::Branch,
            Mode::SelectBaseBranch { .. } => BindingMode::BaseBranch,
            Mode::ValidatingNewBranch { .. }
            | Mode::ConfirmWorktreeDelete { .. }
            | Mode::Loading { .. } => BindingMode::Modal,
        }
    }

    pub fn first_key(&self, mode: BindingMode, wanted: Command) -> Option<KeyChord> {
        self.sections(mode)
            .into_iter()
            .flat_map(|section| section.bindings)
            .find_map(|(key, command)| (command == wanted).then_some(key))
    }

    pub fn sections(&self, mode: BindingMode) -> Vec<HelpSection> {
        let effective = |key| self.command_for(mode, key);
        let mut sections = Vec::new();
        let layers: Vec<(&'static str, &HashMap<KeyChord, Command>)> = match mode {
            BindingMode::Repo => vec![
                ("repository", &self.repo_select),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("general", &self.general),
            ],
            BindingMode::Branch => vec![
                ("branches", &self.branch_select),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("general", &self.general),
            ],
            BindingMode::BaseBranch => vec![
                ("base branch", &self.modal),
                ("navigation", &self.list_navigation),
                ("search", &self.text_edit),
                ("general", &self.general),
            ],
            BindingMode::Modal => vec![("dialog", &self.modal), ("general", &self.general)],
        };
        for (name, layer) in layers {
            let mut bindings = layer
                .iter()
                .filter_map(|(key, command)| {
                    (*command != Command::Noop && effective(*key) == Some(*command))
                        .then_some((*key, *command))
                })
                .collect::<Vec<_>>();
            bindings.sort_by(|left, right| {
                command_rank(left.1)
                    .cmp(&command_rank(right.1))
                    .then_with(|| left.0.to_string().cmp(&right.0.to_string()))
            });
            if !bindings.is_empty() {
                sections.push(HelpSection { name, bindings });
            }
        }
        sections
    }
}

const GENERAL: &[Command] = &[
    Command::Noop,
    Command::Quit,
    Command::Help,
    Command::DismissToast,
];
const TEXT_EDIT: &[Command] = &[
    Command::Noop,
    Command::Clear,
    Command::Backspace,
    Command::DeleteWord,
    Command::CursorLeft,
    Command::CursorRight,
];
const LIST_NAVIGATION: &[Command] = &[Command::Noop, Command::MoveUp, Command::MoveDown];
const MODAL: &[Command] = &[Command::Noop, Command::Open, Command::Back];
const REPO_SELECT: &[Command] = &[
    Command::Noop,
    Command::Open,
    Command::BranchesView,
    Command::Quit,
];
const BRANCH_SELECT: &[Command] = &[
    Command::Noop,
    Command::Open,
    Command::Back,
    Command::NewBranch,
    Command::Delete,
];

fn extend_layer(
    layer: &mut HashMap<KeyChord, Command>,
    raw: HashMap<String, String>,
    allowed: &[Command],
) -> Result<(), String> {
    for (key, action) in raw {
        let chord = key
            .parse::<KeyChord>()
            .map_err(|error| format!("invalid key chord '{key}': {error}"))?;
        let command = action.parse::<Command>()?;
        if !allowed.contains(&command) {
            return Err(format!(
                "key action '{action}' is not allowed in this keys section"
            ));
        }
        layer.insert(chord, command);
    }
    Ok(())
}

const fn command_rank(command: Command) -> u8 {
    match command {
        Command::Open => 0,
        Command::BranchesView => 1,
        Command::NewBranch => 2,
        Command::Delete => 3,
        Command::Back => 4,
        Command::MoveUp => 5,
        Command::MoveDown => 6,
        Command::Clear => 7,
        Command::Backspace => 8,
        Command::DeleteWord => 9,
        Command::CursorLeft => 10,
        Command::CursorRight => 11,
        Command::Help => 12,
        Command::DismissToast => 13,
        Command::Quit => 14,
        Command::Noop => 15,
    }
}

impl<'de> Deserialize<'de> for KeysConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawKeysConfig::deserialize(deserializer)?;
        Self::from_raw(raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_displays_valid_chords() {
        assert_eq!("C-b".parse::<KeyChord>().unwrap().to_string(), "ctrl+b");
        assert_eq!(
            "Alt-backspace".parse::<KeyChord>().unwrap().to_string(),
            "alt+backspace"
        );
        assert_eq!(
            "shift-tab".parse::<KeyChord>().unwrap().to_string(),
            "shift+tab"
        );
        assert_eq!("space".parse::<KeyChord>().unwrap().to_string(), "space");
        let shifted = "S-a".parse::<KeyChord>().unwrap();
        assert_eq!(shifted.to_string(), "shift+a");
        assert_eq!(
            shifted,
            KeyChord::from_event(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT))
        );
        assert_eq!(shifted.to_string().parse::<KeyChord>().unwrap(), shifted);
        assert_eq!("A".parse::<KeyChord>().unwrap(), "a".parse().unwrap());
    }

    #[test]
    fn rejects_invalid_chords_and_unknown_actions() {
        let bad_chord =
            toml::from_str::<KeysConfig>("[branch_select]\n\"Hyper-Nope\" = \"new_branch\"")
                .unwrap_err()
                .to_string();
        assert!(bad_chord.contains("invalid key chord"));
        let bad_action =
            toml::from_str::<KeysConfig>("[branch_select]\n\"C-b\" = \"launch_missiles\"")
                .unwrap_err()
                .to_string();
        assert!(bad_action.contains("unknown key action"));
        let shifted_number =
            toml::from_str::<KeysConfig>("[repo_select]\n\"S-1\" = \"branches_view\"")
                .unwrap_err()
                .to_string();
        assert!(shifted_number.contains("bind the resulting character directly"));
    }

    #[test]
    fn user_keys_merge_with_defaults_and_override_matching_chords() {
        let keys = toml::from_str::<KeysConfig>(
            "[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"",
        )
        .unwrap();
        assert_eq!(
            keys.command_for(BindingMode::Branch, "C-b".parse().unwrap()),
            Some(Command::NewBranch)
        );
        assert_eq!(
            keys.command_for(BindingMode::Branch, "enter".parse().unwrap()),
            Some(Command::Open)
        );
        assert_eq!(
            keys.command_for(BindingMode::Branch, "C-o".parse().unwrap()),
            Some(Command::Noop)
        );
    }

    #[test]
    fn help_rows_are_derived_from_overridden_bindings() {
        let keys = toml::from_str::<KeysConfig>(
            "[branch_select]\n\"C-b\" = \"new_branch\"\n\"C-o\" = \"noop\"",
        )
        .unwrap();
        let rendered = keys
            .sections(BindingMode::Branch)
            .into_iter()
            .flat_map(|section| section.bindings)
            .map(|(key, command)| format!("{key} {command:?}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("ctrl+b NewBranch"));
        assert!(!rendered.contains("ctrl+o NewBranch"));
    }

    #[test]
    fn base_branch_mode_layers_search_navigation_and_dialog_bindings() {
        let keys = KeysConfig::default();
        for (chord, command) in [
            ("up", Command::MoveUp),
            ("down", Command::MoveDown),
            ("C-p", Command::MoveUp),
            ("C-n", Command::MoveDown),
            ("backspace", Command::Backspace),
            ("C-w", Command::DeleteWord),
            ("left", Command::CursorLeft),
            ("right", Command::CursorRight),
            ("enter", Command::Open),
            ("esc", Command::Back),
        ] {
            assert_eq!(
                keys.command_for(BindingMode::BaseBranch, chord.parse().unwrap()),
                Some(command),
                "unexpected base-branch command for {chord}"
            );
        }
    }

    #[test]
    fn delete_dialog_remains_modal() {
        let keys = KeysConfig::default();
        assert_eq!(
            keys.command_for(BindingMode::Modal, "backspace".parse().unwrap()),
            None
        );
        assert_eq!(
            keys.command_for(BindingMode::Modal, "down".parse().unwrap()),
            None
        );
    }
}
