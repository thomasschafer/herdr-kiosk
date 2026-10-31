use std::path::PathBuf;

use kiosk_core::config::on_open::{OnOpenPaneConfig, OnOpenPaneDirection};

use super::*;
use crate::herdr::{
    PaneInfo, PaneSplitResponse, WorkspaceCreateResponse, WorkspaceInfo, WorkspaceWorktreeInfo,
    WorktreeCreateResponse, WorktreeInfo, WorktreeListResponse, WorktreeOpenResponse,
    WorktreeRemoveResponse,
    mock::{HerdrCall, MockHerdrCli},
};

fn on_open_config() -> OnOpenConfig {
    OnOpenConfig {
        panes: vec![
            OnOpenPaneConfig {
                command: "hx".into(),
                direction: OnOpenPaneDirection::Right,
                ratio: Some(0.4),
            },
            OnOpenPaneConfig {
                command: "cargo test".into(),
                direction: OnOpenPaneDirection::Down,
                ratio: None,
            },
        ],
    }
}

fn tui() -> HerdrTui<MockHerdrCli> {
    HerdrTui::new(MockHerdrCli::default(), on_open_config())
}

fn opened_worktree() -> OpenedWorktree {
    OpenedWorktree {
        root_pane_id: "p_root".into(),
        path: "/repo".into(),
    }
}

fn open_response(already_open: bool) -> WorktreeOpenResponse {
    WorktreeOpenResponse {
        opened: Some(opened_worktree()),
        workspace_id: Some("w_1".into()),
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

fn queue_successful_on_open(cli: &MockHerdrCli) {
    cli.pane_open_results
        .lock()
        .unwrap()
        .extend(["p_2", "p_3"].map(|pane_id| {
            Ok(PaneSplitResponse {
                pane_id: pane_id.into(),
            })
        }));
}

fn pane_call_count(cli: &MockHerdrCli) -> usize {
    cli.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| matches!(call, HerdrCall::PaneOpen { .. }))
        .count()
}

fn checkout_workspace(workspace: &str) -> CheckoutWorkspace {
    CheckoutWorkspace {
        workspace: WorkspaceId(workspace.into()),
        repo: "/repo".into(),
        checkout: "/worktrees/feature".into(),
    }
}

fn workspace(workspace_id: &str, worktree: Option<&str>) -> WorkspaceInfo {
    WorkspaceInfo {
        workspace_id: workspace_id.into(),
        worktree: worktree.map(|repo_root| WorkspaceWorktreeInfo {
            repo_root: repo_root.into(),
        }),
    }
}

fn worktree(branch: &str, open: Option<&str>) -> WorktreeInfo {
    WorktreeInfo {
        path: format!("/worktrees/{branch}"),
        branch: Some(branch.into()),
        open_workspace_id: open.map(Into::into),
    }
}

#[test]
fn opening_a_checkout_uses_herdrs_worktree_open_and_runs_on_open_only_when_new() {
    for already_open in [false, true] {
        let tui = tui();
        tui.cli
            .worktree_open_results
            .lock()
            .unwrap()
            .push_back(Ok(open_response(already_open)));
        if !already_open {
            queue_successful_on_open(&tui.cli);
        }

        let opened = tui
            .open_checkout(Path::new("/repo"), Path::new("/repo"))
            .unwrap();

        assert_eq!(opened, Opened::default());
        assert_eq!(
            tui.cli.calls.lock().unwrap()[0],
            HerdrCall::WorktreeOpen {
                cwd: "/repo".into(),
                target: WorktreeOpenTarget::Path("/repo".into()),
                focus: true,
            }
        );
        assert_eq!(
            pane_call_count(&tui.cli),
            if already_open { 0 } else { 2 },
            "already_open={already_open}"
        );
    }
}

#[test]
fn opening_a_branch_with_a_checkout_runs_on_open_only_when_new() {
    for already_open in [false, true] {
        let tui = tui();
        tui.cli
            .worktree_open_results
            .lock()
            .unwrap()
            .push_back(Ok(open_response(already_open)));
        if !already_open {
            queue_successful_on_open(&tui.cli);
        }

        tui.open_branch(Path::new("/repo"), "main", KnownCheckout::Exists)
            .unwrap();

        assert_eq!(
            tui.cli.calls.lock().unwrap()[0],
            HerdrCall::WorktreeOpen {
                cwd: "/repo".into(),
                target: WorktreeOpenTarget::Branch("main".into()),
                focus: true,
            }
        );
        assert_eq!(
            pane_call_count(&tui.cli),
            if already_open { 0 } else { 2 },
            "already_open={already_open}"
        );
    }
}

#[test]
fn a_branch_without_a_checkout_is_created_without_base_or_path() {
    let tui = HerdrTui::new(MockHerdrCli::default(), OnOpenConfig::default());
    tui.cli
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));

    tui.open_branch(Path::new("/repo"), "feature", KnownCheckout::Missing)
        .unwrap();

    assert_eq!(
        *tui.cli.calls.lock().unwrap(),
        [HerdrCall::WorktreeCreate(WorktreeCreateRequest {
            cwd: "/repo".into(),
            branch: "feature".into(),
            base: None,
            path: None,
            focus: true,
        })]
    );
}

