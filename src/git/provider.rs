use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{Repo, RepoScan, ScanWarning, Worktree};

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
