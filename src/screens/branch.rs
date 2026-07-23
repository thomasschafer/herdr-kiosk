use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    app::{FilterItem, FilterRequest, FilterWorker, TickChanges},
    event::{AppEvent, BranchOperationFailure, FilterKey, FilterTarget},
    git::{GitProvider, Repo},
    herdr::HerdrProvider,
    herdr::WorktreeInfo,
    spawn::{
        EventSender, spawn_branch_loading, spawn_open_branch, spawn_open_remote_branch,
        spawn_open_worktrees,
    },
    state::{AppState, BranchEntry, Mode, SearchableList, ToastKind},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchContext {
    pub repo_path: PathBuf,
    pub repo_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BranchId {
    Local(String),
    Remote { remote: String, name: String },
}

impl BranchId {
    pub fn display_name(&self) -> String {
        match self {
            Self::Local(name) => name.clone(),
            Self::Remote { remote, name } => format!("{remote}/{name}"),
        }
    }
}

impl From<&str> for BranchId {
    fn from(name: &str) -> Self {
        Self::Local(name.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWorktreeLoadState {
    Unknown,
    Loaded { repo_path: PathBuf, generation: u64 },
    Failed { repo_path: PathBuf, generation: u64 },
}

#[derive(Debug)]
pub struct BranchViewState {
    pub entries: Vec<BranchEntry>,
    pub list: SearchableList,
    pub loading: bool,
    pub filter_generation: u64,
    pub generation: u64,
    pub pending_selection: Option<BranchId>,
    pub open_worktrees: Vec<WorktreeInfo>,
    pub open_worktree_load_state: OpenWorktreeLoadState,
    remote_branches: BTreeMap<String, Vec<BranchEntry>>,
    pub fetching_remote_repo: Option<PathBuf>,
    fetch_warning_remotes: HashSet<String>,
    unsupported_ref_warning_generation: Option<u64>,
}

impl Default for BranchViewState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            list: SearchableList::new(0),
            loading: false,
            filter_generation: 0,
            generation: 0,
            pending_selection: None,
            open_worktrees: Vec::new(),
            open_worktree_load_state: OpenWorktreeLoadState::Unknown,
            remote_branches: BTreeMap::new(),
            fetching_remote_repo: None,
            fetch_warning_remotes: HashSet::new(),
            unsupported_ref_warning_generation: None,
        }
    }
}

impl BranchViewState {
    pub fn selected(&self) -> Option<&BranchEntry> {
        let selected = self.list.selected?;
        let index = self.list.filtered.get(selected)?.0;
        self.entries.get(index)
    }

    pub fn reset_remotes(&mut self) {
        self.remote_branches.clear();
        self.fetching_remote_repo = None;
        self.fetch_warning_remotes.clear();
    }

    pub fn merge_remote_snapshot(&mut self, remote: String, incoming: Vec<BranchEntry>) {
        let bucket = self.remote_branches.entry(remote).or_default();
        let mut known: HashSet<BranchId> = bucket.iter().map(BranchEntry::id).collect();
        bucket.extend(
            incoming
                .into_iter()
                .filter(|entry| known.insert(entry.id())),
        );
        BranchEntry::sort(bucket);

        let mut merged: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.remote.is_none())
            .cloned()
            .collect();
        let local_names: HashSet<String> = merged.iter().map(|entry| entry.name.clone()).collect();
        for entries in self.remote_branches.values() {
            merged.extend(
                entries
                    .iter()
                    .filter(|entry| !local_names.contains(&entry.name))
                    .cloned(),
            );
        }
        BranchEntry::sort(&mut merged);
        self.entries = merged;
    }

    pub fn promote_remote_to_local(&mut self, branch_name: &str) {
        for entries in self.remote_branches.values_mut() {
            entries.retain(|entry| entry.name != branch_name);
        }
        self.entries
            .retain(|entry| entry.remote.is_none() || entry.name != branch_name);
        if !self
            .entries
            .iter()
            .any(|entry| entry.remote.is_none() && entry.name == branch_name)
        {
            self.entries.push(BranchEntry {
                name: branch_name.to_string(),
                worktree_path: None,
                is_current: false,
                is_default: false,
                remote: None,
                open_workspace_id: None,
            });
        }
        BranchEntry::sort(&mut self.entries);
    }

    pub fn apply_open_indicators(&mut self) {
        for branch in &mut self.entries {
            branch.apply_open_worktrees(&self.open_worktrees);
        }
    }

    pub fn record_fetch_warning(&mut self, remote: String) -> bool {
        self.fetch_warning_remotes.insert(remote)
    }

    pub fn should_warn_unsupported_refs(&mut self, generation: u64, skipped: bool) -> bool {
        if skipped && self.unsupported_ref_warning_generation != Some(generation) {
            self.unsupported_ref_warning_generation = Some(generation);
            true
        } else {
            false
        }
    }

    pub fn clear_remote_snapshots(&mut self) {
        self.remote_branches.clear();
    }

    #[cfg(test)]
    pub(crate) fn remote_snapshots_are_empty(&self) -> bool {
        self.remote_branches.is_empty()
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    changes: &mut TickChanges,
) -> Option<AppEvent> {
    match event {
        AppEvent::FilterCompleted {
            target: FilterTarget::Branches,
            generation,
            matches,
            selected,
        } => {
            if generation == state.branch_view.filter_generation {
                apply_filter_result(state, &matches, selected.as_ref());
            }
        }
        AppEvent::BranchesLoaded {
            repo_path,
            generation,
            branches,
            worktrees,
            skipped_unsupported_refs,
        } => {
            if branch_view_generation_matches(state, &repo_path, generation) {
                if let Some(entry) = state
                    .repos
                    .iter_mut()
                    .find(|entry| entry.repo.path == repo_path)
                {
                    entry.repo.worktrees = worktrees;
                }
                state.branch_view.entries = branches;
                state.branch_view.clear_remote_snapshots();
                if let Some(selection) = state.branch_view.pending_selection.take() {
                    changes.pinned_branch_selection = Some(selection);
                }
                state.branch_view.apply_open_indicators();
                state.branch_view.loading = false;
                crate::screens::delete::reconcile_pending(state, &repo_path);
                state.branch_view.fetching_remote_repo = Some(repo_path.clone());
                changes.start_remote_loading = Some((
                    repo_path.clone(),
                    generation,
                    state
                        .branch_view
                        .entries
                        .iter()
                        .map(|branch| branch.name.clone())
                        .collect(),
                ));
                changes.branches_changed = true;
                changes.resume_pending_deletes = matches!(
                    &state.branch_view.open_worktree_load_state,
                    OpenWorktreeLoadState::Loaded {
                        repo_path: loaded_repo,
                        generation: loaded_generation,
                    } if loaded_repo == &repo_path && *loaded_generation == generation
                );
                warn_unsupported_refs(state, generation, skipped_unsupported_refs);
            }
        }
        AppEvent::RemoteBranchesLoaded {
            repo_path,
            generation,
            remote,
            branches,
            skipped_unsupported_refs,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                merge_remote_snapshot(state, changes, remote, branches);
                state.branch_view.apply_open_indicators();
                changes.branches_changed = true;
                warn_unsupported_refs(state, generation, skipped_unsupported_refs);
            }
        }
        AppEvent::RemoteBranchLoadFailed {
            repo_path,
            generation,
            message,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state.push_toast(ToastKind::Warning, message);
            }
        }
        AppEvent::GitFetchCompleted {
            remote,
            branches,
            repo_path,
            generation,
            error,
            is_final,
            skipped_unsupported_refs,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                if let Some(remote) = remote {
                    merge_remote_snapshot(state, changes, remote.clone(), branches);
                    state.branch_view.apply_open_indicators();
                    changes.branches_changed = true;
                    if let Some(error) = error
                        && state.branch_view.record_fetch_warning(remote.clone())
                    {
                        state.push_toast(
                            ToastKind::Warning,
                            format!("could not fetch remote {remote}: {error}"),
                        );
                    }
                } else if let Some(error) = error {
                    state.push_toast(ToastKind::Warning, error);
                }
                if is_final
                    && state.branch_view.fetching_remote_repo.as_deref()
                        == Some(repo_path.as_path())
                {
                    state.branch_view.fetching_remote_repo = None;
                }
                warn_unsupported_refs(state, generation, skipped_unsupported_refs);
            }
        }
        AppEvent::BranchLoadFailed {
            repo_path,
            generation,
            message,
        } => {
            if branch_view_generation_matches(state, &repo_path, generation) {
                state.branch_view.loading = false;
                state.mode = Mode::RepoSelect;
                state.push_toast(ToastKind::Error, message);
            }
        }
        AppEvent::OpenWorktreesLoaded {
            repo_path,
            generation,
            worktrees,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state.branch_view.open_worktrees = worktrees;
                state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Loaded {
                    repo_path,
                    generation,
                };
                state.branch_view.apply_open_indicators();
                crate::screens::delete::refresh_open_state(state);
                changes.resume_pending_deletes = true;
            }
        }
        AppEvent::OpenWorktreesFailed {
            repo_path,
            generation,
            message,
        } => {
            if branch_context_generation_matches(state, &repo_path, generation) {
                state.branch_view.open_worktrees.clear();
                state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Failed {
                    repo_path,
                    generation,
                };
                state.push_toast(ToastKind::Error, message);
            }
        }
        AppEvent::BranchOperationFailed { repo_path, failure } => {
            if branch_context_matches(state, &repo_path)
                && matches!(
                    state.mode,
                    Mode::Loading {
                        branch: Some(_),
                        ..
                    }
                )
            {
                let context = state.branch_context().cloned().unwrap();
                state.mode = Mode::BranchSelect(context);
                let message = match failure {
                    BranchOperationFailure::Failed(message) => message,
                    BranchOperationFailure::LocalBranchAvailable {
                        branch_name,
                        tracking_created,
                        message,
                    } => {
                        let selection = BranchId::Local(branch_name.clone());
                        state.branch_view.promote_remote_to_local(&branch_name);
                        state.branch_view.list.input.clear();
                        state.branch_view.pending_selection = Some(selection.clone());
                        changes.pinned_branch_selection = Some(selection);
                        changes.branches_changed = true;
                        state.branch_view.loading = true;
                        state.branch_view.reset_remotes();
                        changes.refresh_branch = state
                            .repos
                            .iter()
                            .find(|entry| entry.repo.path == repo_path)
                            .map(|entry| entry.repo.clone());
                        if tracking_created {
                            format!(
                                "Tracking branch {branch_name} was created, but its checkout could not be opened: {message}"
                            )
                        } else {
                            format!(
                                "Local branch {branch_name} now exists, but its checkout could not be opened: {message}"
                            )
                        }
                    }
                };
                state.push_toast(ToastKind::Error, message);
            }
        }
        event => return Some(event),
    }
    None
}

