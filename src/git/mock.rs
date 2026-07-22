use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Result, bail};

use super::{
    DirtyWorktreeRequiresForce, GitProvider, Listed, LocalBranchAlreadyExists, Repo, ScanWarning,
    Worktree,
};

#[derive(Default)]
pub struct MockGitProvider {
    pub repos: Vec<Repo>,
    pub scan_warnings: Vec<ScanWarning>,
    pub branches: Vec<String>,
    pub remote_branches: Vec<String>,
    pub remote_branches_by_remote: HashMap<String, Vec<String>>,
    pub worktrees: Vec<Worktree>,
    pub list_worktree_calls: Mutex<Vec<PathBuf>>,
    pub remotes: Vec<String>,
    pub default_branch: Option<String>,
    pub failure: Mutex<Option<String>>,
    pub prune_failure: Mutex<Option<String>>,
    pub tracking_already_exists: AtomicBool,
    pub tracking_calls: Mutex<Vec<(PathBuf, String, String)>>,
    pub invalid_branch_names: HashSet<String>,
    pub validation_calls: Mutex<Vec<(PathBuf, String)>>,
    pub dirty_remove_once: AtomicBool,
    pub remove_calls: Mutex<Vec<(PathBuf, PathBuf, bool)>>,
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
    fn scan_repos_streaming(
        &self,
        _dir: &Path,
        _depth: u16,
        is_cancelled: &dyn Fn() -> bool,
        on_found: &dyn Fn(Repo),
    ) -> Result<Vec<ScanWarning>> {
        self.check_failure()?;
        for repo in &self.repos {
            if is_cancelled() {
                break;
            }
            on_found(Repo {
                worktrees: Vec::new(),
                ..repo.clone()
            });
        }
        Ok(self.scan_warnings.clone())
    }

    fn list_branches(&self, _repo_path: &Path) -> Result<Listed<String>> {
        self.check_failure()?;
        Ok(Listed::new(self.branches.clone(), false))
    }

    fn list_remote_branches_for_remote(
        &self,
        _repo_path: &Path,
        remote: &str,
    ) -> Result<Listed<String>> {
        self.check_failure()?;
        Ok(Listed::new(
            self.remote_branches_by_remote
                .get(remote)
                .cloned()
                .unwrap_or_else(|| self.remote_branches.clone()),
            false,
        ))
    }

    fn list_worktrees(&self, repo_path: &Path) -> Result<Listed<Worktree>> {
        self.check_failure()?;
        self.list_worktree_calls
            .lock()
            .unwrap()
            .push(repo_path.to_path_buf());
        Ok(Listed::new(self.worktrees.clone(), false))
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

    fn is_valid_branch_name(&self, repo_path: &Path, branch: &str) -> Result<bool> {
        self.check_failure()?;
        self.validation_calls
            .lock()
            .unwrap()
            .push((repo_path.to_path_buf(), branch.to_string()));
        Ok(!self.invalid_branch_names.contains(branch))
    }

    fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
        self.check_failure()?;
        self.remove_calls.lock().unwrap().push((
            repo_path.to_path_buf(),
            worktree_path.to_path_buf(),
            force,
        ));
        if !force && self.dirty_remove_once.swap(false, Ordering::AcqRel) {
            return Err(DirtyWorktreeRequiresForce.into());
        }
        Ok(())
    }

    fn prune_worktrees(&self, repo_path: &Path) -> Result<()> {
        self.check_failure()?;
        if let Some(message) = self.prune_failure.lock().unwrap().take() {
            bail!(message);
        }
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
