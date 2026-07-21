use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{git::Repo, herdr::WorktreeInfo};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct BranchEntry {
    pub name: String,
    pub worktree_path: Option<PathBuf>,
    pub is_current: bool,
    pub is_default: bool,
    pub remote: Option<String>,
    pub open_workspace_id: Option<String>,
}

impl BranchEntry {
    pub fn build_local(
        repo: &Repo,
        branch_names: &[String],
        default_branch: Option<&str>,
        cwd: Option<&Path>,
    ) -> Vec<Self> {
        let worktree_by_branch: HashMap<&str, _> = repo
            .worktrees
            .iter()
            .filter_map(|worktree| worktree.branch.as_deref().map(|branch| (branch, worktree)))
            .collect();
        let current_branch = cwd
            .and_then(|path| {
                repo.worktrees
                    .iter()
                    .filter(|worktree| path.starts_with(&worktree.path))
                    .max_by_key(|worktree| worktree.path.components().count())
            })
            .or_else(|| repo.worktrees.first())
            .and_then(|worktree| worktree.branch.as_deref());

        let mut entries: Vec<_> = branch_names
            .iter()
            .map(|name| Self {
                name: name.clone(),
                worktree_path: worktree_by_branch
                    .get(name.as_str())
                    .map(|worktree| worktree.path.clone()),
                is_current: current_branch == Some(name.as_str()),
                is_default: default_branch == Some(name.as_str()),
                remote: None,
                open_workspace_id: None,
            })
            .collect();
        Self::sort(&mut entries);
        entries
    }

    pub fn build_remote(
        remote: &str,
        remote_names: &[String],
        local_names: &[String],
    ) -> Vec<Self> {
        let local_names: std::collections::HashSet<_> =
            local_names.iter().map(String::as_str).collect();
        remote_names
            .iter()
            .filter(|name| !local_names.contains(name.as_str()))
            .map(|name| Self {
                name: name.clone(),
                worktree_path: None,
                is_current: false,
                is_default: false,
                remote: Some(remote.to_string()),
                open_workspace_id: None,
            })
            .collect()
    }

    pub fn apply_open_worktrees(&mut self, worktrees: &[WorktreeInfo]) {
        let Some(path) = self.worktree_path.as_ref() else {
            self.open_workspace_id = None;
            return;
        };
        self.open_workspace_id = worktrees
            .iter()
            .find(|worktree| Path::new(&worktree.path) == path)
            .and_then(|worktree| worktree.open_workspace_id.clone());
    }

    fn sort(entries: &mut [Self]) {
        entries.sort_by(|left, right| {
            left.remote
                .is_some()
                .cmp(&right.remote.is_some())
                .then(right.is_current.cmp(&left.is_current))
                .then(right.is_default.cmp(&left.is_default))
                .then(
                    right
                        .worktree_path
                        .is_some()
                        .cmp(&left.worktree_path.is_some()),
                )
                .then(left.name.cmp(&right.name))
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::git::Worktree;

    use super::*;

    #[test]
    fn branch_entries_include_open_workspace_state() {
        let repo = Repo {
            name: "repo".into(),
            path: "/repo".into(),
            worktrees: vec![Worktree {
                path: "/repo-feature".into(),
                branch: Some("feature".into()),
                is_main: false,
            }],
        };
        let mut entries = BranchEntry::build_local(
            &repo,
            &["feature".into()],
            None,
            Some(Path::new("/repo-feature/src")),
        );
        entries[0].apply_open_worktrees(&[WorktreeInfo {
            path: "/repo-feature".into(),
            branch: Some("feature".into()),
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_linked_worktree: true,
            open_workspace_id: Some("w_2".into()),
            label: "repo".into(),
        }]);
        assert!(entries[0].is_current);
        assert_eq!(entries[0].open_workspace_id.as_deref(), Some("w_2"));
    }

    #[test]
    fn remote_entries_are_deduplicated_against_local_names() {
        let entries = BranchEntry::build_remote(
            "upstream",
            &["main".into(), "feature".into()],
            &["main".into()],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].remote.as_deref(), Some("upstream"));
        assert_eq!(entries[0].name, "feature");
    }
}
