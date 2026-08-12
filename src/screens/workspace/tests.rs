use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::herdr::WorkspaceWorktreeInfo;

use super::*;

fn workspace(
    id: &str,
    label: &str,
    focused: bool,
    last_focused_unix_ms: Option<u64>,
) -> WorkspaceInfo {
    WorkspaceInfo {
        workspace_id: id.into(),
        label: label.into(),
        focused,
        agent_status: None,
        branch: None,
        last_focused_unix_ms,
        worktree: None,
    }
}

fn ids(workspaces: &[WorkspaceInfo]) -> Vec<&str> {
    workspaces
        .iter()
        .map(|workspace| workspace.workspace_id.as_str())
        .collect()
}

#[test]
fn ordering_is_focused_first_then_recency_then_herdr_order() {
    let ordered = order_workspaces(vec![
        workspace("w1", "unstamped-a", false, None),
        workspace("w2", "old", false, Some(100)),
        workspace("w3", "current", true, Some(50)),
        workspace("w4", "recent", false, Some(300)),
        workspace("w5", "unstamped-b", false, None),
    ]);

    assert_eq!(ids(&ordered), ["w3", "w4", "w2", "w1", "w5"]);
}

#[test]
fn focused_workspace_leads_even_without_a_recency_stamp() {
    let ordered = order_workspaces(vec![
        workspace("w1", "stamped", false, Some(500)),
        workspace("w2", "current", true, None),
    ]);

    assert_eq!(ids(&ordered), ["w2", "w1"]);
}

#[test]
fn entries_prefer_the_repo_name_and_search_branch_and_label() {
    let mut info = workspace("w1", "photodrop fix-modals", false, None);
    info.branch = Some("fix-modals".into());
    info.worktree = Some(WorkspaceWorktreeInfo {
        repo_root: "/repos/photodrop".into(),
        repo_name: "photodrop".into(),
    });

    let entry = WorkspaceEntry::from_info(info);

    assert_eq!(entry.name, "photodrop");
    assert_eq!(entry.branch.as_deref(), Some("fix-modals"));
    assert_eq!(
        entry.search_text,
        "photodrop fix-modals photodrop fix-modals"
    );
}

#[test]
fn plain_workspaces_fall_back_to_the_label() {
    let entry = WorkspaceEntry::from_info(workspace("w1", "~", false, None));

    assert_eq!(entry.name, "~");
    assert_eq!(entry.search_text, "~");
}

#[test]
fn load_starts_the_cursor_on_the_previous_workspace() {
    let mut state = WorkspacePickerState::new();
    state.load(vec![
        workspace("w_current", "current", true, Some(200)),
        workspace("w_previous", "previous", false, Some(100)),
        workspace("w_old", "old", false, Some(50)),
    ]);

    assert_eq!(
        state
            .selected_entry()
            .map(|entry| entry.workspace_id.as_str()),
        Some("w_previous")
    );
}

#[test]
fn load_with_a_single_workspace_selects_it() {
    let mut state = WorkspacePickerState::new();
    state.load(vec![workspace("w_only", "only", true, None)]);

    assert_eq!(
        state
            .selected_entry()
            .map(|entry| entry.workspace_id.as_str()),
        Some("w_only")
    );
}

#[test]
fn refilter_matches_branches_and_clearing_restores_the_toggle_selection() {
    let matcher = SkimMatcherV2::default();
    let mut state = WorkspacePickerState::new();
    let mut with_branch = workspace("w_branch", "photodrop fix-modals", false, Some(50));
    with_branch.branch = Some("fix-modals".into());
    state.load(vec![
        workspace("w_current", "current", true, Some(200)),
        workspace("w_previous", "previous", false, Some(100)),
        with_branch,
    ]);

    state.list.input.text = "fixmod".into();
    state.refilter(&matcher);
    assert_eq!(
        state
            .selected_entry()
            .map(|entry| entry.workspace_id.as_str()),
        Some("w_branch")
    );

    state.list.input.clear();
    state.refilter(&matcher);
    assert_eq!(
        state
            .selected_entry()
            .map(|entry| entry.workspace_id.as_str()),
        Some("w_previous")
    );
}

