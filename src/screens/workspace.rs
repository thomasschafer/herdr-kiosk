use std::{
    cmp::Reverse,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{self as ct_event, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::DefaultTerminal;

use crate::{
    components,
    config::keys::{BindingMode, Command, KeyChord, KeysConfig},
    herdr::{HerdrProvider, WorkspaceInfo},
    state::SearchableList,
    theme::Theme,
};

#[cfg(test)]
mod tests;

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Quit,
    Focused,
}

/// One selectable row in the workspace picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub workspace_id: String,
    pub name: String,
    pub branch: Option<String>,
    pub agent_status: Option<String>,
    pub is_current: bool,
    search_text: String,
}

impl WorkspaceEntry {
    fn from_info(info: WorkspaceInfo) -> Self {
        let name = info
            .worktree
            .as_ref()
            .map_or_else(|| info.label.clone(), |worktree| worktree.repo_name.clone());
        let mut search_text = name.clone();
        if let Some(branch) = &info.branch {
            search_text.push(' ');
            search_text.push_str(branch);
        }
        // Labels are user-renamable, so keep them searchable when they add
        // information beyond the repo name.
        if info.label != name {
            search_text.push(' ');
            search_text.push_str(&info.label);
        }
        Self {
            workspace_id: info.workspace_id,
            name,
            branch: info.branch,
            agent_status: info.agent_status,
            is_current: info.focused,
            search_text,
        }
    }
}

/// Order workspaces for the picker: the focused workspace first, then most
/// recently focused, with unstamped workspaces last in herdr's own order.
pub fn order_workspaces(mut workspaces: Vec<WorkspaceInfo>) -> Vec<WorkspaceInfo> {
    workspaces
        .sort_by_key(|workspace| (!workspace.focused, Reverse(workspace.last_focused_unix_ms)));
    workspaces
}

pub struct WorkspacePickerState {
    pub entries: Vec<WorkspaceEntry>,
    pub list: SearchableList,
    pub loading: bool,
    pub error: Option<String>,
}

impl WorkspacePickerState {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            list: SearchableList::new(0),
            loading: true,
            error: None,
        }
    }

    pub fn load(&mut self, workspaces: Vec<WorkspaceInfo>) {
        self.entries = order_workspaces(workspaces)
            .into_iter()
            .map(WorkspaceEntry::from_info)
            .collect();
        self.list = SearchableList::new(self.entries.len());
        self.apply_default_selection();
        self.loading = false;
    }

    /// Start the cursor on the previous workspace so a plain `enter` toggles
    /// back to it; the current workspace still renders on top for orientation.
    fn apply_default_selection(&mut self) {
        if self.list.filtered.len() > 1 {
            self.list.selected = Some(1);
        }
    }

    pub fn refilter(&mut self, matcher: &SkimMatcherV2) {
        let texts: Vec<&str> = self
            .entries
            .iter()
            .map(|entry| entry.search_text.as_str())
            .collect();
        self.list.filtered = crate::app::fuzzy_rank(&self.list.input.text, &texts, matcher);
        self.list.selected = (!self.list.filtered.is_empty()).then_some(0);
        self.list.scroll_offset = 0;
        if self.list.input.text.is_empty() {
            self.apply_default_selection();
        }
    }

    pub fn selected_entry(&self) -> Option<&WorkspaceEntry> {
        self.list
            .selected
            .and_then(|selected| self.list.filtered.get(selected))
            .and_then(|(index, _)| self.entries.get(*index))
    }
}

impl Default for WorkspacePickerState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerAction {
    Quit,
    Move(i32),
    Insert(char),
    Backspace,
    DeleteWord,
    CursorLeft,
    CursorRight,
    Clear,
    Open,
    Noop,
}

