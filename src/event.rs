use std::path::PathBuf;

use crate::{
    git::{Repo, ScanWarning, Worktree},
    herdr::{WorkspaceInfo, WorktreeInfo},
    state::BranchEntry,
};

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
        generation: u64,
        matches: Vec<(PathBuf, i64)>,
        selected_path: Option<PathBuf>,
    },
    BranchesLoaded {
        branches: Vec<BranchEntry>,
        worktrees: Vec<Worktree>,
        local_names: Vec<String>,
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
    RepoOpened,
    GitError(String),
    HerdrError(String),
}
