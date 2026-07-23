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
    app::{TickChanges, process_app_event},
    event::{AppEvent, WorktreeRemovalOutcome},
    git::{GitProvider, Repo, Worktree, mock::MockGitProvider},
    herdr::{
        HerdrError, HerdrProvider, WorktreeInfo, WorktreeListResponse, WorktreeRemoveResponse,
        mock::{HerdrCall, MockHerdrProvider},
    },
    pending_delete::PendingWorktreeDelete,
    spawn::EventSender,
    state::{
        AppState, BranchContext, BranchEntry, Mode, OpenWorktreeLoadState, RepoEntry,
        SearchableList,
    },
};

use super::*;

fn state_with_repo() -> AppState {
    let mut state = AppState::new(None);
    state.repo_view.entries.push(RepoEntry::new(Repo {
        name: "repo".into(),
        path: "/repo".into(),
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

fn remove_response(_forced: bool) -> WorktreeRemoveResponse {
    WorktreeRemoveResponse { warning: None }
}

#[test]
fn delete_guards_refuse_main_checkout_and_remote_only_entries_in_state() {
    let mut state = AppState::new(None);
    state.mode = Mode::BranchSelect(BranchContext {
        repo_path: "/repo".into(),
        repo_name: "repo".into(),
    });
    state.branch_view.entries = vec![BranchEntry {
        name: "main".into(),
        worktree_path: Some("/repo".into()),
        is_current: true,
        is_default: true,
        remote: None,
        open_workspace_id: Some("w_1".into()),
    }];
    state.branch_view.list = SearchableList::new(1);
    assert_eq!(
        selected_target(&state),
        Err("Cannot delete the main checkout")
    );

    state.branch_view.entries[0] = BranchEntry {
        name: "remote-only".into(),
        worktree_path: None,
        is_current: false,
        is_default: false,
        remote: Some("origin".into()),
        open_workspace_id: None,
    };
    assert_eq!(
        selected_target(&state),
        Err("Remote-only branches have no checkout to delete")
    );
}

#[test]
fn delete_before_open_state_load_is_refused() {
    let git_mock = Arc::new(MockGitProvider::default());
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    let mut state = state_with_branch(true);
    state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Unknown;

    begin(&mut state);

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(
        state
            .toasts
            .back()
            .is_some_and(|toast| toast.message.contains("still loading"))
    );
    assert!(git_mock.remove_calls.lock().unwrap().is_empty());
    assert!(herdr_mock.calls.lock().unwrap().is_empty());
}

#[test]
fn delete_after_open_state_failure_is_refused() {
    let mut state = state_with_branch(true);
    state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Failed {
        repo_path: "/repo".into(),
        generation: state.branch_view.generation,
    };

    begin(&mut state);

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.toasts.back().is_some_and(|toast| {
        toast
            .message
            .contains("open checkout state could not be loaded")
    }));
}

#[test]
fn checkout_becoming_open_after_confirmation_routes_removal_through_herdr() {
    let git_mock = Arc::new(MockGitProvider::default());
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    herdr_mock
        .worktree_list_results
        .lock()
        .unwrap()
        .push_back(Ok(worktree_list_response(vec![worktree()])));
    herdr_mock
        .worktree_remove_results
        .lock()
        .unwrap()
        .push_back(Ok(remove_response(false)));
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    assert!(matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow)
            if flow.target.open_workspace_id.is_none()
    ));

    confirm(&mut state, &git, Some(&herdr), &sender);
    let _event = rx.recv_timeout(Duration::from_secs(1)).unwrap();

    assert_eq!(
        *herdr_mock.calls.lock().unwrap(),
        [
            HerdrCall::WorktreeList {
                cwd: "/repo".into(),
            },
            HerdrCall::WorktreeRemove {
                workspace_id: "w_1".into(),
                force: false,
            },
        ]
    );
    assert!(git_mock.remove_calls.lock().unwrap().is_empty());
}

#[test]
fn fresh_open_state_failure_refuses_removal_without_falling_back_to_git() {
    let git_mock = Arc::new(MockGitProvider::default());
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    herdr_mock
        .worktree_list_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::Invocation("herdr unavailable".into())));
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&herdr), &sender);
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(event, &mut state, &mut TickChanges::default());

    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(state.toasts.back().is_some_and(|toast| {
        toast
            .message
            .contains("could not refresh open checkout state")
    }));
    assert!(git_mock.remove_calls.lock().unwrap().is_empty());
    assert_eq!(
        *herdr_mock.calls.lock().unwrap(),
        [HerdrCall::WorktreeList {
            cwd: "/repo".into(),
        }]
    );
}

