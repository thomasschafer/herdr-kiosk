use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::{git::Repo, herdr::WorktreeInfo, pending_delete::PendingWorktreeDelete};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoEntry {
    pub repo: Repo,
    pub disambiguator: Option<String>,
    pub is_open: bool,
}

impl RepoEntry {
    pub fn new(repo: Repo) -> Self {
        Self {
            repo,
            disambiguator: None,
            is_open: false,
        }
    }

    pub fn display_name(&self) -> String {
        self.disambiguator.as_ref().map_or_else(
            || self.repo.name.clone(),
            |suffix| format!("{} ({suffix})", self.repo.name),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    pub text: String,
    pub cursor: usize,
}

impl TextInput {
    fn boundaries(&self) -> Vec<usize> {
        let mut boundaries: Vec<_> = self.text.grapheme_indices(true).map(|(i, _)| i).collect();
        boundaries.push(self.text.len());
        boundaries
    }

    fn clamp_cursor(&mut self, boundaries: &[usize]) -> usize {
        let cursor = self.cursor.min(self.text.len());
        let index = match boundaries.binary_search(&cursor) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        };
        self.cursor = boundaries.get(index).copied().unwrap_or_default();
        index
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn insert_char(&mut self, character: char) {
        let boundaries = self.boundaries();
        self.clamp_cursor(&boundaries);
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    pub fn backspace(&mut self) {
        let boundaries = self.boundaries();
        let index = self.clamp_cursor(&boundaries);
        if index > 0 {
            let previous = boundaries[index - 1];
            self.text.drain(previous..self.cursor);
            self.cursor = previous;
        }
    }

    pub fn delete_word(&mut self) {
        if self.cursor == 0 || self.text.is_empty() {
            return;
        }
        let boundaries = self.boundaries();
        self.clamp_cursor(&boundaries);
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches(char::is_whitespace);
        let word_start = trimmed.rfind(char::is_whitespace).map_or(0, |index| {
            index + self.text[index..].chars().next().unwrap().len_utf8()
        });
        self.text.drain(word_start..self.cursor);
        self.cursor = word_start;
    }

    pub fn cursor_left(&mut self) {
        let boundaries = self.boundaries();
        let index = self.clamp_cursor(&boundaries);
        if index > 0 {
            self.cursor = boundaries[index - 1];
        }
    }

    pub fn cursor_right(&mut self) {
        let boundaries = self.boundaries();
        let index = self.clamp_cursor(&boundaries);
        if index + 1 < boundaries.len() {
            self.cursor = boundaries[index + 1];
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchableList {
    pub input: TextInput,
    pub filtered: Vec<(usize, i64)>,
    pub selected: Option<usize>,
    pub scroll_offset: usize,
}

impl SearchableList {
    pub fn new(item_count: usize) -> Self {
        Self {
            input: TextInput::default(),
            filtered: (0..item_count).map(|index| (index, 0)).collect(),
            selected: (item_count > 0).then_some(0),
            scroll_offset: 0,
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        if self.filtered.is_empty() {
            return;
        }
        let current = self.selected.unwrap_or_default();
        self.selected = Some(if delta.is_positive() {
            current
                .saturating_add(delta.unsigned_abs() as usize)
                .min(self.filtered.len() - 1)
        } else {
            current.saturating_sub(delta.unsigned_abs() as usize)
        });
    }

    pub fn update_scroll_offset(&mut self, viewport_rows: usize) {
        if self.filtered.is_empty() {
            self.scroll_offset = 0;
            return;
        }
        let viewport_rows = viewport_rows.max(1);
        let selected = self
            .selected
            .unwrap_or_default()
            .min(self.filtered.len() - 1);
        if selected < self.scroll_offset {
            self.scroll_offset = selected;
        } else if selected >= self.scroll_offset.saturating_add(viewport_rows) {
            self.scroll_offset = selected + 1 - viewport_rows;
        }
        self.scroll_offset = self
            .scroll_offset
            .min(self.filtered.len().saturating_sub(viewport_rows));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchContext {
    pub repo_path: PathBuf,
    pub repo_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseBranchSelection {
    pub new_name: String,
    pub bases: Vec<String>,
    pub list: SearchableList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteWorktreeTarget {
    pub branch_name: String,
    pub worktree_path: PathBuf,
    pub open_workspace_id: Option<String>,
    pub force: bool,
    pub in_progress: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NewBranchRoute {
    Existing(BranchEntry),
    Validate {
        context: BranchContext,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    RepoSelect,
    BranchSelect(BranchContext),
    ValidatingNewBranch {
        context: BranchContext,
        name: String,
    },
    SelectBaseBranch {
        context: BranchContext,
        flow: BaseBranchSelection,
    },
    ConfirmWorktreeDelete {
        context: BranchContext,
        target: DeleteWorktreeTarget,
    },
    Loading {
        message: String,
        branch: Option<BranchContext>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub message: String,
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppState {
    pub repos: Vec<RepoEntry>,
    pub repo_list: SearchableList,
    pub branches: Vec<BranchEntry>,
    pub branch_list: SearchableList,
    pub mode: Mode,
    pub loading_repos: bool,
    pub loading_branches: bool,
    pub seen_repo_paths: HashSet<PathBuf>,
    pub open_repo_roots: HashSet<PathBuf>,
    pub current_cwd: Option<PathBuf>,
    pub selection_touched: bool,
    pub toasts: VecDeque<Toast>,
    pub help_open: bool,
    pub active_list_rows: usize,
    pub repo_filter_generation: u64,
    pub branch_filter_generation: u64,
    pub open_worktrees: Vec<WorktreeInfo>,
    pub open_worktrees_repo: Option<PathBuf>,
    pub remote_branches: BTreeMap<String, Vec<BranchEntry>>,
    pub fetching_remote_repo: Option<PathBuf>,
    pub fetch_warning_remotes: HashSet<String>,
    pub base_filter_generation: u64,
    pub pending_worktree_deletes: Vec<PendingWorktreeDelete>,
    pub in_flight_worktree_removals: HashSet<PathBuf>,
}

impl AppState {
    pub fn new(current_cwd: Option<PathBuf>) -> Self {
        Self {
            repos: Vec::new(),
            repo_list: SearchableList::new(0),
            branches: Vec::new(),
            branch_list: SearchableList::new(0),
            mode: Mode::RepoSelect,
            loading_repos: true,
            loading_branches: false,
            seen_repo_paths: HashSet::new(),
            open_repo_roots: HashSet::new(),
            current_cwd,
            selection_touched: false,
            toasts: VecDeque::new(),
            help_open: false,
            active_list_rows: 1,
            repo_filter_generation: 0,
            branch_filter_generation: 0,
            open_worktrees: Vec::new(),
            open_worktrees_repo: None,
            remote_branches: BTreeMap::new(),
            fetching_remote_repo: None,
            fetch_warning_remotes: HashSet::new(),
            base_filter_generation: 0,
            pending_worktree_deletes: Vec::new(),
            in_flight_worktree_removals: HashSet::new(),
        }
    }

    pub fn selected_repo(&self) -> Option<&RepoEntry> {
        let selected = self.repo_list.selected?;
        let index = self.repo_list.filtered.get(selected)?.0;
        self.repos.get(index)
    }

    pub fn selected_branch(&self) -> Option<&BranchEntry> {
        let selected = self.branch_list.selected?;
        let index = self.branch_list.filtered.get(selected)?.0;
        self.branches.get(index)
    }

    pub fn branch_context(&self) -> Option<&BranchContext> {
        match &self.mode {
            Mode::BranchSelect(context)
            | Mode::ValidatingNewBranch { context, .. }
            | Mode::SelectBaseBranch { context, .. }
            | Mode::ConfirmWorktreeDelete { context, .. }
            | Mode::Loading {
                branch: Some(context),
                ..
            } => Some(context),
            Mode::RepoSelect | Mode::Loading { branch: None, .. } => None,
        }
    }

    pub fn new_branch_route(&self) -> Result<NewBranchRoute, &'static str> {
        let Mode::BranchSelect(context) = &self.mode else {
            return Err("New branches can only be created from the branch view");
        };
        let name = self.branch_list.input.text.clone();
        if name.is_empty() {
            return Err("Type a branch name first");
        }
        if let Some(branch) = self
            .branches
            .iter()
            .find(|branch| branch.remote.is_none() && branch.name == name)
        {
            return Ok(NewBranchRoute::Existing(branch.clone()));
        }
        Ok(NewBranchRoute::Validate {
            context: context.clone(),
            name,
        })
    }

    pub fn selected_worktree_for_delete(&self) -> Result<DeleteWorktreeTarget, &'static str> {
        let Mode::BranchSelect(context) = &self.mode else {
            return Err("Worktrees can only be deleted from the branch view");
        };
        let branch = self.selected_branch().ok_or("No branch selected")?;
        if branch.remote.is_some() {
            return Err("Remote-only branches have no checkout to delete");
        }
        let worktree_path = branch
            .worktree_path
            .clone()
            .ok_or("No worktree to delete")?;
        let canonical_worktree =
            std::fs::canonicalize(&worktree_path).unwrap_or_else(|_| worktree_path.clone());
        let canonical_repo =
            std::fs::canonicalize(&context.repo_path).unwrap_or_else(|_| context.repo_path.clone());
        if crate::path::equivalent(&canonical_worktree, &canonical_repo) {
            return Err("Cannot delete the main checkout");
        }
        if self.in_flight_worktree_removals.contains(&worktree_path)
            || self.pending_worktree_deletes.iter().any(|pending| {
                pending.repo_path == context.repo_path && pending.branch_name == branch.name
            })
        {
            return Err("Worktree deletion already in progress");
        }
        Ok(DeleteWorktreeTarget {
            branch_name: branch.name.clone(),
            worktree_path,
            open_workspace_id: branch.open_workspace_id.clone(),
            force: false,
            in_progress: false,
        })
    }

    pub fn mark_pending_worktree_delete(&mut self, pending: PendingWorktreeDelete) {
        self.pending_worktree_deletes.retain(|entry| {
            !(entry.repo_path == pending.repo_path && entry.branch_name == pending.branch_name)
        });
        self.pending_worktree_deletes.push(pending);
    }

    pub fn clear_pending_worktree_delete(&mut self, worktree_path: &Path) {
        self.pending_worktree_deletes
            .retain(|pending| pending.worktree_path != worktree_path);
    }

    pub fn reconcile_pending_worktree_deletes(&mut self, repo_path: &Path) -> bool {
        let active = self
            .repos
            .iter()
            .find(|repo| repo.repo.path == repo_path)
            .map(|repo| {
                repo.repo
                    .worktrees
                    .iter()
                    .map(|worktree| worktree.path.as_path())
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();
        let before = self.pending_worktree_deletes.len();
        self.pending_worktree_deletes.retain(|pending| {
            pending.repo_path != repo_path
                || !pending.is_expired() && active.contains(pending.worktree_path.as_path())
        });
        before != self.pending_worktree_deletes.len()
    }

    pub fn push_toast(&mut self, kind: ToastKind, message: impl Into<String>) {
        let message = message
            .into()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if !message.is_empty() && !self.toasts.iter().any(|toast| toast.message == message) {
            self.toasts.push_back(Toast { kind, message });
        }
    }

    pub fn reset_remote_branches(&mut self) {
        self.remote_branches.clear();
        self.fetching_remote_repo = None;
        self.fetch_warning_remotes.clear();
    }

    /// Merge one remote's latest snapshot without allowing task arrival order to
    /// overwrite another remote or a newer fetch result.
    pub fn merge_remote_branches(&mut self, remote: String, incoming: Vec<BranchEntry>) {
        let bucket = self.remote_branches.entry(remote).or_default();
        let mut known: HashSet<String> = bucket.iter().map(|entry| entry.name.clone()).collect();
        bucket.extend(
            incoming
                .into_iter()
                .filter(|entry| known.insert(entry.name.clone())),
        );
        BranchEntry::sort(bucket);

        let mut merged: Vec<_> = self
            .branches
            .iter()
            .filter(|entry| entry.remote.is_none())
            .cloned()
            .collect();
        let mut names: HashSet<String> = merged.iter().map(|entry| entry.name.clone()).collect();
        for entries in self.remote_branches.values() {
            merged.extend(
                entries
                    .iter()
                    .filter(|entry| names.insert(entry.name.clone()))
                    .cloned(),
            );
        }
        BranchEntry::sort(&mut merged);
        self.branches = merged;
    }

    pub fn canonical_sort(&mut self) {
        let selected_path = self.selected_repo().map(|entry| entry.repo.path.clone());
        let filtered_paths = self
            .repo_list
            .filtered
            .iter()
            .filter_map(|(index, score)| {
                self.repos
                    .get(*index)
                    .map(|entry| (entry.repo.path.clone(), *score))
            })
            .collect::<Vec<_>>();
        self.repos.sort_by(|left, right| {
            left.repo
                .name
                .to_lowercase()
                .cmp(&right.repo.name.to_lowercase())
                .then(left.repo.name.cmp(&right.repo.name))
                .then(left.repo.path.cmp(&right.repo.path))
        });
        if self.repo_list.input.text.is_empty() {
            self.repo_list.filtered = (0..self.repos.len()).map(|index| (index, 0)).collect();
            self.repo_list.selected = selected_path
                .and_then(|path| self.repos.iter().position(|entry| entry.repo.path == path))
                .or_else(|| (!self.repos.is_empty()).then_some(0));
        } else {
            let indices: HashMap<_, _> = self
                .repos
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.repo.path.as_path(), index))
                .collect();
            self.repo_list.filtered = filtered_paths
                .iter()
                .filter_map(|(path, score)| {
                    indices.get(path.as_path()).map(|index| (*index, *score))
                })
                .collect();
            self.repo_list.selected = selected_path.and_then(|path| {
                self.repo_list
                    .filtered
                    .iter()
                    .position(|(index, _)| self.repos[*index].repo.path == path)
            });
        }
    }

    pub fn apply_current_repo_selection(&mut self) {
        if self.selection_touched {
            return;
        }
        let Some(cwd) = self.current_cwd.as_deref() else {
            return;
        };
        let best = self
            .repo_list
            .filtered
            .iter()
            .enumerate()
            .filter(|(_, (index, _))| crate::path::starts_with(cwd, &self.repos[*index].repo.path))
            .max_by_key(|(_, (index, _))| self.repos[*index].repo.path.components().count())
            .map(|(position, _)| position);
        if best.is_some() {
            self.repo_list.selected = best;
        }
    }
}

pub fn collision_disambiguators(repos: &[Repo]) -> Vec<Option<String>> {
    collision_disambiguators_with_case(repos, cfg!(windows))
}

fn collision_disambiguators_with_case(
    repos: &[Repo],
    case_insensitive: bool,
) -> Vec<Option<String>> {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, repo) in repos.iter().enumerate() {
        let key = if case_insensitive {
            repo.name.to_lowercase()
        } else {
            repo.name.clone()
        };
        groups.entry(key).or_default().push(index);
    }
    let parents: Vec<Vec<OsString>> = repos
        .iter()
        .map(|repo| {
            repo.path
                .parent()
                .map(|parent| {
                    parent
                        .components()
                        .filter_map(|component| match component {
                            std::path::Component::Normal(value) => Some(value.to_os_string()),
                            std::path::Component::Prefix(value) => {
                                Some(value.as_os_str().to_os_string())
                            }
                            std::path::Component::RootDir
                            | std::path::Component::CurDir
                            | std::path::Component::ParentDir => None,
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();
    let mut result = vec![None; repos.len()];

    for indices in groups.values().filter(|indices| indices.len() > 1) {
        for &index in indices {
            let parent = &parents[index];
            let depth = (1..=parent.len())
                .find(|depth| {
                    let suffix = &parent[parent.len() - depth..];
                    indices.iter().all(|other| {
                        *other == index
                            || parents[*other].len() < *depth
                            || !path_components_equal(
                                &parents[*other][parents[*other].len() - depth..],
                                suffix,
                                case_insensitive,
                            )
                    })
                })
                .unwrap_or_else(|| {
                    panic!(
                        "duplicate repository path invariant violated for {}",
                        repos[index].path.display()
                    )
                });
            let suffix = parent[parent.len() - depth..]
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            result[index] = Some(if depth < parent.len() {
                format!("…/{suffix}")
            } else {
                suffix
            });
        }
    }
    result
}

fn path_components_equal(left: &[OsString], right: &[OsString], case_insensitive: bool) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left == right
                || case_insensitive
                    && left.to_string_lossy().to_lowercase()
                        == right.to_string_lossy().to_lowercase()
        })
}

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
                    .filter(|worktree| crate::path::starts_with(path, &worktree.path))
                    .max_by_key(|worktree| worktree.path.components().count())
            })
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
            .find(|worktree| crate::path::equivalent(Path::new(&worktree.path), path))
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

    fn repo(path: &str) -> Repo {
        Repo {
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            path: path.into(),
            worktrees: Vec::new(),
        }
    }

    #[test]
    fn collision_disambiguates_two_equal_names_with_shortest_parent_suffix() {
        let repos = [repo("foo/bar/baz"), repo("qux/bar/baz")];
        assert_eq!(
            collision_disambiguators(&repos),
            [Some("foo/bar".into()), Some("qux/bar".into())]
        );
    }

    #[test]
    fn collision_disambiguates_three_places_independently() {
        let repos = [
            repo("/root/a/shared/demo"),
            repo("/root/b/shared/demo"),
            repo("/root/unique/demo"),
        ];
        assert_eq!(
            collision_disambiguators(&repos),
            [
                Some("…/a/shared".into()),
                Some("…/b/shared".into()),
                Some("…/unique".into()),
            ]
        );
    }

    #[test]
    fn collision_handles_parents_that_also_collide() {
        let repos = [repo("/one/team/api"), repo("/two/team/api")];
        assert_eq!(
            collision_disambiguators(&repos),
            [Some("one/team".into()), Some("two/team".into())]
        );
    }

    #[test]
    fn collision_handles_repos_nested_below_search_roots() {
        let repos = [
            repo("/search/client/ios/app"),
            repo("/search/client/web/app"),
            repo("/search/direct/app"),
            repo("/search/other"),
        ];
        assert_eq!(
            collision_disambiguators(&repos),
            [
                Some("…/ios".into()),
                Some("…/web".into()),
                Some("…/direct".into()),
                None,
            ]
        );
    }

    #[test]
    fn collision_leaves_unique_names_unchanged() {
        let repos = [repo("/one/alpha"), repo("/two/beta")];
        assert_eq!(collision_disambiguators(&repos), [None, None]);
    }

    #[test]
    fn collision_windows_mode_is_case_insensitive_and_uses_forward_slashes() {
        let repos = [repo("C:/One/Team/API"), repo("c:/two/team/api")];
        assert_eq!(
            collision_disambiguators_with_case(&repos, true),
            [Some("…/One/Team".into()), Some("…/two/team".into())]
        );
    }

    #[test]
    fn text_input_deletes_unicode_graphemes_and_words() {
        let mut input = TextInput::default();
        for character in "one 👩‍💻".chars() {
            input.insert_char(character);
        }
        input.backspace();
        assert_eq!(input.text, "one ");
        input.delete_word();
        assert!(input.text.is_empty());
    }

    #[test]
    fn current_repo_selection_prefers_the_deepest_containing_repo() {
        let mut state = AppState::new(Some("/work/outer/inner/src".into()));
        state.repos = vec![
            RepoEntry::new(repo("/work/outer")),
            RepoEntry::new(repo("/work/outer/inner")),
        ];
        state.repo_list = SearchableList::new(2);
        state.apply_current_repo_selection();
        assert_eq!(state.repo_list.selected, Some(1));
    }

    #[test]
    fn branch_entries_derive_current_default_worktree_and_open_markers() {
        let repo = Repo {
            name: "repo".into(),
            path: "/repo".into(),
            worktrees: vec![
                Worktree {
                    path: "/repo".into(),
                    branch: Some("main".into()),
                    is_main: true,
                },
                Worktree {
                    path: "/repo-feature".into(),
                    branch: Some("feature".into()),
                    is_main: false,
                },
            ],
        };
        let mut entries = BranchEntry::build_local(
            &repo,
            &["main".into(), "feature".into(), "plain".into()],
            Some("main"),
            Some(Path::new("/repo-feature/src")),
        );
        for entry in &mut entries {
            entry.apply_open_worktrees(&[WorktreeInfo {
                path: "/repo-feature".into(),
                branch: Some("feature".into()),
                is_bare: false,
                is_detached: false,
                is_prunable: false,
                is_linked_worktree: true,
                open_workspace_id: Some("w_2".into()),
                label: "repo".into(),
            }]);
        }
        let feature = entries
            .iter()
            .find(|entry| entry.name == "feature")
            .unwrap();
        assert!(feature.is_current);
        assert!(feature.worktree_path.is_some());
        assert_eq!(feature.open_workspace_id.as_deref(), Some("w_2"));
        let main = entries.iter().find(|entry| entry.name == "main").unwrap();
        assert!(main.is_default);
        assert!(!main.is_current);
        let plain = entries.iter().find(|entry| entry.name == "plain").unwrap();
        assert!(plain.worktree_path.is_none());
    }

    #[test]
    fn branch_entries_have_no_current_marker_when_context_is_outside_repo_worktrees() {
        let mut repo = repo("/repo");
        repo.worktrees.push(Worktree {
            path: "/repo".into(),
            branch: Some("main".into()),
            is_main: true,
        });
        let entries = BranchEntry::build_local(
            &repo,
            &["main".into()],
            Some("main"),
            Some(Path::new("/somewhere-else")),
        );
        assert!(!entries[0].is_current);
        assert!(entries[0].is_default);
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

    #[test]
    fn remote_merges_are_deduplicated_and_sorted_after_locals() {
        let mut state = AppState::new(None);
        state.branches = BranchEntry::build_local(
            &repo("/repo"),
            &["z-local".into(), "main".into()],
            Some("main"),
            None,
        );
        state.merge_remote_branches(
            "upstream".into(),
            BranchEntry::build_remote(
                "upstream",
                &["z-local".into(), "z-remote".into()],
                &["z-local".into(), "main".into()],
            ),
        );
        state.merge_remote_branches(
            "origin".into(),
            BranchEntry::build_remote(
                "origin",
                &["a-remote".into(), "z-remote".into()],
                &["z-local".into(), "main".into()],
            ),
        );

        assert_eq!(
            state
                .branches
                .iter()
                .map(|entry| (entry.name.as_str(), entry.remote.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("main", None),
                ("z-local", None),
                ("a-remote", Some("origin")),
                ("z-remote", Some("origin")),
            ]
        );
    }

    #[test]
    fn new_branch_routing_rejects_empty_and_routes_existing_local() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        assert_eq!(state.new_branch_route(), Err("Type a branch name first"));

        state.branches = BranchEntry::build_local(&repo("/repo"), &["feature".into()], None, None);
        state.branch_list = SearchableList::new(1);
        state.branch_list.input.text = "feature".into();
        assert!(matches!(
            state.new_branch_route(),
            Ok(NewBranchRoute::Existing(branch)) if branch.name == "feature"
        ));
    }

    #[test]
    fn delete_guards_refuse_main_checkout_and_remote_only_entries_in_state() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branches = vec![BranchEntry {
            name: "main".into(),
            worktree_path: Some("/repo".into()),
            is_current: true,
            is_default: true,
            remote: None,
            open_workspace_id: Some("w_1".into()),
        }];
        state.branch_list = SearchableList::new(1);
        assert_eq!(
            state.selected_worktree_for_delete(),
            Err("Cannot delete the main checkout")
        );

        state.branches[0] = BranchEntry {
            name: "remote-only".into(),
            worktree_path: None,
            is_current: false,
            is_default: false,
            remote: Some("origin".into()),
            open_workspace_id: None,
        };
        assert_eq!(
            state.selected_worktree_for_delete(),
            Err("Remote-only branches have no checkout to delete")
        );
    }
}
