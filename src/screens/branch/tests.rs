use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use crate::{
    app::{
        FilterWorker, RunOutcome, TickChanges, apply_exit_effects, process_action,
        process_app_event,
    },
    config::keys::KeysConfig,
    event::{AppEvent, FilterKey, FilterTarget},
    git::{GitProvider, Repo, Worktree, mock::MockGitProvider},
    herdr::{
        HerdrError, HerdrProvider, OpenedWorktree, WorktreeCreateResponse, WorktreeInfo,
        WorktreeListResponse, WorktreeOpenResponse,
        mock::{HerdrCall, MockHerdrProvider},
    },
    keyboard::Action,
    spawn::EventSender,
    state::{AppState, BranchEntry, BranchId, Mode, RepoEntry, SearchableList},
};

use super::*;

fn repo(path: &str) -> Repo {
    Repo {
        name: "repo".into(),
        path: path.into(),
        is_git: true,
        worktrees: Vec::new(),
    }
}

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

fn worktree() -> WorktreeInfo {
    WorktreeInfo {
        path: "/repo-feature".into(),
        branch: Some("feature".into()),
        open_workspace_id: Some("w_1".into()),
    }
}

fn opened_worktree() -> OpenedWorktree {
    OpenedWorktree {
        workspace_id: "w_1".into(),
        root_pane_id: "p_root".into(),
        path: "/repo-feature".into(),
    }
}

fn open_response(already_open: bool) -> WorktreeOpenResponse {
    WorktreeOpenResponse {
        opened: Some(opened_worktree()),
        already_open: Some(already_open),
        warning: None,
    }
}

fn create_response() -> WorktreeCreateResponse {
    WorktreeCreateResponse {
        opened: Some(opened_worktree()),
        warning: None,
    }
}

fn worktree_list_response(worktrees: Vec<WorktreeInfo>) -> WorktreeListResponse {
    WorktreeListResponse { worktrees }
}

fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
}

fn git_provider() -> Arc<dyn GitProvider> {
    Arc::new(MockGitProvider::default())
}

#[test]
fn branch_view_transition_and_back_preserve_repo_filter_and_selection() {
    let mut state = state_with_repo();
    state.repo_view.list.input.text = "repo".into();
    state.repo_view.list.input.cursor = 4;
    state.repo_view.list.scroll_offset = 3;
    state.repo_view.selection_touched = true;
    let git = Arc::new(MockGitProvider {
        branches: vec!["main".into()],
        worktrees: vec![Worktree {
            path: "/repo".into(),
            branch: Some("main".into()),
        }],
        ..MockGitProvider::default()
    }) as Arc<dyn GitProvider>;
    let (sender, _rx) = sender();

    crate::screens::branch::enter(&mut state, &git, None, &sender);
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert_eq!(state.repo_view.list.input.text, "repo");
    assert_eq!(state.repo_view.list.selected, Some(0));
    assert_eq!(state.repo_view.list.scroll_offset, 3);

    let filter_worker = FilterWorker::spawn(sender.clone());
    process_action(
        Action::BackToRepos,
        &mut state,
        &git,
        None,
        &sender,
        &filter_worker,
        &KeysConfig::default(),
    );
    assert_eq!(state.mode, Mode::RepoSelect);
    assert_eq!(state.repo_view.list.input.text, "repo");
    assert_eq!(state.repo_view.list.selected, Some(0));
    assert_eq!(state.repo_view.list.scroll_offset, 3);
}

#[test]
fn branch_view_rejects_plain_folders_with_a_hint() {
    let mut state = state_with_repo();
    state.repo_view.entries[0].repo.is_git = false;
    let git = git_provider();
    let (sender, _rx) = sender();

    enter(&mut state, &git, None, &sender);

    assert_eq!(state.mode, Mode::RepoSelect);
    assert_eq!(
        state.toasts.front().map(|toast| toast.message.as_str()),
        Some("Branches are only available for git repositories")
    );
}