#[test]
fn keys_resolve_against_the_repo_picker_bindings() {
    let keys = KeysConfig::default();
    let cases = [
        (KeyCode::Enter, KeyModifiers::NONE, "", PickerAction::Open),
        (KeyCode::Up, KeyModifiers::NONE, "", PickerAction::Move(-1)),
        (KeyCode::Down, KeyModifiers::NONE, "", PickerAction::Move(1)),
        (
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            "query",
            PickerAction::Quit,
        ),
        (
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            "",
            PickerAction::Quit,
        ),
        (
            KeyCode::Char('q'),
            KeyModifiers::NONE,
            "query",
            PickerAction::Insert('q'),
        ),
        (KeyCode::Esc, KeyModifiers::NONE, "", PickerAction::Quit),
        (
            KeyCode::Esc,
            KeyModifiers::NONE,
            "query",
            PickerAction::Clear,
        ),
        (
            KeyCode::Backspace,
            KeyModifiers::NONE,
            "query",
            PickerAction::Backspace,
        ),
        (
            KeyCode::Char('w'),
            KeyModifiers::CONTROL,
            "query",
            PickerAction::DeleteWord,
        ),
        (
            KeyCode::Char('é'),
            KeyModifiers::NONE,
            "",
            PickerAction::Insert('é'),
        ),
    ];
    for (code, modifiers, query, expected) in cases {
        assert_eq!(
            resolve_action(KeyEvent::new(code, modifiers), query, &keys),
            expected,
            "for {code:?} {modifiers:?} with query {query:?}"
        );
    }
}

#[test]
fn repo_only_commands_swallow_their_keys_instead_of_inserting() {
    let keys = KeysConfig::default();
    // `tab` opens the branch view in the repo picker; the workspace picker has
    // no branch view, and the key must not become query text.
    assert_eq!(
        resolve_action(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), "", &keys),
        PickerAction::Noop
    );
}

#[test]
fn focusing_the_selection_exits_and_targets_the_selected_workspace() {
    use crate::herdr::mock::{HerdrCall, MockHerdrProvider};

    let mock = MockHerdrProvider::default();
    mock.workspace_focus_results
        .lock()
        .unwrap()
        .push_back(Ok(()));
    let mut state = WorkspacePickerState::new();
    state.load(vec![
        workspace("w_current", "current", true, Some(200)),
        workspace("w_previous", "previous", false, Some(100)),
    ]);

    let outcome = focus_selected(&mut state, &mock);

    assert_eq!(outcome, Some(RunOutcome::Focused));
    assert!(state.error.is_none());
    assert_eq!(
        *mock.calls.lock().unwrap(),
        [HerdrCall::WorkspaceFocus {
            workspace_id: "w_previous".into()
        }]
    );
}

#[test]
fn focus_errors_keep_the_picker_open_with_an_error_message() {
    use crate::herdr::HerdrError;
    use crate::herdr::mock::MockHerdrProvider;

    let mock = MockHerdrProvider::default();
    mock.workspace_focus_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::Other {
            code: "workspace_not_found".into(),
            message: "workspace w_previous not found".into(),
        }));
    let mut state = WorkspacePickerState::new();
    state.load(vec![
        workspace("w_current", "current", true, Some(200)),
        workspace("w_previous", "previous", false, Some(100)),
    ]);

    let outcome = focus_selected(&mut state, &mock);

    assert_eq!(outcome, None);
    assert!(
        state
            .error
            .as_deref()
            .is_some_and(|error| error.contains("workspace w_previous not found"))
    );
}

#[test]
fn focus_with_nothing_selected_is_a_quiet_no_op() {
    use crate::herdr::mock::MockHerdrProvider;

    let mock = MockHerdrProvider::default();
    let mut state = WorkspacePickerState::new();

    assert_eq!(focus_selected(&mut state, &mock), None);
    assert!(state.error.is_none());
    assert!(mock.calls.lock().unwrap().is_empty());
}