#[test]
fn a_new_branch_is_created_from_its_base_and_runs_on_open() {
    let tui = tui();
    tui.cli
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));
    queue_successful_on_open(&tui.cli);

    tui.create_branch(Path::new("/repo"), "feat/new", "main")
        .unwrap();

    assert_eq!(
        tui.cli.calls.lock().unwrap()[0],
        HerdrCall::WorktreeCreate(WorktreeCreateRequest {
            cwd: "/repo".into(),
            branch: "feat/new".into(),
            base: Some("main".into()),
            path: None,
            focus: true,
        })
    );
    assert_eq!(pane_call_count(&tui.cli), 2);
}

#[test]
fn on_open_failures_warn_without_replacing_open_success() {
    let tui = tui();
    tui.cli
        .worktree_open_results
        .lock()
        .unwrap()
        .push_back(Ok(open_response(false)));
    tui.cli.pane_open_results.lock().unwrap().extend([
        Err(super::super::PaneOpenError::Split(HerdrError::Other {
            code: "pane_split_failed".into(),
            message: "too small".into(),
        })),
        Err(super::super::PaneOpenError::Run(HerdrError::Other {
            code: "pane_run_failed".into(),
            message: "shell closed".into(),
        })),
    ]);

    let warning = tui
        .open_checkout(Path::new("/repo"), Path::new("/repo"))
        .unwrap()
        .warning
        .unwrap();

    assert!(warning.starts_with("on_open: "));
    assert!(warning.contains("pane 1 split failed"));
    assert!(warning.contains("pane 2 run failed"));
}

#[test]
fn a_stale_create_route_refreshes_worktrees_and_retries_open_once() {
    let tui = tui();
    tui.cli
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeCreateFailed(
            "feature is already checked out".into(),
        )));
    tui.cli
        .worktree_list_results
        .lock()
        .unwrap()
        .push_back(Ok(WorktreeListResponse {
            worktrees: vec![worktree("feature", Some("w_1"))],
        }));
    tui.cli
        .worktree_open_results
        .lock()
        .unwrap()
        .push_back(Ok(open_response(true)));

    tui.open_branch(Path::new("/repo"), "feature", KnownCheckout::Missing)
        .unwrap();

    let calls = tui.cli.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(matches!(calls[0], HerdrCall::WorktreeCreate(_)));
    assert!(matches!(calls[1], HerdrCall::WorktreeList { .. }));
    assert!(matches!(
        &calls[2],
        HerdrCall::WorktreeOpen {
            target: WorktreeOpenTarget::Branch(branch),
            ..
        } if branch == "feature"
    ));
}

