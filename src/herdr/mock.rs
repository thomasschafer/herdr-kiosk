use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Mutex,
};

use super::{
    HerdrError, HerdrProvider, PaneInfo, PaneRunResponse, PaneSplitRequest, PaneSplitResponse,
    WorkspaceCreateResponse, WorkspaceInfo, WorktreeCreateRequest, WorktreeCreateResponse,
    WorktreeListResponse, WorktreeOpenResponse, WorktreeOpenTarget, WorktreeRemoveResponse,
};

#[derive(Debug, Clone, PartialEq)]
pub enum HerdrCall {
    WorktreeOpen {
        cwd: PathBuf,
        target: WorktreeOpenTarget,
        focus: bool,
    },
    WorktreeCreate(WorktreeCreateRequest),
    WorktreeRemove {
        workspace_id: String,
        force: bool,
    },
    WorktreeList {
        cwd: PathBuf,
    },
    WorkspaceList,
    PaneList,
    WorkspaceCreate {
        cwd: PathBuf,
        focus: bool,
    },
    WorkspaceFocus {
        workspace_id: String,
    },
    PaneSplit(PaneSplitRequest),
    PaneRun {
        pane_id: String,
        command: String,
    },
    NotificationShow {
        title: String,
        body: String,
    },
}

#[derive(Default)]
pub struct MockHerdrProvider {
    pub calls: Mutex<Vec<HerdrCall>>,
    pub worktree_open_results: Mutex<VecDeque<Result<WorktreeOpenResponse, HerdrError>>>,
    pub worktree_create_results: Mutex<VecDeque<Result<WorktreeCreateResponse, HerdrError>>>,
    pub worktree_remove_results: Mutex<VecDeque<Result<WorktreeRemoveResponse, HerdrError>>>,
    pub worktree_list_results: Mutex<VecDeque<Result<WorktreeListResponse, HerdrError>>>,
    pub workspace_list_results: Mutex<VecDeque<Result<Vec<WorkspaceInfo>, HerdrError>>>,
    pub pane_list_results: Mutex<VecDeque<Result<Vec<PaneInfo>, HerdrError>>>,
    pub workspace_create_results: Mutex<VecDeque<Result<WorkspaceCreateResponse, HerdrError>>>,
    pub workspace_focus_results: Mutex<VecDeque<Result<(), HerdrError>>>,
    pub pane_split_results: Mutex<VecDeque<Result<PaneSplitResponse, HerdrError>>>,
    pub pane_run_results: Mutex<VecDeque<Result<PaneRunResponse, HerdrError>>>,
    pub notification_show_results: Mutex<VecDeque<Result<(), HerdrError>>>,
}

fn next<T>(queue: &Mutex<VecDeque<Result<T, HerdrError>>>, method: &str) -> Result<T, HerdrError> {
    queue.lock().unwrap().pop_front().unwrap_or_else(|| {
        Err(HerdrError::InvalidResponse(format!(
            "mock has no queued result for {method}"
        )))
    })
}

impl HerdrProvider for MockHerdrProvider {
    fn worktree_open(
        &self,
        cwd: &Path,
        target: &WorktreeOpenTarget,
        focus: bool,
    ) -> Result<WorktreeOpenResponse, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::WorktreeOpen {
            cwd: cwd.to_path_buf(),
            target: target.clone(),
            focus,
        });
        next(&self.worktree_open_results, "worktree_open")
    }

    fn worktree_create(
        &self,
        request: &WorktreeCreateRequest,
    ) -> Result<WorktreeCreateResponse, HerdrError> {
        self.calls
            .lock()
            .unwrap()
            .push(HerdrCall::WorktreeCreate(request.clone()));
        next(&self.worktree_create_results, "worktree_create")
    }

    fn worktree_remove(
        &self,
        workspace_id: &str,
        force: bool,
    ) -> Result<WorktreeRemoveResponse, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::WorktreeRemove {
            workspace_id: workspace_id.to_string(),
            force,
        });
        next(&self.worktree_remove_results, "worktree_remove")
    }

    fn worktree_list(&self, cwd: &Path) -> Result<WorktreeListResponse, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::WorktreeList {
            cwd: cwd.to_path_buf(),
        });
        next(&self.worktree_list_results, "worktree_list")
    }

    fn workspace_list(&self) -> Result<Vec<WorkspaceInfo>, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::WorkspaceList);
        next(&self.workspace_list_results, "workspace_list")
    }

    fn pane_list(&self) -> Result<Vec<PaneInfo>, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::PaneList);
        next(&self.pane_list_results, "pane_list")
    }

    fn workspace_create(
        &self,
        cwd: &Path,
        focus: bool,
    ) -> Result<WorkspaceCreateResponse, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::WorkspaceCreate {
            cwd: cwd.to_path_buf(),
            focus,
        });
        next(&self.workspace_create_results, "workspace_create")
    }

    fn workspace_focus(&self, workspace_id: &str) -> Result<(), HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::WorkspaceFocus {
            workspace_id: workspace_id.into(),
        });
        next(&self.workspace_focus_results, "workspace_focus")
    }

    fn pane_split(&self, request: &PaneSplitRequest) -> Result<PaneSplitResponse, HerdrError> {
        self.calls
            .lock()
            .unwrap()
            .push(HerdrCall::PaneSplit(request.clone()));
        next(&self.pane_split_results, "pane_split")
    }

    fn pane_run(&self, pane_id: &str, command: &str) -> Result<PaneRunResponse, HerdrError> {
        self.calls.lock().unwrap().push(HerdrCall::PaneRun {
            pane_id: pane_id.into(),
            command: command.into(),
        });
        next(&self.pane_run_results, "pane_run")
    }

    fn notification_show(&self, title: &str, body: &str) -> Result<(), HerdrError> {
        self.calls
            .lock()
            .unwrap()
            .push(HerdrCall::NotificationShow {
                title: title.into(),
                body: body.into(),
            });
        next(&self.notification_show_results, "notification_show")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_records_calls_and_returns_queued_errors() {
        let mock = MockHerdrProvider::default();
        mock.worktree_open_results
            .lock()
            .unwrap()
            .push_back(Err(HerdrError::WorktreeNotFound("missing".into())));

        let result = mock.worktree_open(
            Path::new("/repo"),
            &WorktreeOpenTarget::Branch("feature".into()),
            true,
        );
        assert!(matches!(result, Err(HerdrError::WorktreeNotFound(_))));
        assert_eq!(
            *mock.calls.lock().unwrap(),
            [HerdrCall::WorktreeOpen {
                cwd: PathBuf::from("/repo"),
                target: WorktreeOpenTarget::Branch("feature".into()),
                focus: true,
            }]
        );
    }

    #[test]
    fn unconfigured_mock_fails_loudly() {
        let error = MockHerdrProvider::default().workspace_list().unwrap_err();
        assert!(matches!(error, HerdrError::InvalidResponse(_)));
    }
}