pub(crate) fn matches_repo(state: &AppState, repo_path: &Path) -> bool {
    matches!(&state.mode, Mode::BranchSelect(context) if context.repo_path == repo_path)
}

fn branch_view_generation_matches(state: &AppState, repo_path: &Path, generation: u64) -> bool {
    matches_repo(state, repo_path) && state.branch_view.generation == generation
}

fn branch_context_matches(state: &AppState, repo_path: &Path) -> bool {
    state
        .branch_context()
        .is_some_and(|context| context.repo_path == repo_path)
}

fn branch_context_generation_matches(state: &AppState, repo_path: &Path, generation: u64) -> bool {
    branch_context_matches(state, repo_path) && state.branch_view.generation == generation
}

fn pin_selection(state: &AppState, changes: &mut TickChanges) {
    if changes.pinned_branch_selection.is_none() {
        changes.pinned_branch_selection = state.selected_branch().map(BranchEntry::id);
    }
}

fn merge_remote_snapshot(
    state: &mut AppState,
    changes: &mut TickChanges,
    remote: String,
    branches: Vec<BranchEntry>,
) {
    let selected = state.selected_branch().map(BranchEntry::id);
    pin_selection(state, changes);
    let visible = state
        .branch_view
        .list
        .filtered
        .iter()
        .filter_map(|(index, score)| {
            state
                .branch_view
                .entries
                .get(*index)
                .map(|branch| (branch.id(), *score))
        })
        .collect::<Vec<_>>();

    state.branch_view.merge_remote_snapshot(remote, branches);
    let indices = state
        .branch_view
        .entries
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.id(), index))
        .collect::<std::collections::HashMap<_, _>>();
    state.branch_view.list.filtered = visible
        .iter()
        .filter_map(|(id, score)| indices.get(id).map(|index| (*index, *score)))
        .collect();
    state.branch_view.list.selected = selected
        .as_ref()
        .and_then(|id| {
            state
                .branch_view
                .list
                .filtered
                .iter()
                .position(|(index, _)| state.branch_view.entries[*index].id() == *id)
        })
        .or_else(|| (!state.branch_view.list.filtered.is_empty()).then_some(0));
}