#[test]
fn a_stale_open_route_refreshes_worktrees_and_retries_create_once() {
    let tui = HerdrTui::new(MockHerdrCli::default(), OnOpenConfig::default());
    tui.cli
        .worktree_open_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeNotFound("feature vanished".into())));
    tui.cli
        .worktree_list_results
        .lock()
        .unwrap()
        .push_back(Ok(WorktreeListResponse {
            worktrees: Vec::new(),
        }));
    tui.cli
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Ok(create_response()));

    tui.open_branch(Path::new("/repo"), "feature", KnownCheckout::Exists)
        .unwrap();

    let calls = tui.cli.calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(matches!(calls[0], HerdrCall::WorktreeOpen { .. }));
    assert!(matches!(calls[1], HerdrCall::WorktreeList { .. }));
    assert!(matches!(calls[2], HerdrCall::WorktreeCreate(_)));
}

#[test]
fn an_operation_already_in_progress_is_reported_in_plain_words() {
    let tui = tui();
    tui.cli
        .worktree_create_results
        .lock()
        .unwrap()
        .push_back(Err(HerdrError::WorktreeOperationInProgress(
            "raw details".into(),
        )));

    let message = tui
        .open_branch(Path::new("/repo"), "feature", KnownCheckout::Missing)
        .unwrap_err()
        .to_string();

    assert!(message.contains("already in progress"));
    assert!(!message.contains("worktree_operation_in_progress"));
}

#[test]
fn opening_a_folder_focuses_the_workspace_of_a_pane_in_it() {
    let tui = tui();
    tui.cli
        .pane_list_results
        .lock()
        .unwrap()
        .push_back(Ok(vec![PaneInfo {
            workspace_id: "w_folder".into(),
            cwd: Some("/folder".into()),
            foreground_cwd: Some("/unrelated".into()),
        }]));
    tui.cli
        .workspace_focus_results
        .lock()
        .unwrap()
        .push_back(Ok(()));

    let opened = tui.open_folder(Path::new("/folder")).unwrap();

    assert_eq!(opened, Opened::default());
    assert_eq!(
        *tui.cli.calls.lock().unwrap(),
        [
            HerdrCall::PaneList,
            HerdrCall::WorkspaceFocus {
                workspace_id: "w_folder".into(),
            },
        ]
    );
}

#[test]
fn opening_an_unmatched_folder_creates_a_focused_workspace_without_on_open() {
    let tui = tui();
    tui.cli
        .pane_list_results
        .lock()
        .unwrap()
        .push_back(Ok(Vec::new()));
    tui.cli
        .workspace_create_results
        .lock()
        .unwrap()
        .push_back(Ok(WorkspaceCreateResponse {
            workspace_id: Some("w_new".into()),
            warning: None,
        }));

    tui.open_folder(Path::new("/folder")).unwrap();

    assert_eq!(
        *tui.cli.calls.lock().unwrap(),
        [
            HerdrCall::PaneList,
            HerdrCall::WorkspaceCreate {
                cwd: "/folder".into(),
                focus: true,
            },
        ]
    );
}