/// Resolve a key against the repo-picker bindings, which carry every command
/// the workspace picker needs while honoring the user's remaps.
pub(crate) fn resolve_action(key: KeyEvent, query: &str, keys: &KeysConfig) -> PickerAction {
    let chord = KeyChord::from_event(key);
    let command = keys.command_for(BindingMode::Repo, chord);
    // A bare-letter quit binding (default `q`) must stay typeable once a
    // query is active, mirroring the repo picker.
    let quit_becomes_text = command == Some(Command::Quit)
        && chord.modifiers == KeyModifiers::NONE
        && matches!(chord.code, KeyCode::Char(_))
        && !query.is_empty();
    if let Some(command) = command.filter(|_| !quit_becomes_text) {
        return match command {
            Command::Quit => PickerAction::Quit,
            Command::MoveUp => PickerAction::Move(-1),
            Command::MoveDown => PickerAction::Move(1),
            Command::Open => PickerAction::Open,
            Command::Clear => {
                if query.is_empty() {
                    PickerAction::Quit
                } else {
                    PickerAction::Clear
                }
            }
            Command::Backspace => PickerAction::Backspace,
            Command::DeleteWord => PickerAction::DeleteWord,
            Command::CursorLeft => PickerAction::CursorLeft,
            Command::CursorRight => PickerAction::CursorRight,
            // Repo-view commands with no workspace-picker equivalent swallow
            // the key instead of inserting text.
            Command::Noop
            | Command::Help
            | Command::DismissToast
            | Command::BranchesView
            | Command::Back
            | Command::NewBranch
            | Command::Delete => PickerAction::Noop,
        };
    }
    if let KeyCode::Char(character) = chord.code
        && !chord
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        && !character.is_control()
    {
        return PickerAction::Insert(character);
    }
    PickerAction::Noop
}

/// Focus the selected workspace. Returns the outcome to exit with, or `None`
/// to keep the picker open (nothing selected, or the focus call failed and the
/// error is now displayed).
pub(crate) fn focus_selected(
    state: &mut WorkspacePickerState,
    herdr: &dyn HerdrProvider,
) -> Option<RunOutcome> {
    let entry = state.selected_entry()?;
    match herdr.workspace_focus(&entry.workspace_id) {
        Ok(()) => Some(RunOutcome::Focused),
        Err(error) => {
            state.error = Some(format!("could not focus workspace: {error}"));
            None
        }
    }
}

pub fn run(
    terminal: &mut DefaultTerminal,
    herdr: &Arc<dyn HerdrProvider>,
    theme: &Theme,
    keys: &KeysConfig,
) -> Result<RunOutcome> {
    let (sender, receiver) = mpsc::channel();
    {
        let provider = Arc::clone(herdr);
        thread::spawn(move || {
            let _ = sender.send(provider.workspace_list());
        });
    }
    let matcher = SkimMatcherV2::default();
    let mut state = WorkspacePickerState::new();
    let spinner_start = Instant::now();
    loop {
        terminal.draw(|frame| {
            components::workspace_list::draw(frame, &mut state, theme, keys, spinner_start);
        })?;
        if let Ok(result) = receiver.try_recv() {
            match result {
                Ok(workspaces) => state.load(workspaces),
                Err(error) => {
                    state.loading = false;
                    state.error = Some(format!("could not list workspaces: {error}"));
                }
            }
        }
        if !ct_event::poll(EVENT_POLL_INTERVAL)? {
            continue;
        }
        let Event::Key(key) = ct_event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match resolve_action(key, &state.list.input.text, keys) {
            PickerAction::Quit => return Ok(RunOutcome::Quit),
            PickerAction::Move(delta) => state.list.move_selection(delta),
            PickerAction::Insert(character) => {
                state.list.input.insert_char(character);
                state.refilter(&matcher);
            }
            PickerAction::Backspace => {
                state.list.input.backspace();
                state.refilter(&matcher);
            }
            PickerAction::DeleteWord => {
                state.list.input.delete_word();
                state.refilter(&matcher);
            }
            PickerAction::CursorLeft => state.list.input.cursor_left(),
            PickerAction::CursorRight => state.list.input.cursor_right(),
            PickerAction::Clear => {
                state.list.input.clear();
                state.refilter(&matcher);
            }
            PickerAction::Open => {
                if let Some(outcome) = focus_selected(&mut state, herdr.as_ref()) {
                    return Ok(outcome);
                }
            }
            PickerAction::Noop => {}
        }
    }
}
