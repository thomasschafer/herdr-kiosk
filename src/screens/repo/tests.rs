use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

use crate::{
    app::{TickChanges, process_app_event},
    config::SortOrder,
    event::{AppEvent, FilterKey, FilterTarget},
    git::Repo,
    herdr::{
        HerdrError, HerdrProvider, PaneInfo, WorkspaceCreateResponse, WorkspaceInfo,
        WorkspaceWorktreeInfo,
        mock::{HerdrCall, MockHerdrProvider},
    },
    recency::RecencyKey,
    spawn::EventSender,
    state::{AppState, Mode, SearchableList},
};

use super::*;

fn repo(path: &str) -> Repo {
    Repo {
        name: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
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

fn state_with_folder() -> AppState {
    let mut state = state_with_repo();
    state.repo_view.entries[0].repo.is_git = false;
    state
}

fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
    let (tx, rx) = mpsc::channel();
    (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
}

#[test]
fn collision_disambiguates_two_equal_names_with_shortest_parent_suffix() {
    let repos = [repo("foo/bar/baz"), repo("qux/bar/baz")];
    assert_eq!(
        collision_disambiguators(&repos),
        [Some("foo/bar".into()), Some("qux/bar".into())]
    );
}

#[test]
fn collision_disambiguates_three_places_independently() {
    let repos = [
        repo("/root/a/shared/demo"),
        repo("/root/b/shared/demo"),
        repo("/root/unique/demo"),
    ];
    assert_eq!(
        collision_disambiguators(&repos),
        [
            Some("…/a/shared".into()),
            Some("…/b/shared".into()),
            Some("…/unique".into()),
        ]
    );
}

#[test]
fn collision_handles_parents_that_also_collide() {
    let repos = [repo("/one/team/api"), repo("/two/team/api")];
    assert_eq!(
        collision_disambiguators(&repos),
        [Some("one/team".into()), Some("two/team".into())]
    );
}

#[test]
fn collision_handles_repos_nested_below_search_roots() {
    let repos = [
        repo("/search/client/ios/app"),
        repo("/search/client/web/app"),
        repo("/search/direct/app"),
        repo("/search/other"),
    ];
    assert_eq!(
        collision_disambiguators(&repos),
        [
            Some("…/ios".into()),
            Some("…/web".into()),
            Some("…/direct".into()),
            None,
        ]
    );
}

#[test]
fn collision_leaves_unique_names_unchanged() {
    let repos = [repo("/one/alpha"), repo("/two/beta")];
    assert_eq!(collision_disambiguators(&repos), [None, None]);
}

#[test]
fn collision_windows_mode_is_case_insensitive_and_uses_forward_slashes() {
    let repos = [repo("C:/One/Team/API"), repo("c:/two/team/api")];
    assert_eq!(
        collision_disambiguators_with_case(&repos, true),
        [Some("…/One/Team".into()), Some("…/two/team".into())]
    );
}

#[test]
fn current_repo_selection_prefers_the_deepest_containing_repo() {
    let mut state = AppState::new(Some("/work/outer/inner/src".into()));
    state.repo_view.entries = vec![
        RepoEntry::new(repo("/work/outer")),
        RepoEntry::new(repo("/work/outer/inner")),
    ];
    state.repo_view.list = SearchableList::new(2);
    state.repo_view.apply_current_selection();
    assert_eq!(state.repo_view.list.selected, Some(1));
}

#[test]
fn alphabetical_sort_is_unchanged_by_populated_recency_state() {
    let mut state = AppState::new(None);
    state.repo_view.entries = ["/repos/zulu", "/repos/Alpha", "/other/alpha", "/repos/beta"]
        .into_iter()
        .map(repo)
        .map(RepoEntry::new)
        .collect();
    state.repo_view.list = SearchableList::new(state.repo_view.entries.len());
    sort_entries(&mut state);
    let baseline = state.repo_view.entries.clone();

    state
        .recency
        .record(RecencyKey::repo(Path::new("/repos/zulu")));
    state
        .recency
        .record(RecencyKey::repo(Path::new("/repos/beta")));
    state.repo_view.entries.reverse();
    sort_entries(&mut state);

    assert_eq!(state.sort_order, SortOrder::Alphabetical);
    assert_eq!(state.repo_view.entries, baseline);
}

#[test]
fn recency_resting_sort_uses_rank_then_alphabetical_fallback() {
    let mut state = AppState::new(None);
    state.sort_order = SortOrder::Recency;
    state.repo_view.entries = [
        "/repos/delta",
        "/repos/alpha",
        "/repos/charlie",
        "/repos/bravo",
    ]
    .into_iter()
    .map(repo)
    .map(RepoEntry::new)
    .collect();
    state.repo_view.list = SearchableList::new(state.repo_view.entries.len());
    state
        .recency
        .record(RecencyKey::repo(Path::new("/repos/alpha")));
    state
        .recency
        .record(RecencyKey::repo(Path::new("/repos/delta")));

    sort_entries(&mut state);

    assert_eq!(
        state
            .repo_view
            .entries
            .iter()
            .map(|entry| entry.repo.name.as_str())
            .collect::<Vec<_>>(),
        ["delta", "alpha", "bravo", "charlie"]
    );
}

#[test]
fn recency_defaults_to_previous_repo_while_alphabetical_keeps_current_repo() {
    let entries = || {
        ["/work/alpha", "/work/beta"]
            .into_iter()
            .map(repo)
            .map(RepoEntry::new)
            .collect::<Vec<_>>()
    };
    let mut alphabetical = AppState::new(Some("/work/beta/src".into()));
    alphabetical.repo_view.entries = entries();
    alphabetical.repo_view.list = SearchableList::new(2);
    sort_entries(&mut alphabetical);
    apply_default_selection(&mut alphabetical);
    assert_eq!(alphabetical.selected_repo().unwrap().repo.name, "beta");

    let mut recency = AppState::new(Some("/work/beta/src".into()));
    recency.sort_order = SortOrder::Recency;
    recency.repo_view.entries = entries();
    recency.repo_view.list = SearchableList::new(2);
    recency
        .recency
        .record(RecencyKey::repo(Path::new("/work/alpha")));
    recency
        .recency
        .record(RecencyKey::repo(Path::new("/work/beta")));
    sort_entries(&mut recency);
    apply_default_selection(&mut recency);
    assert_eq!(recency.selected_repo().unwrap().repo.name, "alpha");
}

#[test]
fn current_logical_selection_survives_current_generation_filter_results() {
    let mut state = AppState::new(None);
    state.repo_view.entries = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| {
            RepoEntry::new(Repo {
                name: name.into(),
                path: PathBuf::from(format!("/{name}")),
                is_git: true,
                worktrees: Vec::new(),
            })
        })
        .collect();
    state.repo_view.list = SearchableList::new(3);
    state.repo_view.list.selected = Some(1);
    state.repo_view.filter_generation = 7;
    process_app_event(
        AppEvent::FilterCompleted {
            target: FilterTarget::Repos,
            generation: 7,
            matches: vec![
                (FilterKey::Repo("/gamma".into()), 3),
                (FilterKey::Repo("/beta".into()), 2),
                (FilterKey::Repo("/alpha".into()), 1),
            ],
            selected: None,
        },
        &mut state,
        &mut TickChanges::default(),
    );
    assert_eq!(state.selected_repo().unwrap().repo.name, "beta");
}

#[test]
fn opening_transitions_to_loading_and_dispatches_through_mock_provider() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.worktree_open_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeOpenFailed("boom".into())));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_repo();

    open_selected(&mut state, Some(&provider), &sender);

    assert_eq!(
        state.mode,
        Mode::Loading {
            message: "Opening repo…".into(),
            branch: None,
        }
    );
    let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        &event,
        AppEvent::RepoOpenFailed(message) if message.contains("boom")
    ));
    process_app_event(event, &mut state, &mut TickChanges::default());
    assert_eq!(state.mode, Mode::RepoSelect);
    assert!(state.toasts.front().unwrap().message.contains("boom"));
    assert_eq!(
        *mock.calls.lock().unwrap(),
        [HerdrCall::WorktreeOpen {
            cwd: "/repo".into(),
            target: crate::herdr::WorktreeOpenTarget::Path("/repo".into()),
            focus: true,
        }]
    );
}