#[test]
fn open_state_comes_from_herdrs_workspace_pane_and_worktree_lists() {
    let tui = tui();
    tui.cli
        .workspace_list_results
        .lock()
        .unwrap()
        .push_back(Ok(vec![
            workspace("w_1", None),
            workspace("w_2", Some("/repo")),
        ]));
    tui.cli.pane_list_results.lock().unwrap().push_back(Ok(vec![
        PaneInfo {
            workspace_id: "w_1".into(),
            cwd: Some("/folder".into()),
            foreground_cwd: Some("/elsewhere".into()),
        },
        PaneInfo {
            workspace_id: "w_2".into(),
            cwd: None,
            foreground_cwd: None,
        },
    ]));
    tui.cli
        .worktree_list_results
        .lock()
        .unwrap()
        .push_back(Ok(WorktreeListResponse {
            worktrees: vec![worktree("open", Some("w_3")), worktree("closed", None)],
        }));

    assert_eq!(tui.open_repos().unwrap(), [PathBuf::from("/repo")]);
    assert_eq!(tui.open_folders().unwrap(), [PathBuf::from("/folder")]);
    assert_eq!(
        tui.checkouts(Path::new("/repo")).unwrap(),
        [
            CheckoutState {
                path: "/worktrees/open".into(),
                session: Some(CheckoutWorkspace {
                    workspace: WorkspaceId("w_3".into()),
                    repo: "/repo".into(),
                    checkout: "/worktrees/open".into(),
                }),
            },
            CheckoutState {
                path: "/worktrees/closed".into(),
                session: None,
            },
        ]
    );
}

#[test]
fn removal_asks_for_force_when_herdr_refuses_a_dirty_checkout() {
    let tui = tui();
    tui.cli.worktree_remove_results.lock().unwrap().extend([
        Err(HerdrError::DirtyWorktreeRequiresForce("dirty".into())),
        Err(HerdrError::WorktreeNotFound("gone".into())),
        Ok(WorktreeRemoveResponse {
            warning: Some("note".into()),
        }),
    ]);
    let workspace = checkout_workspace("w_1");

    assert!(matches!(
        tui.remove(&workspace, false),
        Err(RemoveError::DirtyRequiresForce(_))
    ));
    assert!(matches!(
        tui.remove(&workspace, false),
        Err(RemoveError::Failed(HerdrError::WorktreeNotFound(_)))
    ));
    assert_eq!(
        tui.remove(&workspace, true).unwrap().warning.as_deref(),
        Some("note")
    );
    assert_eq!(
        tui.cli.calls.lock().unwrap()[2],
        HerdrCall::WorktreeRemove {
            workspace_id: "w_1".into(),
            force: true,
        }
    );
}

fn adoption_call() -> HerdrCall {
    HerdrCall::WorktreeOpen {
        cwd: "/repo".into(),
        target: WorktreeOpenTarget::Path("/worktrees/feature".into()),
        focus: false,
    }
}

fn remove_call(force: bool) -> HerdrCall {
    HerdrCall::WorktreeRemove {
        workspace_id: "w_1".into(),
        force,
    }
}

fn not_herdr_managed() -> HerdrError {
    HerdrError::NotLinkedWorktree("workspace is not a Herdr-managed worktree checkout".into())
}

/// Queues herdr refusing to remove the checkout of plain workspace `w_1`, then taking
/// it over with `adopted`.
fn queue_refusal_of_plain_workspace(
    cli: &MockHerdrCli,
    adopted: Result<WorktreeOpenResponse, HerdrError>,
) {
    cli.worktree_remove_results
        .lock()
        .unwrap()
        .push_back(Err(not_herdr_managed()));
    cli.workspace_list_results
        .lock()
        .unwrap()
        .push_back(Ok(vec![
            workspace("w_0", Some("/repo")),
            workspace("w_1", None),
        ]));
    cli.worktree_open_results.lock().unwrap().push_back(adopted);
}

#[test]
fn removing_the_checkout_of_a_plain_workspace_has_herdr_take_it_over_first() {
    for force in [false, true] {
        let tui = tui();
        queue_refusal_of_plain_workspace(&tui.cli, Ok(open_response(true)));
        tui.cli
            .worktree_remove_results
            .lock()
            .unwrap()
            .push_back(Ok(WorktreeRemoveResponse { warning: None }));

        assert_eq!(
            tui.remove(&checkout_workspace("w_1"), force),
            Ok(Removed::default())
        );
        assert_eq!(
            *tui.cli.calls.lock().unwrap(),
            [
                remove_call(force),
                HerdrCall::WorkspaceList,
                adoption_call(),
                remove_call(force),
            ],
            "force={force}"
        );
    }
}