pub(crate) fn queue_filter(
    state: &mut AppState,
    worker: &FilterWorker,
    selected_id: Option<BranchId>,
) {
    state.branch_view.filter_generation = state.branch_view.filter_generation.wrapping_add(1);
    if state.branch_view.list.input.text.is_empty() {
        state.branch_view.list.filtered = (0..state.branch_view.entries.len())
            .map(|index| (index, 0))
            .collect();
        if state.branch_view.entries.is_empty() {
            state.branch_view.list.selected = None;
        } else {
            state.branch_view.list.selected = selected_id
                .as_ref()
                .and_then(|id| {
                    state
                        .branch_view
                        .entries
                        .iter()
                        .position(|branch| branch.id() == *id)
                })
                .or(Some(0));
        }
        state.branch_view.list.scroll_offset = 0;
        return;
    }
    worker.request(FilterRequest {
        target: FilterTarget::Branches,
        generation: state.branch_view.filter_generation,
        query: state.branch_view.list.input.text.clone(),
        items: state
            .branch_view
            .entries
            .iter()
            .map(|branch| FilterItem {
                key: FilterKey::Branch(branch.id()),
                text: branch.display_name(),
            })
            .collect(),
        selected: selected_id.map(FilterKey::Branch),
    });
}

fn apply_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let current = state.selected_branch().map(BranchEntry::id);
    let indices = state
        .branch_view
        .entries
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.id(), index))
        .collect::<std::collections::HashMap<_, _>>();
    state.branch_view.list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Branch(id) => indices.get(id).map(|index| (*index, *score)),
            FilterKey::Repo(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
        })
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Branch(id) => Some(id.clone()),
        FilterKey::Repo(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
    });
    state.branch_view.list.selected = current
        .or(requested)
        .as_ref()
        .and_then(|id| {
            state
                .branch_view
                .list
                .filtered
                .iter()
                .position(|(index, _)| state.branch_view.entries[*index].id() == *id)
        })
        .or_else(|| (!state.branch_view.list.filtered.is_empty()).then_some(0));
    state.branch_view.list.scroll_offset = 0;
}