#[test]
fn opening_matching_plain_folder_focuses_existing_workspace() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.pane_list_results
        .lock()
        .unwrap()
        .push_back(Ok(vec![PaneInfo {
            workspace_id: "w_folder".into(),
            cwd: Some("/repo".into()),
            foreground_cwd: Some("/unrelated".into()),
        }]));
    mock.workspace_focus_results
        .lock()
        .unwrap()
        .push_back(Ok(()));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_folder();

    open_selected(&mut state, Some(&provider), &sender);

    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        AppEvent::RepoOpened { warning: None }
    ));
    assert_eq!(
        *mock.calls.lock().unwrap(),
        [
            HerdrCall::PaneList,
            HerdrCall::WorkspaceFocus {
                workspace_id: "w_folder".into(),
            },
        ]
    );
}

#[test]
fn opening_unmatched_plain_folder_creates_focused_workspace() {
    let mock = Arc::new(MockHerdrProvider::default());
    mock.pane_list_results
        .lock()
        .unwrap()
        .push_back(Ok(Vec::new()));
    mock.workspace_create_results
        .lock()
        .unwrap()
        .push_back(Ok(WorkspaceCreateResponse {
            workspace_id: Some("w_new".into()),
            warning: None,
        }));
    let provider: Arc<dyn HerdrProvider> = mock.clone();
    let (sender, rx) = sender();
    let mut state = state_with_folder();

    open_selected(&mut state, Some(&provider), &sender);

    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        AppEvent::RepoOpened { warning: None }
    ));
    assert_eq!(
        *mock.calls.lock().unwrap(),
        [
            HerdrCall::PaneList,
            HerdrCall::WorkspaceCreate {
                cwd: "/repo".into(),
                focus: true,
            },
        ]
    );
}

