//! The herdr TUI as a kiosk backend. herdr manages worktrees itself and matches
//! checkouts to workspaces on its side, so most operations are one CLI call.

use std::path::{Path, PathBuf};

use kiosk_core::backend::{
    Backend, CheckoutState, KnownCheckout, NativeWorktrees, Opened, RemoveError, Removed,
    Worktrees, identity::LiveFolder,
};

use super::{
    HerdrCli, HerdrError, OpenedWorktree, WorktreeCreateRequest, WorktreeOpenTarget,
    focus_or_create_folder, on_open,
};
use kiosk_core::config::OnOpenConfig;

/// A herdr workspace id. herdr recycles ids, so one only identifies a workspace for as
/// long as kiosk is running.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceId(pub String);

pub struct HerdrTui<C: HerdrCli> {
    cli: C,
    on_open: OnOpenConfig,
}

impl<C: HerdrCli> HerdrTui<C> {
    pub fn new(cli: C, on_open: OnOpenConfig) -> Self {
        Self { cli, on_open }
    }

    /// Runs `[on_open]` in a workspace herdr has just opened for `opened`.
    fn finish_open(
        &self,
        opened: Option<&OpenedWorktree>,
        warning: Option<String>,
        newly_opened: bool,
    ) -> Opened {
        let on_open_warning = opened.filter(|_| newly_opened).and_then(|opened| {
            on_open::apply(
                &self.on_open,
                &opened.root_pane_id,
                Path::new(&opened.path),
                |request, command| self.cli.pane_open(request, command),
            )
        });
        Opened {
            warning: on_open::combine_warnings(warning, on_open_warning),
        }
    }

    /// Opens or creates `branch`'s checkout. The route comes from the picker's listing,
    /// which can be stale: when herdr says the route was wrong, its own listing decides,
    /// and the other route is tried once.
    fn open_branch_with_retry(
        &self,
        repo: &Path,
        branch: &str,
        route: KnownCheckout,
    ) -> Result<Opened, HerdrError> {
        match self.run_branch_route(repo, branch, route) {
            Err(error) if is_wrong_route_failure(route, &error) => {
                let refreshed = self.cli.worktree_list(repo).ok().map(|response| {
                    if response
                        .worktrees
                        .iter()
                        .any(|worktree| worktree.branch.as_deref() == Some(branch))
                    {
                        KnownCheckout::Exists
                    } else {
                        KnownCheckout::Missing
                    }
                });
                match refreshed.filter(|refreshed| *refreshed != route) {
                    Some(refreshed) => self.run_branch_route(repo, branch, refreshed),
                    None => Err(error),
                }
            }
            result => result,
        }
    }

    fn run_branch_route(
        &self,
        repo: &Path,
        branch: &str,
        route: KnownCheckout,
    ) -> Result<Opened, HerdrError> {
        match route {
            KnownCheckout::Exists => {
                let response = self.cli.worktree_open(
                    repo,
                    &WorktreeOpenTarget::Branch(branch.to_string()),
                    true,
                )?;
                Ok(self.finish_open(
                    response.opened.as_ref(),
                    response.warning,
                    response.already_open == Some(false),
                ))
            }
            KnownCheckout::Missing => self.create(repo, branch, None),
        }
    }

    fn create(&self, repo: &Path, branch: &str, base: Option<&str>) -> Result<Opened, HerdrError> {
        let response = self.cli.worktree_create(&WorktreeCreateRequest {
            cwd: repo.to_path_buf(),
            branch: branch.to_string(),
            base: base.map(str::to_string),
            path: None,
            focus: true,
        })?;
        Ok(self.finish_open(response.opened.as_ref(), response.warning, true))
    }
}

fn is_wrong_route_failure(route: KnownCheckout, error: &HerdrError) -> bool {
    match (route, error) {
        (KnownCheckout::Missing, HerdrError::WorktreeCreateFailed(message)) => {
            message.to_ascii_lowercase().contains("already checked out")
        }
        (KnownCheckout::Exists, HerdrError::WorktreeNotFound(_)) => true,
        (KnownCheckout::Exists, HerdrError::WorktreeOpenFailed(message)) => {
            let message = message.to_ascii_lowercase();
            message.contains("not found") || message.contains("no worktree")
        }
        _ => false,
    }
}

impl<C: HerdrCli> Backend for HerdrTui<C> {
    type SessionId = WorkspaceId;
    type Error = HerdrError;

    fn open_repos(&self) -> Result<Vec<PathBuf>, HerdrError> {
        Ok(self
            .cli
            .workspace_list()?
            .into_iter()
            .filter_map(|workspace| workspace.worktree)
            .map(|worktree| PathBuf::from(worktree.repo_root))
            .collect())
    }

    fn open_folders(&self) -> Result<Vec<PathBuf>, HerdrError> {
        Ok(self
            .cli
            .pane_list()?
            .into_iter()
            .filter_map(|pane| pane.cwd.map(PathBuf::from))
            .collect())
    }

    fn checkouts(&self, repo: &Path) -> Result<Vec<CheckoutState<WorkspaceId>>, HerdrError> {
        Ok(self
            .cli
            .worktree_list(repo)?
            .worktrees
            .into_iter()
            .map(|worktree| CheckoutState {
                path: worktree.path.into(),
                session: worktree.open_workspace_id.map(WorkspaceId),
            })
            .collect())
    }

    fn open_checkout(&self, repo: &Path, checkout: &Path) -> Result<Opened, HerdrError> {
        let response =
            self.cli
                .worktree_open(repo, &WorktreeOpenTarget::Path(checkout.into()), true)?;
        Ok(self.finish_open(
            response.opened.as_ref(),
            response.warning,
            response.already_open == Some(false),
        ))
    }

    fn open_folder(&self, folder: &Path) -> Result<Opened, HerdrError> {
        let sessions: Vec<_> = self
            .cli
            .pane_list()?
            .into_iter()
            .filter_map(|pane| {
                Some(LiveFolder {
                    session: pane.workspace_id,
                    folder: pane.cwd?.into(),
                })
            })
            .collect();
        let warning = focus_or_create_folder(
            &sessions,
            folder,
            |workspace| self.cli.workspace_focus(workspace),
            || {
                self.cli
                    .workspace_create(folder, true)
                    .map(|response| response.warning)
            },
        )?;
        Ok(Opened { warning })
    }

    fn notify(&self, title: &str, body: &str) -> Result<(), HerdrError> {
        self.cli.notification_show(title, body)
    }

    fn worktrees(&self) -> Worktrees<'_, Self> {
        Worktrees::Native(self)
    }
}

impl<C: HerdrCli> NativeWorktrees<Self> for HerdrTui<C> {
    fn open_branch(
        &self,
        repo: &Path,
        branch: &str,
        known: KnownCheckout,
    ) -> Result<Opened, HerdrError> {
        self.open_branch_with_retry(repo, branch, known)
    }

    fn create_branch(&self, repo: &Path, branch: &str, base: &str) -> Result<Opened, HerdrError> {
        self.create(repo, branch, Some(base))
    }

    fn remove(
        &self,
        workspace: &WorkspaceId,
        force: bool,
    ) -> Result<Removed, RemoveError<HerdrError>> {
        self.cli
            .worktree_remove(&workspace.0, force)
            .map(|response| Removed {
                warning: response.warning,
            })
            .map_err(|error| match error {
                HerdrError::DirtyWorktreeRequiresForce(_) => RemoveError::DirtyRequiresForce(error),
                error => RemoveError::Failed(error),
            })
    }
}

#[cfg(test)]
mod tests;
