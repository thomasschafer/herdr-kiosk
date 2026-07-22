use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    config::keys::{Command, KeyChord, KeysConfig},
    keyboard::Action,
    state::{AppState, Mode},
};

pub fn resolve_action(key: KeyEvent, state: &AppState, keys: &KeysConfig) -> Option<Action> {
    let chord = KeyChord::from_event(key);
    if state.help_overlay.is_some() {
        if chord == KeyChord::new(KeyCode::Esc, KeyModifiers::NONE) {
            return Some(Action::CloseHelp);
        }
        if let Some(action) = keys
            .command_for(KeysConfig::mode_for(&state.mode), chord)
            .and_then(help_command_to_action)
        {
            return Some(action);
        }
        if let KeyCode::Char(character) = chord.code
            && chord.modifiers == KeyModifiers::NONE
            && (character.is_ascii_graphic() || character == ' ')
        {
            return Some(Action::Insert(character));
        }
        return None;
    }

    if !state.toasts.is_empty() && keys.is_dismiss_toast(chord) {
        return Some(Action::DismissToast);
    }

    let binding_mode = KeysConfig::mode_for(&state.mode);
    if matches!(
        state.mode,
        Mode::Loading { .. } | Mode::ValidatingNewBranch { .. }
    ) {
        return match keys.command_for(binding_mode, chord) {
            Some(Command::Quit) => Some(Action::Quit),
            _ => None,
        };
    }

    let query = active_query(state);
    if let Some(command) = keys.command_for(binding_mode, chord)
        && !(command == Command::Quit
            && chord.modifiers == KeyModifiers::NONE
            && matches!(chord.code, KeyCode::Char(_))
            && !query.is_empty())
    {
        return command_to_action(command, state);
    }
    if let KeyCode::Char(character) = chord.code
        && chord.modifiers == KeyModifiers::NONE
        && (character.is_ascii_graphic() || character == ' ')
        && matches!(
            state.mode,
            Mode::RepoSelect | Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. }
        )
    {
        return Some(Action::Insert(character));
    }
    None
}

fn help_command_to_action(command: Command) -> Option<Action> {
    match command {
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        Command::Clear => Some(Action::ClearQuery),
        Command::Backspace => Some(Action::Backspace),
        Command::DeleteWord => Some(Action::DeleteWord),
        Command::CursorLeft => Some(Action::CursorLeft),
        Command::CursorRight => Some(Action::CursorRight),
        Command::Noop
        | Command::Quit
        | Command::Help
        | Command::DismissToast
        | Command::Open
        | Command::BranchesView
        | Command::Back
        | Command::NewBranch
        | Command::Delete => None,
    }
}

fn command_to_action(command: Command, state: &AppState) -> Option<Action> {
    match command {
        Command::Noop => None,
        Command::Quit => Some(Action::Quit),
        Command::Help => Some(Action::ShowHelp),
        Command::DismissToast => (!state.toasts.is_empty()).then_some(Action::DismissToast),
        Command::MoveUp => Some(Action::MoveSelection(-1)),
        Command::MoveDown => Some(Action::MoveSelection(1)),
        Command::Open => match &state.mode {
            Mode::RepoSelect => Some(Action::OpenRepo),
            Mode::BranchSelect(_)
                if !active_query(state).is_empty() && state.branch_list.filtered.is_empty() =>
            {
                Some(Action::StartNewBranch)
            }
            Mode::BranchSelect(_) => Some(Action::OpenBranch),
            Mode::SelectBaseBranch { .. } => Some(Action::CreateNewBranch),
            Mode::ConfirmWorktreeDelete { target, .. } if !target.in_progress => {
                Some(Action::ConfirmDeleteWorktree)
            }
            Mode::Loading { .. }
            | Mode::ValidatingNewBranch { .. }
            | Mode::ConfirmWorktreeDelete { .. } => None,
        },
        Command::BranchesView => {
            matches!(state.mode, Mode::RepoSelect).then_some(Action::OpenBranches)
        }
        Command::Back => {
            if active_query(state).is_empty() {
                match state.mode {
                    Mode::BranchSelect(_) => Some(Action::BackToRepos),
                    Mode::SelectBaseBranch { .. } | Mode::ConfirmWorktreeDelete { .. } => {
                        Some(Action::CancelOverlay)
                    }
                    _ => None,
                }
            } else {
                Some(Action::ClearQuery)
            }
        }
        Command::NewBranch => {
            matches!(state.mode, Mode::BranchSelect(_)).then_some(Action::StartNewBranch)
        }
        Command::Delete => {
            matches!(state.mode, Mode::BranchSelect(_)).then_some(Action::DeleteWorktree)
        }
        Command::Clear => {
            if !active_query(state).is_empty() {
                Some(Action::ClearQuery)
            } else if matches!(state.mode, Mode::RepoSelect) {
                Some(Action::Quit)
            } else {
                None
            }
        }
        Command::Backspace => Some(Action::Backspace),
        Command::DeleteWord => Some(Action::DeleteWord),
        Command::CursorLeft => Some(Action::CursorLeft),
        Command::CursorRight => Some(Action::CursorRight),
    }
}

