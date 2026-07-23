use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    config::OnOpenConfig, git::Repo, herdr::WorktreeInfo, pending_delete::PendingWorktreeDelete,
    screens::branch::BranchViewState,
};

pub use crate::screens::branch::{BranchContext, BranchId, OpenWorktreeLoadState};

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

    pub fn visible_items(&self, viewport_rows: usize) -> Vec<(usize, usize)> {
        let start = self.scroll_offset.min(self.filtered.len());
        let end = start
            .saturating_add(viewport_rows.max(1))
            .min(self.filtered.len());
        let mut positions = (start..end).collect::<Vec<_>>();
        if let Some(selected) = self
            .selected
            .filter(|selected| *selected < self.filtered.len())
            && (selected < start || selected >= end)
        {
            positions.push(selected);
            positions.sort_unstable();
        }
        positions
            .into_iter()
            .map(|position| (position, self.filtered[position].0))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseBranchSelection {
    pub new_name: String,
    pub bases: Vec<String>,
    pub list: SearchableList,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpBindingRow {
    pub section_name: &'static str,
    pub key_display: String,
    pub command_name: &'static str,
    pub description: &'static str,
}

impl HelpBindingRow {
    pub fn search_text(&self) -> String {
        format!(
            "{} {} {}",
            self.key_display, self.command_name, self.description
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpOverlayState {
    pub rows: Vec<HelpBindingRow>,
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
    category: ToastCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastCategory {
    General,
    Scan,
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppState {
    pub repos: Vec<RepoEntry>,
    pub repo_list: SearchableList,
    pub branch_view: BranchViewState,
    pub mode: Mode,
    pub loading_repos: bool,
    pub seen_repo_paths: HashSet<PathBuf>,
    pub open_repo_roots: HashSet<PathBuf>,
    pub current_cwd: Option<PathBuf>,
    pub selection_touched: bool,
    pub toasts: VecDeque<Toast>,
    toast_messages: HashSet<String>,
    scan_warning_count: usize,
    pub help_overlay: Option<HelpOverlayState>,
    pub active_list_rows: usize,
    pub repo_filter_generation: u64,
    pub base_filter_generation: u64,
    pub help_filter_generation: u64,
    pub pending_worktree_deletes: Vec<PendingWorktreeDelete>,
    pub in_flight_worktree_removals: HashSet<PathBuf>,
    pub on_open: OnOpenConfig,
}

impl AppState {
    pub fn new(current_cwd: Option<PathBuf>) -> Self {
        Self {
            repos: Vec::new(),
            repo_list: SearchableList::new(0),
            branch_view: BranchViewState::default(),
            mode: Mode::RepoSelect,
            loading_repos: true,
            seen_repo_paths: HashSet::new(),
            open_repo_roots: HashSet::new(),
            current_cwd,
            selection_touched: false,
            toasts: VecDeque::new(),
            toast_messages: HashSet::new(),
            scan_warning_count: 0,
            help_overlay: None,
            active_list_rows: 1,
            repo_filter_generation: 0,
            base_filter_generation: 0,
            help_filter_generation: 0,
            pending_worktree_deletes: Vec::new(),
            in_flight_worktree_removals: HashSet::new(),
            on_open: OnOpenConfig::default(),
        }
    }

    pub fn selected_repo(&self) -> Option<&RepoEntry> {
        let selected = self.repo_list.selected?;
        let index = self.repo_list.filtered.get(selected)?.0;
        self.repos.get(index)
    }

    pub fn selected_branch(&self) -> Option<&BranchEntry> {
        self.branch_view.selected()
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
        if self.branch_view.loading {
            return Err("Branches are still loading");
        }
        let name = self.branch_view.list.input.text.clone();
        if name.is_empty() {
            return Err("Type a branch name first");
        }
        if let Some(branch) = self
            .branch_view
            .entries
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
        match &self.branch_view.open_worktree_load_state {
            OpenWorktreeLoadState::Loaded {
                repo_path,
                generation,
            } if repo_path == &context.repo_path && *generation == self.branch_view.generation => {}
            OpenWorktreeLoadState::Failed {
                repo_path,
                generation,
            } if repo_path == &context.repo_path && *generation == self.branch_view.generation => {
                return Err(
                    "Cannot delete checkout because open checkout state could not be loaded",
                );
            }
            _ => return Err("Open checkout state is still loading; deletion is disabled"),
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
        self.push_toast_with_category(kind, &message.into(), ToastCategory::General);
    }

    pub fn push_scan_warning(&mut self) {
        self.scan_warning_count = self.scan_warning_count.saturating_add(1);
        let message = format!(
            "{} director{} could not be scanned",
            self.scan_warning_count,
            if self.scan_warning_count == 1 {
                "y"
            } else {
                "ies"
            }
        );
        if let Some(toast) = self
            .toasts
            .iter_mut()
            .find(|toast| toast.category == ToastCategory::Scan)
        {
            self.toast_messages.remove(&toast.message);
            toast.message.clone_from(&message);
            self.toast_messages.insert(message);
        } else {
            self.push_toast_with_category(ToastKind::Warning, &message, ToastCategory::Scan);
        }
    }

    pub fn dismiss_toast(&mut self) {
        if let Some(toast) = self.toasts.pop_front() {
            self.toast_messages.remove(&toast.message);
        }
    }

    fn push_toast_with_category(
        &mut self,
        kind: ToastKind,
        message: &str,
        category: ToastCategory,
    ) {
        const MAX_RETAINED_TOASTS: usize = 8;

        let message = message.split_whitespace().collect::<Vec<_>>().join(" ");
        if message.is_empty() || self.toast_messages.contains(&message) {
            return;
        }
        if self.toasts.len() == MAX_RETAINED_TOASTS {
            let warning = self
                .toasts
                .iter()
                .position(|toast| toast.kind == ToastKind::Warning);
            let remove = match (kind, warning) {
                (ToastKind::Error | ToastKind::Warning, Some(index)) => Some(index),
                (ToastKind::Error, None) => Some(0),
                (ToastKind::Warning, None) => None,
            };
            let Some(remove) = remove else {
                return;
            };
            if let Some(removed) = self.toasts.remove(remove) {
                self.toast_messages.remove(&removed.message);
            }
        }
        self.toast_messages.insert(message.clone());
        self.toasts.push_back(Toast {
            kind,
            message,
            category,
        });
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
    pub fn id(&self) -> BranchId {
        self.remote.as_ref().map_or_else(
            || BranchId::Local(self.name.clone()),
            |remote| BranchId::Remote {
                remote: remote.clone(),
                name: self.name.clone(),
            },
        )
    }

    pub fn display_name(&self) -> String {
        self.id().display_name()
    }

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

    pub(crate) fn sort(entries: &mut [Self]) {
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
                .then(left.remote.cmp(&right.remote))
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
    fn searchable_list_scroll_follows_keyboard_selection() {
        let mut list = SearchableList::new(8);
        list.move_selection(6);
        list.update_scroll_offset(3);
        assert_eq!(list.selected, Some(6));
        assert_eq!(list.scroll_offset, 4);
        list.move_selection(-5);
        list.update_scroll_offset(3);
        assert_eq!(list.selected, Some(1));
        assert_eq!(list.scroll_offset, 1);
    }

    #[test]
    fn searchable_list_materializes_only_the_visible_window_and_selection() {
        let mut list = SearchableList::new(20_000);
        list.scroll_offset = 10_000;
        list.selected = Some(19_999);

        let visible = list.visible_items(12);

        assert_eq!(visible.len(), 13);
        assert_eq!(visible.first(), Some(&(10_000, 10_000)));
        assert_eq!(visible[11], (10_011, 10_011));
        assert_eq!(visible.last(), Some(&(19_999, 19_999)));
    }

    #[test]
    fn scan_warnings_are_aggregated_into_one_toast() {
        let mut state = AppState::new(None);

        for _ in 0..1_000 {
            state.push_scan_warning();
        }

        assert_eq!(state.toasts.len(), 1);
        assert_eq!(
            state.toasts[0].message,
            "1000 directories could not be scanned"
        );
        assert_eq!(state.toast_messages.len(), 1);
    }

    #[test]
    fn toast_queue_is_bounded_and_dedup_index_stays_in_sync() {
        let mut state = AppState::new(None);

        for index in 0..100 {
            state.push_toast(ToastKind::Warning, format!("warning {index}"));
        }
        for _ in 0..100 {
            state.push_toast(ToastKind::Warning, "warning 99");
        }

        assert_eq!(state.toasts.len(), 8);
        assert_eq!(state.toast_messages.len(), state.toasts.len());
        assert_eq!(
            state.toasts.front().map(|toast| toast.message.as_str()),
            Some("warning 92")
        );
        state.dismiss_toast();
        assert_eq!(state.toast_messages.len(), state.toasts.len());
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
                },
                Worktree {
                    path: "/repo-feature".into(),
                    branch: Some("feature".into()),
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
                open_workspace_id: Some("w_2".into()),
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
    fn new_branch_routing_rejects_empty_and_routes_existing_local() {
        let mut state = AppState::new(None);
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        assert_eq!(state.new_branch_route(), Err("Type a branch name first"));

        state.branch_view.entries =
            BranchEntry::build_local(&repo("/repo"), &["feature".into()], None, None);
        state.branch_view.list = SearchableList::new(1);
        state.branch_view.list.input.text = "feature".into();
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
        state.branch_view.entries = vec![BranchEntry {
            name: "main".into(),
            worktree_path: Some("/repo".into()),
            is_current: true,
            is_default: true,
            remote: None,
            open_workspace_id: Some("w_1".into()),
        }];
        state.branch_view.list = SearchableList::new(1);
        assert_eq!(
            state.selected_worktree_for_delete(),
            Err("Cannot delete the main checkout")
        );

        state.branch_view.entries[0] = BranchEntry {
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
