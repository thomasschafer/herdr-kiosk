use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::{Duration, Instant},
};

use ratatui::{Terminal, backend::TestBackend};

use crate::{
    app::{FilterWorker, TickChanges, process_action, process_app_event},
    config::keys::KeysConfig,
    event::{AppEvent, FilterKey, FilterTarget},
    git::{GitProvider, Repo, mock::MockGitProvider},
    herdr::{
        HerdrProvider, OpenedWorktree, WorktreeCreateResponse,
        mock::{HerdrCall, MockHerdrProvider},
    },
    keyboard::Action,
    screens::repo::RepoEntry,
    spawn::EventSender,
    state::{AppState, BranchContext, BranchEntry, Mode, OpenWorktreeLoadState, SearchableList},
    theme::Theme,
};

use super::*;

fn state_with_repo() -> AppState {
    let mut state = AppState::new(None);
    state.repo_view.entries.push(RepoEntry::new(Repo {
        name: "repo".into(),
        path: "/repo".into(),
        is_git: true,
        worktrees: Vec::new(),
    }));
    state.repo_view.list = SearchableList::new(1);
    state
}

fn state_with_branch(has_worktree: bool) -> AppState {
    let mut state = state_with_repo();
    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    state.branch_view.entries = vec![BranchEntry {
        name: "feature".into(),
        worktree_path: has_worktree.then(|| PathBuf::from("/repo-feature")),
        is_current: false,
        is_default: false,
        remote: None,
        open_workspace_id: None,
    }];
    state.branch_view.list = SearchableList::new(1);
    state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Loaded {
        repo_path: "/repo".into(),
        generation: state.branch_view.generation,
    };
    state
}

fn opened_worktree() -> OpenedWorktree {
    OpenedWorktree {
        workspace_id: "w_1".into(),
        root_pane_id: "p_root".into(),
        path: "/repo-feature".into(),
    }
}

fn create_response() -> WorktreeCreateResponse {
    WorktreeCreateResponse {
        opened: Some(opened_worktree()),
        warning: None,
    }
}

fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
}

#[test]
fn new_branch_routing_rejects_empty_and_routes_existing_local() {
    let mut state = AppState::new(None);
    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    assert_eq!(route(&state), Err("Type a branch name first"));

    state.branch_view.entries = BranchEntry::build_local(
        &Repo {
            name: "repo".into(),
            path: "/repo".into(),
            is_git: true,
            worktrees: Vec::new(),
        },
        &["feature".into()],
        None,
        None,
    );
    state.branch_view.list = SearchableList::new(1);
    state.branch_view.list.input.text = "feature".into();
    assert!(matches!(
        route(&state),
        Ok(NewBranchRoute::Existing(branch)) if branch.name == "feature"
    ));
}

#[test]
fn current_logical_selection_survives_current_generation_filter_results() {
    let mut state = state_with_branch(false);
    state.mode = Mode::SelectBaseBranch {
        context: BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        },
        flow: BaseBranchSelection {
            new_name: "new".into(),
            bases: vec!["alpha".into(), "beta".into(), "gamma".into()],
            list: SearchableList::new(3),
        },
    };
    if let Mode::SelectBaseBranch { flow, .. } = &mut state.mode {
        flow.list.selected = Some(1);
    }
    state.new_branch.filter_generation = 11;
    process_app_event(
        AppEvent::FilterCompleted {
            target: FilterTarget::Bases,
            generation: 11,
            matches: vec![
                (FilterKey::Base("gamma".into()), 3),
                (FilterKey::Base("beta".into()), 2),
                (FilterKey::Base("alpha".into()), 1),
            ],
            selected: None,
        },
        &mut state,
        &mut TickChanges::default(),
    );
    let Mode::SelectBaseBranch { flow, .. } = &state.mode else {
        unreachable!()
    };
    let selected = flow.list.selected.unwrap();
    let index = flow.list.filtered[selected].0;
    assert_eq!(flow.bases[index], "beta");
}

#[test]
fn base_picker_text_actions_edit_only_the_base_query() {
    let mut state = state_with_branch(false);
    state.branch_view.list.input.text = "underlying".into();
    state.mode = Mode::SelectBaseBranch {
        context: BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        },
        flow: BaseBranchSelection {
            new_name: "feat/new".into(),
            bases: vec!["main".into(), "feature".into()],
            list: SearchableList::new(2),
        },
    };
    let Mode::SelectBaseBranch { flow, .. } = &mut state.mode else {
        unreachable!()
    };
    flow.list.input.text = "one two".into();
    flow.list.input.cursor = flow.list.input.text.len();

    let git = Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>;
    let (sender, _rx) = sender();
    let worker = FilterWorker::spawn(sender.clone());
    let keys = KeysConfig::default();
    for action in [
        Action::CursorLeft,
        Action::CursorRight,
        Action::Backspace,
        Action::DeleteWord,
        Action::Insert('x'),
    ] {
        process_action(action, &mut state, &git, None, &sender, &worker, &keys);
    }
    let Mode::SelectBaseBranch { flow, .. } = &state.mode else {
        unreachable!()
    };
    assert_eq!(flow.list.input.text, "one x");
    assert_eq!(state.branch_view.list.input.text, "underlying");
}