#[test]
fn open_indicators_use_workspaces_for_git_and_pane_cwds_for_folders() {
    let mut state = state_with_repo();
    state.repo_view.entries.push(RepoEntry::new(Repo {
        name: "folder".into(),
        path: "/folder".into(),
        is_git: false,
        worktrees: Vec::new(),
    }));
    state.repo_view.list = SearchableList::new(2);

    process_app_event(
        AppEvent::OpenWorkspacesLoaded {
            workspaces: vec![WorkspaceInfo {
                worktree: Some(WorkspaceWorktreeInfo {
                    repo_root: "/repo".into(),
                }),
            }],
        },
        &mut state,
        &mut TickChanges::default(),
    );
    process_app_event(
        AppEvent::OpenFolderPanesLoaded {
            panes: vec![PaneInfo {
                workspace_id: "w_folder".into(),
                cwd: Some("/folder".into()),
                foreground_cwd: Some("/repo".into()),
            }],
        },
        &mut state,
        &mut TickChanges::default(),
    );

    assert!(state.repo_view.entries[0].is_open);
    assert!(state.repo_view.entries[1].is_open);
}

#[test]
fn opening_without_herdr_keeps_picker_usable_and_shows_error() {
    let (sender, _rx) = sender();
    let mut state = state_with_repo();
    open_selected(&mut state, None, &sender);
    assert_eq!(state.mode, Mode::RepoSelect);
    assert_eq!(
        state.toasts.front().unwrap().message,
        "not running inside herdr"
    );
}
