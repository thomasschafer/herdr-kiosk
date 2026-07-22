use std::path::PathBuf;

use crate::{
    git::{Repo, ScanWarning, Worktree},
    herdr::{WorkspaceInfo, WorktreeInfo},
    state::BranchEntry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTarget {
    Repos,
    Branches,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterKey {
    Repo(PathBuf),
    Branch(String),
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ReposFound {
        repo: Repo,
    },
    RepoEnriched {
        repo_path: PathBuf,
        worktrees: Vec<Worktree>,
    },
    ScanComplete {
        search_dirs: Vec<(PathBuf, u16)>,
    },
    ScanWarning(ScanWarning),
    FilterCompleted {
        target: FilterTarget,
        generation: u64,
        matches: Vec<(FilterKey, i64)>,
        selected: Option<FilterKey>,
    },
    BranchesLoaded {
        repo_path: PathBuf,
        branches: Vec<BranchEntry>,
        worktrees: Vec<Worktree>,
    },
    BranchLoadFailed {
        repo_path: PathBuf,
        message: String,
    },
    RemoteBranchesLoaded {
        branches: Vec<BranchEntry>,
    },
    GitFetchCompleted {
        branches: Vec<BranchEntry>,
        repo_path: PathBuf,
        is_final: bool,
    },
    WorktreeCreated {
        path: PathBuf,
    },
    WorktreeRemoved {
        branch_name: String,
        worktree_path: PathBuf,
    },
    WorktreeRemoveFailed {
        branch_name: String,
        worktree_path: PathBuf,
        error: String,
    },
    OpenWorkspacesLoaded {
        workspaces: Vec<WorkspaceInfo>,
    },
    OpenWorktreesLoaded {
        repo_path: PathBuf,
        worktrees: Vec<WorktreeInfo>,
    },
    OpenWorktreesFailed {
        repo_path: PathBuf,
        message: String,
    },
    RepoOpened,
    RepoOpenFailed(String),
    BranchOperationFailed {
        repo_path: PathBuf,
        message: String,
    },
    OpenWorkspacesFailed(String),
    GitError(String),
}