#[test]
fn existing_checkout_routes_to_open_and_success_exits() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_open_results
        .lock()
        .unwrap()
        .push_back(Ok(open_response(false)));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    crate::screens::branch::open_selected(&mut state, &git_provider(), Some(&provider), &sender);
    assert!(matches!(
        &state.mode,
        Mode::Loading { message, branch: Some(_) } if message == "Opening feature…"
    ));
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);
    assert!(changes.workspace_opened);
    assert_eq!(
        *mock.calls.lock().unwrap(),
        [HerdrCall::WorktreeOpen {
            cwd: "/repo".into(),
            target: crate::herdr::WorktreeOpenTarget::Branch("feature".into()),
            focus: true,
        }]
    );
}

#[test]
fn on_open_warning_emits_notification_exit_effect_without_cancelling_open_success() {
    let mut state = state_with_repo();
    let mut changes = TickChanges::default();
    let mock = Arc::new(MockHerdrProvider::default());
    mock.notification_show_results
        .lock()
        .unwrap()
        .push_back(Ok(()));
    let provider: Arc<dyn HerdrProvider> = mock.clone();

    process_app_event(
        AppEvent::RepoOpened {
            warning: Some("on_open: pane 1 run failed".into()),
        },
        &mut state,
        &mut changes,
    );

    assert!(changes.workspace_opened);
    assert_eq!(
        apply_exit_effects(&mut changes, Some(&provider)),
        Some(RunOutcome::Opened)
    );
    assert_eq!(
        *mock.calls.lock().unwrap(),
        [HerdrCall::NotificationShow {
            title: "herdr-kiosk".into(),
            body: "on_open: pane 1 run failed".into(),
        }]
    );
}

#[test]
fn missing_checkout_routes_to_create_without_base_or_path_and_success_exits() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);

    crate::screens::branch::open_selected(&mut state, &git_provider(), Some(&provider), &sender);
    assert!(matches!(
        &state.mode,
        Mode::Loading { message, branch: Some(_) }
            if message == "Creating worktree for feature…"
    ));
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);
    assert!(changes.workspace_opened);
    let calls = mock.calls.lock().unwrap();
    let HerdrCall::WorktreeCreate(request) = &calls[0] else {
        panic!("expected worktree create")
    };
    assert_eq!(request.cwd, Path::new("/repo"));
    assert_eq!(request.branch, "feature");
    assert!(request.base.is_none());
    assert!(request.path.is_none());
    assert!(request.focus);
}

#[test]
fn stale_create_route_refreshes_worktrees_and_retries_open_once() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_create_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeCreateFailed(
            "feature is already checked out".into(),
        )));
    mock.worktree_list_results
        .lock()
        .unwrap()
        .push_back(Ok(worktree_list_response(vec![worktree()])));
    mock.worktree_open_results
        .lock()
        .unwrap()
        .push_back(Ok(open_response(true)));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);

    crate::screens::branch::open_selected(&mut state, &git_provider(), Some(&provider), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);

    assert!(changes.workspace_opened);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(matches!(calls[0], HerdrCall::WorktreeCreate(_)));
    assert!(matches!(calls[1], HerdrCall::WorktreeList { .. }));
    assert!(matches!(
        &calls[2],
        HerdrCall::WorktreeOpen {
            target: crate::herdr::WorktreeOpenTarget::Branch(branch),
            ..
        } if branch == "feature"
    ));
}

#[test]
fn stale_open_route_refreshes_worktrees_and_retries_create_once() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_open_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeNotFound("feature vanished".into())));
    mock.worktree_list_results
        .lock()
        .unwrap()
        .push_back(Ok(worktree_list_response(Vec::new())));
    mock.worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    crate::screens::branch::open_selected(&mut state, &git_provider(), Some(&provider), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);

    assert!(changes.workspace_opened);
    let calls = mock.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(matches!(calls[0], HerdrCall::WorktreeOpen { .. }));
    assert!(matches!(calls[1], HerdrCall::WorktreeList { .. }));
    assert!(matches!(calls[2], HerdrCall::WorktreeCreate(_)));
}

