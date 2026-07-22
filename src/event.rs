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
    Bases,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterKey {
    Repo(PathBuf),
    Branch(String),
    Base(String),
    Help(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeRemovalOutcome {
    Removed { warning: Option<String> },
    DirtyRequiresForce,
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum AppEvent {
    ReposFound {
        repo: Repo,
    },
    ScanComplete,
    ScanWarning(ScanWarning),
    FilterCompleted {
        target: FilterTarget,
        generation: u64,
        matches: Vec<(FilterKey, i64)>,
        selected: Option<FilterKey>,
    },
    BranchesLoaded {
        repo_path: PathBuf,
        generation: u64,
        branches: Vec<BranchEntry>,
        worktrees: Vec<Worktree>,
    },
    BranchLoadFailed {
        repo_path: PathBuf,
        generation: u64,
        message: String,
    },
    RemoteBranchesLoaded {
        repo_path: PathBuf,
        generation: u64,
        remote: String,
        branches: Vec<BranchEntry>,
    },
    RemoteBranchLoadFailed {
        repo_path: PathBuf,
        generation: u64,
        message: String,
    },
    GitFetchCompleted {
        remote: Option<String>,
        branches: Vec<BranchEntry>,
        repo_path: PathBuf,
        generation: u64,
        error: Option<String>,
        is_final: bool,
    },
    BranchNameValidated {
        repo_path: PathBuf,
        branch_name: String,
        valid: bool,
        error: Option<String>,
    },
    WorktreeRemovalFinished {
        repo_path: PathBuf,
        branch_name: String,
        worktree_path: PathBuf,
        outcome: WorktreeRemovalOutcome,
    },
    OpenWorkspacesLoaded {
        workspaces: Vec<WorkspaceInfo>,
    },
    OpenWorktreesLoaded {
        repo_path: PathBuf,
        generation: u64,
        worktrees: Vec<WorktreeInfo>,
    },
    OpenWorktreesFailed {
        repo_path: PathBuf,
        generation: u64,
        message: String,
    },
    RepoOpened {
        warning: Option<String>,
    },
    RepoOpenFailed(String),
    BranchOperationFailed {
        repo_path: PathBuf,
        message: String,
    },
    OpenWorkspacesFailed(String),
}
