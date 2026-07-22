mod cli;
pub mod mock;
mod provider;
mod repo;

pub use cli::CliGitProvider;
pub use provider::{
    DirtyWorktreeRequiresForce, GitProvider, LocalBranchAlreadyExists,
    is_dirty_worktree_requires_force, is_local_branch_already_exists,
};
pub use repo::{Repo, ScanWarning, Worktree};

#[cfg(not(unix))]
use anyhow::Context;
use anyhow::Result;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed<T> {
    pub items: Vec<T>,
    pub skipped_unsupported_refs: bool,
}

impl<T> Listed<T> {
    pub(crate) fn new(items: Vec<T>, skipped_unsupported_refs: bool) -> Self {
        Self {
            items,
            skipped_unsupported_refs,
        }
    }
}

impl<T> std::ops::Deref for Listed<T> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        &self.items
    }
}

pub const UNSUPPORTED_REF_WARNING: &str =
    "Some Git refs were skipped because non-UTF-8 ref names are unsupported.";

/// Parse `git worktree list --porcelain -z` output into worktrees.
pub fn parse_worktree_porcelain(output: &[u8]) -> Result<Listed<Worktree>> {
    let mut worktrees = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;
    let mut skipped_unsupported_refs = false;

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
                });
            }
        } else if let Some(branch) = field.strip_prefix(b"branch refs/heads/") {
            if let Ok(branch) = std::str::from_utf8(branch) {
                current_branch = Some(branch.to_string());
            } else {
                current_branch = None;
                skipped_unsupported_refs = true;
            }
        } else if field.is_empty() {
            if let Some(path) = current_path.take() {
                worktrees.push(Worktree {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_branch = None;
        }
    }

    if let Some(path) = current_path {
        worktrees.push(Worktree {
            path,
            branch: current_branch,
        });
    }

    Ok(Listed::new(worktrees, skipped_unsupported_refs))
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
        assert_eq!(worktrees[1].branch.as_deref(), Some("feat/thing"));
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

    #[test]
    fn non_utf8_checked_out_branch_does_not_reject_worktree_listing() {
        let listed = parse_worktree_porcelain(
            b"worktree /repo\0HEAD abc123\0branch refs/heads/invalid-\xff\0\0",
        )
        .unwrap();

        assert_eq!(listed.items.len(), 1);
        assert!(listed.items[0].branch.is_none());
        assert!(listed.skipped_unsupported_refs);
    }
}