#[test]
fn open_failure_returns_to_branch_view() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_open_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeOpenFailed("boom".into())));
    let provider: Arc<dyn HerdrProvider> = mock;
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);
    crate::screens::branch::open_selected(&mut state, &git_provider(), Some(&provider), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.toasts.front().unwrap().message.contains("boom"));
}

#[test]
fn create_in_progress_failure_is_friendly_and_returns_to_branch_view() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_create_results.lock().unwrap().push_back(Err(
        HerdrError::WorktreeOperationInProgress("raw details".into()),
    ));
    let provider: Arc<dyn HerdrProvider> = mock;
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);
    crate::screens::branch::open_selected(&mut state, &git_provider(), Some(&provider), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    let message = &state.toasts.front().unwrap().message;
    assert!(message.contains("already in progress"));
    assert!(!message.contains("worktree_operation_in_progress"));
}

#[test]
fn remote_branch_race_falls_through_to_worktree_create_with_entry_remote() {
    let git_mock = Arc::new(MockGitProvider::default());
    git_mock
        .tracking_already_exists
        .store(true, Ordering::Release);
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    herdr_mock
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);
    state.branch_view.entries[0].remote = Some("upstream".into());

    crate::screens::branch::open_selected(&mut state, &git, Some(&herdr), &sender);

    assert!(matches!(
        &state.mode,
        Mode::Loading { message, branch: Some(_) }
            if message == "Checking out remote branch feature…"
    ));
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);
    assert!(changes.workspace_opened);
    assert_eq!(
        *git_mock.tracking_calls.lock().unwrap(),
        [(PathBuf::from("/repo"), "feature".into(), "upstream".into())]
    );
    let calls = herdr_mock.calls.lock().unwrap();
    let HerdrCall::WorktreeCreate(request) = &calls[0] else {
        panic!("expected worktree create")
    };
    assert_eq!(request.branch, "feature");
    assert!(request.base.is_none());
    assert!(request.path.is_none());
    assert!(request.focus);
}

#[test]
fn remote_tracking_failure_returns_to_branch_view_without_creating_worktree() {
    let git_mock = Arc::new(MockGitProvider::default());
    *git_mock.failure.lock().unwrap() = Some("remote ref is missing".into());
    let git: Arc<dyn GitProvider> = git_mock;
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(false);
    state.branch_view.entries[0].remote = Some("upstream".into());

    crate::screens::branch::open_selected(&mut state, &git, Some(&herdr), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(
        state
            .toasts
            .front()
            .unwrap()
            .message
            .contains("remote ref is missing")
    );
    assert!(herdr_mock.calls.lock().unwrap().is_empty());
}

#[test]
fn same_named_remote_rows_track_the_selected_remote() {
    for (selected, expected_remote) in [(0, "origin"), (1, "upstream")] {
        let git_mock = Arc::new(MockGitProvider::default());
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock
            .worktree_create_results
            .lock()
            .unwrap()
            .push_back(Ok(create_response()));
        let herdr: Arc<dyn HerdrProvider> = herdr_mock;
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        state.branch_view.entries = ["origin", "upstream"]
            .map(|remote| BranchEntry::build_remote(remote, &["feature".into()], &[]))
            .into_iter()
            .flatten()
            .collect();
        state.branch_view.list = SearchableList::new(2);
        state.branch_view.list.selected = Some(selected);

        crate::screens::branch::open_selected(&mut state, &git, Some(&herdr), &sender);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::RepoOpened { .. }
        ));
        assert_eq!(
            *git_mock.tracking_calls.lock().unwrap(),
            [(
                PathBuf::from("/repo"),
                "feature".into(),
                expected_remote.into()
            )]
        );
    }
}

