mod cli;
pub mod mock;
mod provider;
mod repo;

pub use cli::{CliGitProvider, walk_repos};
pub use provider::{GitProvider, LocalBranchAlreadyExists, is_local_branch_already_exists};
pub use repo::{Repo, RepoScan, ScanWarning, Worktree};

/// Parse `git worktree list --porcelain` output into worktrees.
pub fn parse_worktree_porcelain(output: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;
    let mut is_first = true;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(path.into());
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = Some(branch.to_string());
        } else if line.is_empty() {
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

    worktrees
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn parses_multiple_worktrees() {
        let output = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/main

worktree /home/user/project-feat
HEAD def456
branch refs/heads/feat/thing

";
        let worktrees = parse_worktree_porcelain(output);
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].path, PathBuf::from("/home/user/project"));
        assert!(worktrees[0].is_main);
        assert_eq!(worktrees[1].branch.as_deref(), Some("feat/thing"));
        assert!(!worktrees[1].is_main);
    }

    #[test]
    fn parses_detached_and_unterminated_entries() {
        let detached =
            parse_worktree_porcelain("worktree /home/user/project\nHEAD abc123\ndetached\n\n");
        assert_eq!(detached.len(), 1);
        assert!(detached[0].branch.is_none());

        let unterminated =
            parse_worktree_porcelain("worktree /home/user/project\nbranch refs/heads/main");
        assert_eq!(unterminated[0].branch.as_deref(), Some("main"));
    }

    #[test]
    fn empty_porcelain_has_no_worktrees() {
        assert!(parse_worktree_porcelain("").is_empty());
    }
}
