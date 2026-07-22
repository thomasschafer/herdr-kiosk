use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{keyboard::Action, state::AppState};

pub fn resolve_action(key: KeyEvent, state: &AppState) -> Option<Action> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if control && key.code == KeyCode::Char('c') {
        return Some(Action::Quit);
    }
    if control && key.code == KeyCode::Char('x') && !state.toasts.is_empty() {
        return Some(Action::DismissToast);
    }
    if matches!(
        state.mode,
        crate::state::Mode::Loading { .. } | crate::state::Mode::ValidatingNewBranch { .. }
    ) {
        return None;
    }

    if let crate::state::Mode::ConfirmWorktreeDelete { target, .. } = &state.mode {
        if target.in_progress {
            return None;
        }
        return match key.code {
            KeyCode::Enter => Some(Action::ConfirmDeleteWorktree),
            KeyCode::Esc => Some(Action::CancelOverlay),
            _ => None,
        };
    }

    let query = match &state.mode {
        crate::state::Mode::RepoSelect => &state.repo_list.input.text,
        crate::state::Mode::BranchSelect(_) => &state.branch_list.input.text,
        crate::state::Mode::SelectBaseBranch { flow, .. } => &flow.list.input.text,
        crate::state::Mode::Loading { .. }
        | crate::state::Mode::ValidatingNewBranch { .. }
        | crate::state::Mode::ConfirmWorktreeDelete { .. } => unreachable!("handled above"),
    };

    match key.code {
        KeyCode::Up | KeyCode::Char('p') if control => Some(Action::MoveSelection(-1)),
        KeyCode::Down | KeyCode::Char('n') if control => Some(Action::MoveSelection(1)),
        KeyCode::Up => Some(Action::MoveSelection(-1)),
        KeyCode::Down => Some(Action::MoveSelection(1)),
        KeyCode::Enter if matches!(state.mode, crate::state::Mode::RepoSelect) => {
            Some(Action::OpenRepo)
        }
        KeyCode::Enter if matches!(state.mode, crate::state::Mode::SelectBaseBranch { .. }) => {
            Some(Action::CreateNewBranch)
        }
        KeyCode::Enter
            if matches!(state.mode, crate::state::Mode::BranchSelect(_))
                && !query.is_empty()
                && state.branch_list.filtered.is_empty() =>
        {
            Some(Action::StartNewBranch)
        }
        KeyCode::Enter => Some(Action::OpenBranch),
        KeyCode::Char('o')
            if control && matches!(state.mode, crate::state::Mode::BranchSelect(_)) =>
        {
            Some(Action::StartNewBranch)
        }
        KeyCode::Char('x')
            if control && matches!(state.mode, crate::state::Mode::BranchSelect(_)) =>
        {
            Some(Action::DeleteWorktree)
        }
        KeyCode::Tab if matches!(state.mode, crate::state::Mode::RepoSelect) => {
            Some(Action::OpenBranches)
        }
        KeyCode::Tab => Some(Action::Noop),
        KeyCode::Esc
            if query.is_empty() && matches!(state.mode, crate::state::Mode::BranchSelect(_)) =>
        {
            Some(Action::BackToRepos)
        }
        KeyCode::Esc if matches!(state.mode, crate::state::Mode::SelectBaseBranch { .. }) => {
            Some(Action::CancelOverlay)
        }
        KeyCode::Esc if query.is_empty() => Some(Action::Quit),
        KeyCode::Esc => Some(Action::ClearQuery),
        KeyCode::Backspace if alt => Some(Action::DeleteWord),
        KeyCode::Backspace => Some(Action::Backspace),
        KeyCode::Char('w') if control => Some(Action::DeleteWord),
        KeyCode::Left if !control && !alt => Some(Action::CursorLeft),
        KeyCode::Right if !control && !alt => Some(Action::CursorRight),
        KeyCode::Char('q')
            if query.is_empty()
                && matches!(state.mode, crate::state::Mode::RepoSelect)
                && !control
                && !alt
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            Some(Action::Quit)
        }
        KeyCode::Char(character)
            if !control && !alt && (character.is_ascii_graphic() || character == ' ') =>
        {
            Some(Action::Insert(character))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        AppState, BaseBranchSelection, BranchContext, DeleteWorktreeTarget, Mode, SearchableList,
    };

    #[test]
    fn q_quits_only_when_query_is_empty() {
        let mut state = AppState::new(None);
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(resolve_action(q, &state), Some(Action::Quit));
        state.repo_list.input.text.push('a');
        state.repo_list.input.cursor = 1;
        assert_eq!(resolve_action(q, &state), Some(Action::Insert('q')));
    }

    #[test]
    fn escape_clears_a_query_before_quitting() {
        let mut state = AppState::new(None);
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(resolve_action(escape, &state), Some(Action::Quit));
        state.repo_list.input.text.push('a');
        assert_eq!(resolve_action(escape, &state), Some(Action::ClearQuery));
    }

    #[test]
    fn branch_keys_enter_back_without_quitting_and_always_type_q() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        assert_eq!(
            resolve_action(
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
                &state
            ),
            Some(Action::Insert('q'))
        );
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &state),
            Some(Action::BackToRepos)
        );
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &state),
            Some(Action::Noop)
        );

        state.branch_list.input.text = "feature".into();
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &state),
            Some(Action::ClearQuery)
        );
    }

    #[test]
    fn loading_accepts_only_ctrl_c() {
        let mut state = AppState::new(None);
        state.mode = crate::state::Mode::Loading {
            message: "Opening…".into(),
            branch: None,
        };
        assert_eq!(
            resolve_action(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &state
            ),
            Some(Action::Quit)
        );
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &state),
            None
        );
    }

    #[test]
    fn kiosk_new_delete_and_modal_defaults_are_preserved() {
        let context = BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        };
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(context.clone());
        state.branch_list.input.text = "feat/new".into();
        state.branch_list.filtered.clear();
        assert_eq!(
            resolve_action(
                KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
                &state
            ),
            Some(Action::StartNewBranch)
        );
        assert_eq!(
            resolve_action(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                &state
            ),
            Some(Action::DeleteWorktree)
        );
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &state),
            Some(Action::StartNewBranch)
        );

        state.mode = Mode::SelectBaseBranch {
            context: context.clone(),
            flow: BaseBranchSelection {
                new_name: "feat/new".into(),
                bases: vec!["main".into()],
                list: SearchableList::new(1),
            },
        };
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &state),
            Some(Action::CreateNewBranch)
        );
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &state),
            Some(Action::CancelOverlay)
        );

        state.mode = Mode::ConfirmWorktreeDelete {
            context,
            target: DeleteWorktreeTarget {
                branch_name: "feature".into(),
                worktree_path: "/repo-feature".into(),
                open_workspace_id: None,
                force: false,
                in_progress: false,
            },
        };
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &state),
            Some(Action::ConfirmDeleteWorktree)
        );
        assert_eq!(
            resolve_action(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &state),
            Some(Action::CancelOverlay)
        );
    }
}