#[test]
fn tracking_success_then_create_failure_reconciles_to_selected_local_branch() {
    let git_mock = Arc::new(MockGitProvider {
        branches: vec!["feature".into()],
        ..MockGitProvider::default()
    });
    let git: Arc<dyn GitProvider> = git_mock;
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    herdr_mock
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeCreateFailed("disk full".into())));
    let herdr: Arc<dyn HerdrProvider> = herdr_mock;
    let (sender, rx) = sender();
    let worker = FilterWorker::spawn(sender.clone());
    let mut state = state_with_branch(false);
    state.branch_view.entries[0].remote = Some("upstream".into());
    state.branch_view.list.input.text = "upstream/feature".into();
    state.branch_view.list.input.cursor = state.branch_view.list.input.text.len();

    crate::screens::branch::open_selected(&mut state, &git, Some(&herdr), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(event, &mut state, &mut changes);

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.branch_view.list.input.text.is_empty());
    assert_eq!(
        state.selected_branch().unwrap().id(),
        BranchId::Local("feature".into())
    );
    assert!(
        state
            .branch_view
            .entries
            .iter()
            .all(|branch| branch.remote.is_none())
    );
    assert!(state.toasts.front().is_some_and(|toast| {
        toast.kind == ToastKind::Error
            && toast
                .message
                .contains("Tracking branch feature was created")
            && toast.message.contains("disk full")
    }));
    assert_eq!(
        changes.pinned_branch_selection,
        Some(BranchId::Local("feature".into()))
    );

    let repo = changes.refresh_branch.take().expect("branch refresh");
    let previous_generation = state.branch_view.generation;
    crate::screens::branch::refresh(&mut state, &git, None, &sender, repo);
    assert_eq!(state.branch_view.generation, previous_generation + 1);
    let refreshed = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut refreshed_changes = TickChanges::default();
    process_app_event(refreshed, &mut state, &mut refreshed_changes);
    crate::screens::branch::queue_filter(
        &mut state,
        &worker,
        refreshed_changes.pinned_branch_selection.take(),
    );

    assert_eq!(
        state.selected_branch().unwrap().id(),
        BranchId::Local("feature".into())
    );
    assert!(state.selected_branch().unwrap().remote.is_none());
}