#[test]
fn a_taken_over_dirty_checkout_still_needs_force_and_stays_taken_over() {
    let tui = tui();
    queue_refusal_of_plain_workspace(&tui.cli, Ok(open_response(true)));
    tui.cli.worktree_remove_results.lock().unwrap().extend([
        Err(HerdrError::DirtyWorktreeRequiresForce("dirty".into())),
        Ok(WorktreeRemoveResponse { warning: None }),
    ]);
    let session = checkout_workspace("w_1");

    assert!(matches!(
        tui.remove(&session, false),
        Err(RemoveError::DirtyRequiresForce(
            HerdrError::DirtyWorktreeRequiresForce(_)
        ))
    ));
    assert_eq!(tui.remove(&session, true), Ok(Removed::default()));
    assert_eq!(
        *tui.cli.calls.lock().unwrap(),
        [
            remove_call(false),
            HerdrCall::WorkspaceList,
            adoption_call(),
            remove_call(false),
            remove_call(true),
        ]
    );
}

#[test]
fn a_refusal_for_a_herdr_managed_workspace_is_reported_without_taking_it_over() {
    for listed in [vec![workspace("w_1", Some("/repo"))], Vec::new()] {
        let tui = tui();
        tui.cli
            .worktree_remove_results
            .lock()
            .unwrap()
            .push_back(Err(not_herdr_managed()));
        tui.cli
            .workspace_list_results
            .lock()
            .unwrap()
            .push_back(Ok(listed));

        assert_eq!(
            tui.remove(&checkout_workspace("w_1"), false),
            Err(RemoveError::Failed(not_herdr_managed()))
        );
        assert_eq!(
            *tui.cli.calls.lock().unwrap(),
            [remove_call(false), HerdrCall::WorkspaceList]
        );
    }
}

#[test]
fn removal_stops_when_herdr_does_not_take_over_the_plain_workspace() {
    let other_workspace = WorktreeOpenResponse {
        workspace_id: Some("w_2".into()),
        ..open_response(true)
    };
    let unnamed_workspace = WorktreeOpenResponse {
        workspace_id: None,
        ..open_response(true)
    };
    let undecodable = WorktreeOpenResponse {
        opened: None,
        workspace_id: None,
        already_open: None,
        warning: Some("unexpected response".into()),
    };
    let cases = [
        (Ok(other_workspace), "took over workspace w_2"),
        (Ok(open_response(false)), "in a new workspace"),
        (Ok(unnamed_workspace), "did not say which workspace"),
        (Ok(undecodable), "unexpected response"),
        (
            Err(HerdrError::WorktreeOpenFailed("no tab".into())),
            "worktree_open_failed: no tab",
        ),
    ];
    for (adopted, expected) in cases {
        let tui = tui();
        queue_refusal_of_plain_workspace(&tui.cli, adopted);

        let Err(RemoveError::Failed(error)) = tui.remove(&checkout_workspace("w_1"), true) else {
            panic!("removal should fail for {expected}");
        };
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(
            *tui.cli.calls.lock().unwrap(),
            [remove_call(true), HerdrCall::WorkspaceList, adoption_call()],
            "{expected}"
        );
    }
}

#[test]
fn removal_is_not_retried_when_herdr_refuses_again_after_taking_over() {
    let tui = tui();
    queue_refusal_of_plain_workspace(&tui.cli, Ok(open_response(true)));
    tui.cli
        .worktree_remove_results
        .lock()
        .unwrap()
        .push_back(Err(not_herdr_managed()));

    assert_eq!(
        tui.remove(&checkout_workspace("w_1"), false),
        Err(RemoveError::Failed(not_herdr_managed()))
    );
    assert_eq!(tui.cli.calls.lock().unwrap().len(), 4);
}