#[test]
fn herdr_delete_requires_a_second_force_confirmation_then_refreshes() {
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    herdr_mock.worktree_list_results.lock().unwrap().extend([
        Ok(worktree_list_response(vec![worktree()])),
        Ok(worktree_list_response(vec![worktree()])),
    ]);
    herdr_mock.worktree_remove_results.lock().unwrap().extend([
        Err(HerdrError::DirtyWorktreeRequiresForce("dirty".into())),
        Ok(remove_response(true)),
    ]);
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let git = git_provider();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);
    state.branch_view.entries[0].open_workspace_id = Some("w_1".into());

    begin(&mut state);
    confirm(&mut state, &git, Some(&herdr), &sender);
    let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(first, &mut state, &mut TickChanges::default());
    assert!(matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow)
            if flow.target.force && !flow.target.in_progress
    ));

    confirm(&mut state, &git, Some(&herdr), &sender);
    let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(second, &mut state, &mut changes);
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(changes.refresh_branch.is_some());
    assert_eq!(
        *herdr_mock.calls.lock().unwrap(),
        [
            HerdrCall::WorktreeList {
                cwd: "/repo".into(),
            },
            HerdrCall::WorktreeRemove {
                workspace_id: "w_1".into(),
                force: false,
            },
            HerdrCall::WorktreeList {
                cwd: "/repo".into(),
            },
            HerdrCall::WorktreeRemove {
                workspace_id: "w_1".into(),
                force: true,
            },
        ]
    );
}

#[test]
fn closed_git_checkout_requires_force_confirmation_and_prunes_after_success() {
    let git_mock = Arc::new(MockGitProvider::default());
    git_mock.dirty_remove_once.store(true, Ordering::Release);
    let git: Arc<dyn GitProvider> = git_mock.clone();
    let herdr_mock = Arc::new(MockHerdrProvider::default());
    let mut closed_worktree = worktree();
    closed_worktree.open_workspace_id = None;
    herdr_mock.worktree_list_results.lock().unwrap().extend([
        Ok(worktree_list_response(vec![closed_worktree.clone()])),
        Ok(worktree_list_response(vec![closed_worktree])),
    ]);
    let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_branch(true);

    begin(&mut state);
    confirm(&mut state, &git, Some(&herdr), &sender);
    let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    process_app_event(first, &mut state, &mut TickChanges::default());
    assert!(matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow)
            if flow.target.force && !flow.target.in_progress
    ));

    confirm(&mut state, &git, Some(&herdr), &sender);
    let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let mut changes = TickChanges::default();
    process_app_event(second, &mut state, &mut changes);
    assert!(matches!(state.mode, Mode::BranchSelect(_)));
    assert!(changes.refresh_branch.is_some());
    assert_eq!(
        *git_mock.remove_calls.lock().unwrap(),
        [
            (
                PathBuf::from("/repo"),
                PathBuf::from("/repo-feature"),
                false
            ),
            (PathBuf::from("/repo"), PathBuf::from("/repo-feature"), true),
        ]
    );
    assert_eq!(
        *git_mock.prune_calls.lock().unwrap(),
        [PathBuf::from("/repo")]
    );
    assert_eq!(
        *herdr_mock.calls.lock().unwrap(),
        [
            HerdrCall::WorktreeList {
                cwd: "/repo".into(),
            },
            HerdrCall::WorktreeList {
                cwd: "/repo".into(),
            },
        ]
    );
}

#[test]
fn post_deletion_refresh_rejects_old_final_fetch_without_clearing_spinner() {
    let git = git_provider();
    let (sender, _rx) = sender();
    let mut state = state_with_branch(true);
    let old_generation = state.branch_view.generation;
    begin(&mut state);
    if let Mode::ConfirmWorktreeDelete(flow) = &mut state.mode {
        flow.target.in_progress = true;
    }
    let mut changes = TickChanges::default();

    process_app_event(
        AppEvent::WorktreeRemovalFinished {
            repo_path: "/repo".into(),
            branch_name: "feature".into(),
            worktree_path: "/repo-feature".into(),
            outcome: WorktreeRemovalOutcome::Removed { warning: None },
        },
        &mut state,
        &mut changes,
    );
    let repo = changes.refresh_branch.take().expect("branch refresh");
    crate::screens::branch::refresh(&mut state, &git, None, &sender, repo);
    assert_eq!(state.branch_view.generation, old_generation + 1);
    state.branch_view.fetching_remote_repo = Some("/repo".into());

    process_app_event(
        AppEvent::GitFetchCompleted {
            remote: Some("origin".into()),
            branches: Vec::new(),
            repo_path: "/repo".into(),
            generation: old_generation,
            error: None,
            is_final: true,
            skipped_unsupported_refs: false,
        },
        &mut state,
        &mut TickChanges::default(),
    );

    assert_eq!(
        state.branch_view.fetching_remote_repo.as_deref(),
        Some(Path::new("/repo"))
    );
}

#[test]
fn late_recovered_delete_completion_does_not_change_a_newer_mode() {
    let mut state = state_with_repo();
    state.repo_view.entries[0].repo.worktrees.push(Worktree {
        path: "/repo-feature".into(),
        branch: Some("feature".into()),
    });
    state.delete.in_flight.insert("/repo-feature".into());
    mark_pending(
        &mut state,
        PendingWorktreeDelete::new("/repo".into(), "feature".into(), "/repo-feature".into()),
    );
    let mut changes = TickChanges::default();

    process_app_event(
        AppEvent::WorktreeRemovalFinished {
            repo_path: "/repo".into(),
            branch_name: "feature".into(),
            worktree_path: "/repo-feature".into(),
            outcome: WorktreeRemovalOutcome::Removed { warning: None },
        },
        &mut state,
        &mut changes,
    );

    assert_eq!(state.mode, Mode::RepoSelect);
    assert!(state.delete.pending.is_empty());
    assert!(state.repo_view.entries[0].repo.worktrees.is_empty());
    assert!(changes.refresh_branch.is_none());
}