#[test]
fn remote_merge_reapplies_filter_and_preserves_selection() {
    let (sender, rx) = sender();
    let worker = FilterWorker::spawn(sender);
    let mut state = state_with_branch(false);
    state.branch_view.merge_remote_snapshot(
        "upstream".into(),
        BranchEntry::build_remote("upstream", &["z-feature".into()], &["feature".into()]),
    );
    state.branch_view.list.input.text = "feature".into();
    state.branch_view.list.input.cursor = 7;
    state.branch_view.list.filtered = vec![(0, 0), (1, 0)];
    state.branch_view.list.selected = Some(1);
    state.branch_view.fetching_remote_repo = Some("/repo".into());
    let mut changes = TickChanges::default();

    process_app_event(
        AppEvent::GitFetchCompleted {
            remote: Some("origin".into()),
            branches: BranchEntry::build_remote(
                "origin",
                &["a-feature".into()],
                &["feature".into()],
            ),
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
            error: None,
            is_final: false,
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut changes,
    );
    assert!(changes.branches_changed);
    assert_eq!(
        state.selected_branch().unwrap().name,
        "z-feature",
        "selection must remain safe while the async filter catches up"
    );
    crate::screens::branch::queue_filter(
        &mut state,
        &worker,
        changes.pinned_branch_selection.take(),
    );
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert_eq!(state.branch_view.list.input.text, "feature");
    assert_eq!(state.branch_view.list.filtered.len(), 3);
    assert_eq!(state.selected_branch().unwrap().name, "z-feature");
}

#[test]
fn streamed_remote_update_preserves_same_named_remote_selection_by_identity() {
    let (sender, rx) = sender();
    let worker = FilterWorker::spawn(sender);
    let mut state = state_with_branch(false);
    state.branch_view.entries[0].name = "main".into();
    state.branch_view.merge_remote_snapshot(
        "origin".into(),
        BranchEntry::build_remote("origin", &["feature".into()], &["main".into()]),
    );
    state.branch_view.merge_remote_snapshot(
        "upstream".into(),
        BranchEntry::build_remote("upstream", &["feature".into()], &["main".into()]),
    );
    state.branch_view.list.input.text = "feature".into();
    state.branch_view.list.input.cursor = 7;
    state.branch_view.list.filtered = vec![(1, 0), (2, 0)];
    state.branch_view.list.selected = Some(1);
    let expected = BranchId::Remote {
        remote: "upstream".into(),
        name: "feature".into(),
    };
    assert_eq!(state.selected_branch().unwrap().id(), expected);
    let mut changes = TickChanges::default();

    process_app_event(
        AppEvent::RemoteBranchesLoaded {
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
            remote: "origin".into(),
            branches: BranchEntry::build_remote(
                "origin",
                &["feature".into(), "fix".into()],
                &["main".into()],
            ),
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut changes,
    );
    assert_eq!(state.selected_branch().unwrap().id(), expected);
    crate::screens::branch::queue_filter(
        &mut state,
        &worker,
        changes.pinned_branch_selection.take(),
    );
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert_eq!(state.selected_branch().unwrap().id(), expected);
}

#[test]
fn stale_remote_and_fetch_events_are_dropped() {
    let mut state = state_with_branch(false);
    state.branch_view.fetching_remote_repo = Some("/repo".into());
    let mut changes = TickChanges::default();
    let remote_branch = BranchEntry::build_remote("origin", &["remote".into()], &[]);

    process_app_event(
        AppEvent::RemoteBranchesLoaded {
            repo_path: "/other".into(),
            generation: state.branch_view.generation,
            remote: "origin".into(),
            branches: remote_branch.clone(),
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut changes,
    );
    process_app_event(
        AppEvent::GitFetchCompleted {
            remote: Some("origin".into()),
            branches: remote_branch,
            repo_path: "/other".into(),
            generation: state.branch_view.generation,
            error: Some("boom".into()),
            is_final: true,
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut changes,
    );

    assert_eq!(state.branch_view.entries.len(), 1);
    assert_eq!(
        state.branch_view.fetching_remote_repo.as_deref(),
        Some(Path::new("/repo"))
    );
    assert!(state.toasts.is_empty());
    assert!(!changes.branches_changed);
}

#[test]
fn unsupported_ref_warning_is_emitted_once_without_blanking_valid_branches() {
    let mut state = state_with_branch(false);
    let generation = state.branch_view.generation;
    let mut changes = TickChanges::default();
    let valid = state.branch_view.entries.clone();

    process_app_event(
        AppEvent::BranchesLoaded {
            repo_path: "/repo".into(),
            generation,
            branches: valid,
            worktrees: Vec::new(),
            skipped_unsupported_refs: true,
        },
        &mut state,
        &mut changes,
    );
    process_app_event(
        AppEvent::RemoteBranchesLoaded {
            repo_path: "/repo".into(),
            generation,
            remote: "origin".into(),
            branches: Vec::new(),
            skipped_unsupported_refs: true,
        },
        &mut state,
        &mut changes,
    );

    assert_eq!(state.branch_view.entries[0].name, "feature");
    assert_eq!(
        state
            .toasts
            .iter()
            .filter(|toast| toast.message == crate::git::UNSUPPORTED_REF_WARNING)
            .count(),
        1
    );
}

#[test]
fn same_repo_reentry_drops_every_prior_visit_load_result() {
    let mut state = state_with_branch(false);
    state.branch_view.generation = 2;
    state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Loaded {
        repo_path: "/repo".into(),
        generation: 2,
    };
    state.branch_view.fetching_remote_repo = Some("/repo".into());
    let stale_branch = BranchEntry {
        name: "stale".into(),
        worktree_path: Some("/repo-stale".into()),
        is_current: false,
        is_default: false,
        remote: None,
        open_workspace_id: None,
    };
    let stale_remote = BranchEntry::build_remote("origin", &["remote".into()], &[]);
    let mut changes = TickChanges::default();

    for event in [
        AppEvent::BranchesLoaded {
            repo_path: "/repo".into(),
            generation: 1,
            branches: vec![stale_branch],
            worktrees: Vec::new(),
            skipped_unsupported_refs: false,
        },
        AppEvent::RemoteBranchesLoaded {
            repo_path: "/repo".into(),
            generation: 1,
            remote: "origin".into(),
            branches: stale_remote.clone(),
            skipped_unsupported_refs: false,
        },
        AppEvent::GitFetchCompleted {
            remote: Some("origin".into()),
            branches: stale_remote,
            repo_path: "/repo".into(),
            generation: 1,
            error: Some("stale fetch".into()),
            is_final: true,
            skipped_unsupported_refs: false,
        },
        AppEvent::OpenWorktreesLoaded {
            repo_path: "/repo".into(),
            generation: 1,
            worktrees: vec![worktree()],
        },
    ] {
        process_app_event(event, &mut state, &mut changes);
    }

    assert_eq!(
        state
            .branch_view
            .entries
            .iter()
            .map(|branch| branch.name.as_str())
            .collect::<Vec<_>>(),
        ["feature"]
    );
    assert!(state.branch_view.entries[0].open_workspace_id.is_none());
    assert!(state.branch_view.remote_snapshots_are_empty());
    assert_eq!(
        state.branch_view.fetching_remote_repo.as_deref(),
        Some(Path::new("/repo"))
    );
    assert!(state.toasts.is_empty());
    assert!(!changes.branches_changed);
    assert!(changes.start_remote_loading.is_none());
}

#[test]
fn fetch_failure_toasts_are_deduplicated_per_remote() {
    let mut state = state_with_branch(false);
    state.branch_view.fetching_remote_repo = Some("/repo".into());
    for message in ["first failure", "second failure"] {
        process_app_event(
            AppEvent::GitFetchCompleted {
                remote: Some("origin".into()),
                branches: Vec::new(),
                repo_path: "/repo".into(),
                generation: state.branch_view.generation,
                error: Some(message.into()),
                is_final: false,
                skipped_unsupported_refs: false,
            },
            &mut state,
            &mut TickChanges::default(),
        );
    }

    assert_eq!(state.toasts.len(), 1);
    assert!(state.toasts[0].message.contains("first failure"));
}

#[test]
fn final_fetch_for_current_repo_clears_indicator() {
    let mut state = state_with_branch(false);
    state.branch_view.fetching_remote_repo = Some("/repo".into());

    process_app_event(
        AppEvent::GitFetchCompleted {
            remote: None,
            branches: Vec::new(),
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
            error: None,
            is_final: true,
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut TickChanges::default(),
    );

    assert!(state.branch_view.fetching_remote_repo.is_none());
}

#[test]
fn branch_git_failure_returns_to_repo_but_indicator_failure_does_not() {
    let mut state = state_with_branch(false);
    let mut changes = TickChanges::default();
    process_app_event(
        AppEvent::OpenWorktreesFailed {
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
            message: "indicator unavailable".into(),
        },
        &mut state,
        &mut changes,
    );
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert_eq!(state.toasts.len(), 1);

    process_app_event(
        AppEvent::BranchLoadFailed {
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
            message: "git unavailable".into(),
        },
        &mut state,
        &mut changes,
    );
    assert_eq!(state.mode, Mode::RepoSelect);
    assert_eq!(state.toasts.len(), 2);
}

#[test]
fn same_named_remote_branches_keep_distinct_identities() {
    let mut state = AppState::new(None);
    state.branch_view.merge_remote_snapshot(
        "origin".into(),
        BranchEntry::build_remote("origin", &["feature".into()], &[]),
    );
    state.branch_view.merge_remote_snapshot(
        "upstream".into(),
        BranchEntry::build_remote("upstream", &["feature".into()], &[]),
    );

    assert_eq!(
        state
            .branch_view
            .entries
            .iter()
            .map(BranchEntry::id)
            .collect::<Vec<_>>(),
        [
            BranchId::Remote {
                remote: "origin".into(),
                name: "feature".into(),
            },
            BranchId::Remote {
                remote: "upstream".into(),
                name: "feature".into(),
            },
        ]
    );
    assert_eq!(
        state
            .branch_view
            .entries
            .iter()
            .map(BranchEntry::display_name)
            .collect::<Vec<_>>(),
        ["origin/feature", "upstream/feature"]
    );
}

#[test]
fn local_branch_shadows_same_name_from_every_remote() {
    let mut state = AppState::new(None);
    state.branch_view.entries =
        BranchEntry::build_local(&repo("/repo"), &["feature".into()], None, None);
    state.branch_view.merge_remote_snapshot(
        "origin".into(),
        BranchEntry::build_remote("origin", &["feature".into()], &[]),
    );
    state.branch_view.merge_remote_snapshot(
        "upstream".into(),
        BranchEntry::build_remote("upstream", &["feature".into()], &[]),
    );

    assert_eq!(state.branch_view.entries.len(), 1);
    assert_eq!(
        state.branch_view.entries[0].id(),
        BranchId::Local("feature".into())
    );
}

#[test]
fn remote_merges_are_deduplicated_and_sorted_after_locals() {
    let mut state = AppState::new(None);
    state.branch_view.entries = BranchEntry::build_local(
        &repo("/repo"),
        &["z-local".into(), "main".into()],
        Some("main"),
        None,
    );
    state.branch_view.merge_remote_snapshot(
        "upstream".into(),
        BranchEntry::build_remote(
            "upstream",
            &["z-local".into(), "z-remote".into()],
            &["z-local".into(), "main".into()],
        ),
    );
    state.branch_view.merge_remote_snapshot(
        "origin".into(),
        BranchEntry::build_remote(
            "origin",
            &["a-remote".into(), "z-remote".into()],
            &["z-local".into(), "main".into()],
        ),
    );

    assert_eq!(
        state
            .branch_view
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.remote.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("main", None),
            ("z-local", None),
            ("a-remote", Some("origin")),
            ("z-remote", Some("origin")),
            ("z-remote", Some("upstream")),
        ]
    );
}

#[test]
fn navigation_after_typing_wins_over_the_pending_filter_selection() {
    let git = Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>;
    let (sender, _rx) = sender();
    let worker = FilterWorker::spawn(sender.clone());
    let keys = KeysConfig::default();
    let mut state = state_with_branch(false);
    state.branch_view.entries = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| BranchEntry {
            name: name.into(),
            worktree_path: None,
            is_current: false,
            is_default: false,
            remote: None,
            open_workspace_id: None,
        })
        .collect();
    state.branch_view.list = SearchableList::new(3);
    process_action(
        Action::Insert('a'),
        &mut state,
        &git,
        None,
        &sender,
        &worker,
        &keys,
    );
    process_action(
        Action::MoveSelection(1),
        &mut state,
        &git,
        None,
        &sender,
        &worker,
        &keys,
    );
    let generation = state.branch_view.filter_generation;
    process_app_event(
        AppEvent::FilterCompleted {
            target: FilterTarget::Branches,
            generation,
            matches: vec![
                (FilterKey::Branch("alpha".into()), 3),
                (FilterKey::Branch("beta".into()), 2),
                (FilterKey::Branch("gamma".into()), 1),
            ],
            selected: None,
        },
        &mut state,
        &mut TickChanges::default(),
    );
    assert_eq!(state.selected_branch().unwrap().name, "beta");
}
