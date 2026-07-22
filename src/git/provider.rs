use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use anyhow::Result;

use super::{Repo, RepoScan, ScanWarning, Worktree};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalBranchAlreadyExists {
    branch: String,
}

impl LocalBranchAlreadyExists {
    pub fn new(branch: impl Into<String>) -> Self {
        Self {
            branch: branch.into(),
        }
    }
}

impl fmt::Display for LocalBranchAlreadyExists {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "local branch '{}' already exists", self.branch)
    }
}

impl Error for LocalBranchAlreadyExists {}

pub fn is_local_branch_already_exists(error: &anyhow::Error) -> bool {
    error.downcast_ref::<LocalBranchAlreadyExists>().is_some()
}

pub trait GitProvider: Send + Sync {
    fn scan_repos(&self, dirs: &[(PathBuf, u16)]) -> Result<RepoScan>;

    fn scan_repos_streaming(
        &self,
        dir: &Path,
        depth: u16,
        on_found: &dyn Fn(Repo),
    ) -> Result<Vec<ScanWarning>> {
        let scan = self.scan_repos(&[(dir.to_path_buf(), depth)])?;
        for repo in scan.repos {
            on_found(repo);
        }
        Ok(scan.warnings)
    }

    fn discover_repos(&self, dirs: &[(PathBuf, u16)]) -> Result<RepoScan>;
    fn list_branches(&self, repo_path: &Path) -> Result<Vec<String>>;
    fn list_remote_branches(&self, repo_path: &Path) -> Result<Vec<String>>;
    fn list_remote_branches_for_remote(
        &self,
        repo_path: &Path,
        remote: &str,
    ) -> Result<Vec<String>>;
    fn list_worktrees(&self, repo_path: &Path) -> Result<Vec<Worktree>>;
    fn list_remotes(&self, repo_path: &Path) -> Result<Vec<String>>;
    fn fetch_remote(&self, repo_path: &Path, remote: &str) -> Result<()>;
    fn create_tracking_branch(&self, repo_path: &Path, branch: &str, remote: &str) -> Result<()>;
    fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path) -> Result<()>;
    fn prune_worktrees(&self, repo_path: &Path) -> Result<()>;
    fn default_branch(&self, repo_path: &Path, local_branches: &[String])
    -> Result<Option<String>>;
}
