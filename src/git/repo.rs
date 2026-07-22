use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Repo {
    pub name: String,
    pub path: PathBuf,
    pub worktrees: Vec<Worktree>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanWarning {
    pub path: PathBuf,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_and_worktree_serde_round_trip() {
        let repo = Repo {
            name: "demo".to_string(),
            path: PathBuf::from("/tmp/demo"),
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/demo"),
                branch: Some("main".to_string()),
            }],
        };

        let json = serde_json::to_string(&repo).unwrap();
        let decoded: Repo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, repo);
    }
}