fn active_query(state: &AppState) -> &str {
    match &state.mode {
        Mode::RepoSelect => &state.repo_list.input.text,
        Mode::BranchSelect(_) => &state.branch_list.input.text,
        Mode::SelectBaseBranch { flow, .. } => &flow.list.input.text,
        Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmWorktreeDelete { .. } => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        BaseBranchSelection, BranchContext, DeleteWorktreeTarget, HelpBindingRow, HelpOverlayState,
        SearchableList,
    };

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn question_mark_remains_searchable_and_configured_help_opens_overlay() {
        let state = AppState::new(None);
        let keys = KeysConfig::default();
        assert_eq!(
            resolve_action(key(KeyCode::Char('?'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('?'))
        );
        assert_eq!(
            resolve_action(
                key(KeyCode::Char('h'), KeyModifiers::CONTROL),
                &state,
                &keys
            ),
            Some(Action::ShowHelp)
        );
    }

    #[test]
    fn q_default_is_contextual_and_can_be_unbound() {
        let mut state = AppState::new(None);
        let q = key(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(
            resolve_action(q, &state, &KeysConfig::default()),
            Some(Action::Quit)
        );
        state.repo_list.input.text = "a".into();
        assert_eq!(
            resolve_action(q, &state, &KeysConfig::default()),
            Some(Action::Insert('q'))
        );

        state.repo_list.input.clear();
        let keys = toml::from_str::<KeysConfig>("[repo_select]\nq = \"noop\"").unwrap();
        assert_eq!(resolve_action(q, &state, &keys), None);
    }

    #[test]
    fn toast_dismiss_has_precedence_over_branch_delete() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        let chord = key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(
            resolve_action(chord, &state, &KeysConfig::default()),
            Some(Action::DeleteWorktree)
        );
        state.push_toast(crate::state::ToastKind::Warning, "notice");
        assert_eq!(
            resolve_action(chord, &state, &KeysConfig::default()),
            Some(Action::DismissToast)
        );
    }

    #[test]
    fn existing_modal_defaults_are_preserved() {
        let context = BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        };
        let keys = KeysConfig::default();
        let mut state = AppState::new(None);
        state.mode = Mode::SelectBaseBranch {
            context: context.clone(),
            flow: BaseBranchSelection {
                new_name: "feat".into(),
                bases: vec!["main".into()],
                list: SearchableList::new(1),
            },
        };
        assert_eq!(
            resolve_action(key(KeyCode::Enter, KeyModifiers::NONE), &state, &keys),
            Some(Action::CreateNewBranch)
        );
        assert_eq!(
            resolve_action(key(KeyCode::Esc, KeyModifiers::NONE), &state, &keys),
            Some(Action::CancelOverlay)
        );
        state.mode = Mode::ConfirmWorktreeDelete {
            context,
            target: DeleteWorktreeTarget {
                branch_name: "feat".into(),
                worktree_path: "/wt".into(),
                open_workspace_id: None,
                force: false,
                in_progress: false,
            },
        };
        assert_eq!(
            resolve_action(key(KeyCode::Enter, KeyModifiers::NONE), &state, &keys),
            Some(Action::ConfirmDeleteWorktree)
        );
    }

    #[test]
    fn base_picker_resolves_all_search_and_navigation_actions() {
        let mut state = AppState::new(None);
        state.mode = Mode::SelectBaseBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            flow: BaseBranchSelection {
                new_name: "feat".into(),
                bases: vec!["main".into(), "next".into()],
                list: SearchableList::new(2),
            },
        };
        let keys = KeysConfig::default();
        for (code, modifiers, expected) in [
            (KeyCode::Up, KeyModifiers::NONE, Action::MoveSelection(-1)),
            (KeyCode::Down, KeyModifiers::NONE, Action::MoveSelection(1)),
            (
                KeyCode::Char('p'),
                KeyModifiers::CONTROL,
                Action::MoveSelection(-1),
            ),
            (
                KeyCode::Char('n'),
                KeyModifiers::CONTROL,
                Action::MoveSelection(1),
            ),
            (KeyCode::Backspace, KeyModifiers::NONE, Action::Backspace),
            (
                KeyCode::Char('w'),
                KeyModifiers::CONTROL,
                Action::DeleteWord,
            ),
            (KeyCode::Left, KeyModifiers::NONE, Action::CursorLeft),
            (KeyCode::Right, KeyModifiers::NONE, Action::CursorRight),
        ] {
            assert_eq!(
                resolve_action(key(code, modifiers), &state, &keys),
                Some(expected)
            );
        }
    }

    #[test]
    fn help_overlay_searches_without_triggering_underlying_mode_commands() {
        let mut state = AppState::new(None);
        state.help_overlay = Some(HelpOverlayState {
            rows: vec![HelpBindingRow {
                section_name: "general",
                key_display: "ctrl+h".into(),
                command_name: "help",
                description: "Show active key bindings",
            }],
            list: SearchableList::new(1),
        });
        let keys = KeysConfig::default();
        assert_eq!(
            resolve_action(key(KeyCode::Char('q'), KeyModifiers::NONE), &state, &keys),
            Some(Action::Insert('q'))
        );
        assert_eq!(
            resolve_action(key(KeyCode::Backspace, KeyModifiers::NONE), &state, &keys),
            Some(Action::Backspace)
        );
        assert_eq!(
            resolve_action(key(KeyCode::Down, KeyModifiers::NONE), &state, &keys),
            Some(Action::MoveSelection(1))
        );
        assert_eq!(
            resolve_action(key(KeyCode::Esc, KeyModifiers::NONE), &state, &keys),
            Some(Action::CloseHelp)
        );
    }
}
