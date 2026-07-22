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
    if matches!(state.mode, crate::state::Mode::Loading { .. }) {
        return None;
    }

    let query = match state.mode {
        crate::state::Mode::RepoSelect => &state.repo_list.input.text,
        crate::state::Mode::BranchSelect(_) => &state.branch_list.input.text,
        crate::state::Mode::Loading { .. } => unreachable!("handled above"),
    };

    match key.code {
        KeyCode::Up | KeyCode::Char('p') if control => Some(Action::MoveSelection(-1)),
        KeyCode::Down | KeyCode::Char('n') if control => Some(Action::MoveSelection(1)),
        KeyCode::Up => Some(Action::MoveSelection(-1)),
        KeyCode::Down => Some(Action::MoveSelection(1)),
        KeyCode::Enter if matches!(state.mode, crate::state::Mode::RepoSelect) => {
            Some(Action::OpenRepo)
        }
        KeyCode::Enter => Some(Action::OpenBranch),
        KeyCode::Tab if matches!(state.mode, crate::state::Mode::RepoSelect) => {
            Some(Action::OpenBranches)
        }
        KeyCode::Tab => Some(Action::Noop),
        KeyCode::Esc
            if query.is_empty() && matches!(state.mode, crate::state::Mode::BranchSelect(_)) =>
        {
            Some(Action::BackToRepos)
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
    use crate::state::{AppState, BranchContext, Mode};

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
}