fn warn_unsupported_refs(state: &mut AppState, generation: u64, skipped: bool) {
    if state
        .branch_view
        .should_warn_unsupported_refs(generation, skipped)
    {
        state.push_toast(ToastKind::Warning, crate::git::UNSUPPORTED_REF_WARNING);
    }
}

pub(crate) fn enter(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Some(entry) = state.selected_repo() else {
        return;
    };
    let context = BranchContext {
        repo_path: entry.repo.path.clone(),
        repo_name: entry.repo.name.clone(),
    };
    let repo = entry.repo.clone();
    let repo_path = context.repo_path.clone();
    state.mode = Mode::BranchSelect(context);
    state.branch_view.entries.clear();
    state.branch_view.list = SearchableList::new(0);
    state.branch_view.open_worktrees.clear();
    state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Unknown;
    state.branch_view.loading = true;
    state.branch_view.reset_remotes();
    state.branch_view.filter_generation = state.branch_view.filter_generation.wrapping_add(1);
    advance_generation(state);
    let generation = state.branch_view.generation;
    spawn_branch_loading(git, sender, repo, state.current_cwd.clone(), generation);
    if let Some(provider) = herdr {
        spawn_open_worktrees(provider, sender, repo_path, generation);
    }
}

pub(crate) fn refresh(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
    repo: Repo,
) {
    advance_generation(state);
    let repo_path = repo.path.clone();
    let generation = state.branch_view.generation;
    spawn_branch_loading(git, sender, repo, state.current_cwd.clone(), generation);
    if let Some(provider) = herdr {
        spawn_open_worktrees(provider, sender, repo_path, generation);
    }
}

fn advance_generation(state: &mut AppState) {
    state.branch_view.generation = state
        .branch_view
        .generation
        .checked_add(1)
        .expect("branch view generation overflow");
}

pub(crate) fn open_selected(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Some(branch) = state.selected_branch().cloned() else {
        return;
    };
    open(state, git, herdr, sender, &branch);
}

pub(crate) fn open(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
    branch: &BranchEntry,
) {
    let Some(context) = state.branch_context().cloned() else {
        return;
    };
    let branch_name = branch.name.clone();
    let has_worktree = branch.worktree_path.is_some();
    let remote = branch.remote.clone();
    let Some(provider) = herdr else {
        state.push_toast(ToastKind::Error, "not running inside herdr");
        return;
    };
    let verb = if remote.is_some() {
        format!("Checking out remote branch {branch_name}…")
    } else if has_worktree {
        format!("Opening {branch_name}…")
    } else {
        format!("Creating worktree for {branch_name}…")
    };
    state.mode = Mode::Loading {
        message: verb,
        branch: Some(context.clone()),
    };
    if let Some(remote) = remote {
        spawn_open_remote_branch(
            git,
            provider,
            sender,
            context.repo_path,
            branch_name,
            remote,
            state.on_open.clone(),
        );
    } else {
        spawn_open_branch(
            provider,
            sender,
            context.repo_path,
            branch_name,
            has_worktree,
            state.on_open.clone(),
        );
    }
}

pub(crate) fn move_selection(state: &mut AppState, delta: i32) {
    state.branch_view.list.move_selection(delta);
}

pub(crate) fn edit(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    edit(&mut state.branch_view.list);
    queue_filter(state, worker, None);
}

pub(crate) fn leave(state: &mut AppState) {
    state.mode = Mode::RepoSelect;
    state.branch_view.reset_remotes();
}

#[cfg(test)]
mod tests;
