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
    #[serde(default = "is_git_by_default")]
    pub is_git: bool,
    pub worktrees: Vec<Worktree>,
}

const fn is_git_by_default() -> bool {
    true
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
            is_git: true,
            worktrees: vec![Worktree {
                path: PathBuf::from("/tmp/demo"),
                branch: Some("main".to_string()),
            }],
        };

        let json = serde_json::to_string(&repo).unwrap();
        let decoded: Repo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, repo);
    }

    #[test]
    fn repo_serde_defaults_legacy_entries_to_git() {
        let decoded: Repo =
            serde_json::from_str(r#"{"name":"demo","path":"/tmp/demo","worktrees":[]}"#).unwrap();
        assert!(decoded.is_git);
    }
}
