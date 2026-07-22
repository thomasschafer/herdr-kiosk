mod cli;
pub mod mock;
mod provider;
mod repo;

pub use cli::{CliGitProvider, walk_repos};
pub use provider::{
    DirtyWorktreeRequiresForce, GitProvider, LocalBranchAlreadyExists,
    is_dirty_worktree_requires_force, is_local_branch_already_exists,
};
pub use repo::{Repo, RepoScan, ScanWarning, Worktree};

use anyhow::{Context, Result};

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::{ffi::OsString, path::PathBuf};

/// Parse `git worktree list --porcelain -z` output into worktrees.
pub fn parse_worktree_porcelain(output: &[u8]) -> Result<Vec<Worktree>> {
    let mut worktrees = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;
    let mut is_first = true;

    for field in output.split(|byte| *byte == 0) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            #[cfg(unix)]
            let path = PathBuf::from(OsString::from_vec(path.to_vec()));
            #[cfg(not(unix))]
            let path = PathBuf::from(OsString::from(
                std::str::from_utf8(path).context("git returned a non-UTF-8 worktree path")?,
            ));
            if let Some(previous_path) = current_path.replace(path) {
                worktrees.push(Worktree {
                    path: previous_path,
                    branch: current_branch.take(),
                    is_main: is_first,
                });
                is_first = false;
            }
        } else if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
            current_branch = Some(
                std::str::from_utf8(branch)
                    .context("git returned a non-UTF-8 branch name")?
                    .to_string(),
            );
        } else if field.is_empty() {
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch.take(),
                    is_main: is_first,
                });
                is_first = false;
            }
            current_branch = None;
        }
    }

    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
            is_main: is_first,
        });
    }

    Ok(worktrees)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parses_multiple_worktrees() {
        let output = b"worktree /home/user/project\0HEAD abc123\0branch refs/heads/main\0\0worktree /home/user/project-feat\0HEAD def456\0branch refs/heads/feat/thing\0\0";
        let worktrees = parse_worktree_porcelain(output).unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/project"));
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[1].branch.as_deref(), Some("feat/thing"));
        assert!(!worktrees[1].is_main);
    }

    #[test]
    fn parses_detached_and_unterminated_entries() {
        let detached =
            parse_worktree_porcelain(b"worktree /home/user/project\0HEAD abc123\0detached\0\0")
                .unwrap();
        assert_eq!(detached.len(), 1);
        assert!(detached[0].branch.is_none());

        let unterminated =
            parse_worktree_porcelain(b"worktree /home/user/project\0branch refs/heads/main")
                .unwrap();
        assert_eq!(unterminated[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn empty_porcelain_has_no_worktrees() {
        assert!(parse_worktree_porcelain(b"").unwrap().is_empty());
    }

    #[test]
    fn preserves_newlines_in_worktree_paths() {
        let worktrees = parse_worktree_porcelain(
            b"worktree /home/user/project\nfeature\0HEAD abc123\0branch refs/heads/feature\0\0",
        )
        .unwrap();

        assert_eq!(worktrees.len(), 1);
        assert_eq!(
            worktrees[0].path,
            PathBuf::from("/home/user/project\nfeature")
        );
        assert_eq!(worktrees[0].branch.as_deref(), Some("feature"));
    }
}