#[test]
fn validating_new_branch_keeps_branch_view_visible_under_popup() {
    let mut state = state_with_branch(false);
    state.mode = Mode::ValidatingNewBranch {
        context: BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        },
        name: "feat/new".into(),
    };
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    let theme = Theme::from_config(&crate::config::ThemeConfig::default());
    terminal
        .draw(|frame| {
            crate::app::draw(
                frame,
                &mut state,
                &theme,
                &KeysConfig::default(),
                Instant::now(),
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rendered = buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>();
    assert!(rendered.contains("repo — select branch"));
    assert!(rendered.contains("feature"));
    assert!(rendered.contains("New branch \"feat/new\""));
    assert!(rendered.contains("Validating branch name…"));
}

#[test]
fn invalid_new_branch_name_is_validated_by_git_and_returns_to_branch_view() {
    let git_mock = Arc::new(MockGitProvider {
        invalid_branch_names: HashSet::from(["bad..name".into()]),
        ..MockGitProvider::default()
    });
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);
    state.branch_view.list.input.text = "bad..name".into();
    state.branch_view.list.input.cursor = 9;

    start(&mut state, &git, None, &sender);
    assert!(matches!(state.mode, Mode::ValidatingNewBranch { .. }));
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(
        state
            .toasts
            .front()
            .unwrap()
            .message
            .contains("Invalid branch name")
    );
    assert_eq!(
        *git_mock.validation_calls.lock().unwrap(),
        [(PathBuf::from("/repo"), "bad..name".into())]
    );
}

#[test]
fn new_branch_before_local_load_stays_in_branch_view_then_works_after_load() {
    let git_mock = Arc::new(MockGitProvider::default());
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);
    state.branch_view.entries.clear();
    state.branch_view.list = SearchableList::new(0);
    state.branch_view.list.input.text = "feat/new".into();
    state.branch_view.list.input.cursor = "feat/new".len();
    state.branch_view.loading = true;

    start(&mut state, &git, None, &sender);

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.branch_view.loading);
    assert!(
        state
            .toasts
            .back()
            .unwrap()
            .message
            .contains("still loading")
    );
    assert!(rx.try_recv().is_err());

    state.branch_view.loading = false;
    start(&mut state, &git, None, &sender);

    assert!(matches!(state.mode, Mode::ValidatingNewBranch { .. }));
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(event, AppEvent::BranchNameValidated { .. }));
    assert_eq!(
        *git_mock.validation_calls.lock().unwrap(),
        [(PathBuf::from("/repo"), "feat/new".into())]
    );
}

#[test]
fn validated_new_branch_preselects_known_default_local_base() {
    let mut state = state_with_branch(false);
    state.branch_view.entries = vec![
        BranchEntry {
            name: "feature".into(),
            worktree_path: None,
            is_current: false,
            is_default: false,
            remote: None,
            open_workspace_id: None,
        },
        BranchEntry {
            name: "main".into(),
            worktree_path: Some("/repo".into()),
            is_current: true,
            is_default: true,
            remote: None,
            open_workspace_id: Some("w_1".into()),
        },
        BranchEntry {
            name: "remote".into(),
            worktree_path: None,
            is_current: false,
            is_default: false,
            remote: Some("origin".into()),
            open_workspace_id: None,
        },
    ];
    state.mode = Mode::ValidatingNewBranch {
        context: BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        },
        name: "feat/new".into(),
    };
    process_app_event(
        AppEvent::BranchNameValidated {
            repo_path: "/repo".into(),
            branch_name: "feat/new".into(),
            valid: true,
            error: None,
        },
        &mut state,
        &mut TickChanges::default(),
    );

    let Mode::SelectBaseBranch { flow, .. } = &state.mode else {
        panic!("expected base picker")
    };
    assert_eq!(flow.bases, ["feature", "main"]);
    let selected = flow.list.filtered[flow.list.selected.unwrap()].0;
    assert_eq!(flow.bases[selected], "main");
}

#[test]
fn selected_base_is_passed_to_focused_new_branch_creation() {
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    herdr_mock
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);
    state.mode = Mode::SelectBaseBranch {
        context: BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        },
        flow: BaseBranchSelection {
            new_name: "feat/new".into(),
            bases: vec!["main".into(), "feature".into()],
            list: SearchableList {
                selected: Some(1),
                ..SearchableList::new(2)
            },
        },
    };

    create(&mut state, Some(&herdr), &sender);
    assert!(matches!(
        &state.mode,
        Mode::Loading { message, .. } if message == "Creating feat/new from feature…"
    ));
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);
    assert!(changes.workspace_opened);
    let calls = herdr_mock.calls.lock().unwrap();
    let HerdrCall::WorktreeCreate(request) = &calls[0] else {
        panic!("expected worktree create")
    };
    assert_eq!(request.branch, "feat/new");
    assert_eq!(request.base.as_deref(), Some("feature"));
    assert!(request.focus);
}
