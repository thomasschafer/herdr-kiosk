use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, bail};

use super::{GitProvider, LocalBranchAlreadyExists, Repo, RepoScan, ScanWarning, Worktree};

#[derive(Default)]
pub struct MockGitProvider {
    pub repos: Vec<Repo>,
    pub scan_warnings: Vec<ScanWarning>,
    pub branches: Vec<String>,
    pub remote_branches: Vec<String>,
    pub remote_branches_by_remote: HashMap<String, Vec<String>>,
    pub worktrees: Vec<Worktree>,
    pub remotes: Vec<String>,
    pub default_branch: Option<String>,
    pub failure: Mutex<Option<String>>,
    pub tracking_already_exists: AtomicBool,
    pub tracking_calls: Mutex<Vec<(PathBuf, String, String)>>,
    pub remove_calls: Mutex<Vec<(PathBuf, PathBuf)>>,
    pub prune_calls: Mutex<Vec<PathBuf>>,
    pub fetch_calls: Mutex<Vec<(PathBuf, String)>>,
}

impl MockGitProvider {
    fn check_failure(&self) -> Result<()> {
        if let Some(message) = self.failure.lock().unwrap().take() {
            bail!(message);
        }
        Ok(())
    }
}

impl GitProvider for MockGitProvider {
    fn scan_repos(&self, _dirs: &[(PathBuf, u16)]) -> Result<RepoScan> {
        self.check_failure()?;
        Ok(RepoScan {
            repos: self
                .repos
                .iter()
                .cloned()
                .map(|repo| Repo {
                    worktrees: Vec::new(),
                    ..repo
                })
                .collect(),
            warnings: self.scan_warnings.clone(),
        })
    }

    fn discover_repos(&self, _dirs: &[(PathBuf, u16)]) -> Result<RepoScan> {
        self.check_failure()?;
        Ok(RepoScan {
            repos: self.repos.clone(),
            warnings: self.scan_warnings.clone(),
        })
    }

    fn list_branches(&self, _repo_path: &Path) -> Result<Vec<String>> {
        self.check_failure()?;
        Ok(self.branches.clone())
    }

    fn list_remote_branches(&self, _repo_path: &Path) -> Result<Vec<String>> {
        self.check_failure()?;
        Ok(self.remote_branches.clone())
    }

    fn list_remote_branches_for_remote(
        &self,
        _repo_path: &Path,
        remote: &str,
    ) -> Result<Vec<String>> {
        self.check_failure()?;
        Ok(self
            .remote_branches_by_remote
            .get(remote)
            .cloned()
            .unwrap_or_else(|| self.remote_branches.clone()))
    }

    fn list_worktrees(&self, _repo_path: &Path) -> Result<Vec<Worktree>> {
        self.check_failure()?;
        Ok(self.worktrees.clone())
    }

    fn list_remotes(&self, _repo_path: &Path) -> Result<Vec<String>> {
        self.check_failure()?;
        Ok(self.remotes.clone())
    }

    fn fetch_remote(&self, repo_path: &Path, remote: &str) -> Result<()> {
        self.check_failure()?;
        self.fetch_calls
            .lock()
            .unwrap()
            .push((repo_path.to_path_buf(), remote.to_string()));
        Ok(())
    }

    fn create_tracking_branch(&self, repo_path: &Path, branch: &str, remote: &str) -> Result<()> {
        self.check_failure()?;
        self.tracking_calls.lock().unwrap().push((
            repo_path.to_path_buf(),
            branch.to_string(),
            remote.to_string(),
        ));
        if self.tracking_already_exists.swap(false, Ordering::AcqRel) {
            return Err(LocalBranchAlreadyExists::new(branch).into());
        }
        Ok(())
    }

    fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path) -> Result<()> {
        self.check_failure()?;
        self.remove_calls
            .lock()
            .unwrap()
            .push((repo_path.to_path_buf(), worktree_path.to_path_buf()));
        Ok(())
    }

    fn prune_worktrees(&self, repo_path: &Path) -> Result<()> {
        self.check_failure()?;
        self.prune_calls
            .lock()
            .unwrap()
            .push(repo_path.to_path_buf());
        Ok(())
    }

    fn default_branch(
        &self,
        _repo_path: &Path,
        _local_branches: &[String],
    ) -> Result<Option<String>> {
        self.check_failure()?;
        Ok(self.default_branch.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_remote_for_tracking_branch() {
        let provider = MockGitProvider::default();
        provider
            .create_tracking_branch(Path::new("/repo"), "feature", "upstream")
            .unwrap();
        assert_eq!(
            *provider.tracking_calls.lock().unwrap(),
            [(PathBuf::from("/repo"), "feature".into(), "upstream".into())]
        );
    }
}
