use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    app::{FilterItem, FilterOrdering, FilterRequest, FilterWorker, TickChanges},
    config::SortOrder,
    event::{AppEvent, FilterKey, FilterTarget},
    git::Repo,
    herdr::HerdrProvider,
    recency::RecencyStore,
    spawn::{EventSender, spawn_open_folder, spawn_open_repo},
    state::{AppState, Mode, SearchableList, ToastKind},
};

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

#[derive(Debug)]
pub struct RepoViewState {
    pub entries: Vec<RepoEntry>,
    pub list: SearchableList,
    pub loading: bool,
    pub selection_touched: bool,
    seen_paths: HashSet<PathBuf>,
    open_roots: HashSet<PathBuf>,
    open_folder_roots: HashSet<PathBuf>,
    folder_indicators_requested: bool,
    pub(crate) current_cwd: Option<PathBuf>,
    pub(crate) filter_generation: u64,
}

impl RepoViewState {
    pub fn new(current_cwd: Option<PathBuf>) -> Self {
        Self {
            entries: Vec::new(),
            list: SearchableList::new(0),
            loading: true,
            selection_touched: false,
            seen_paths: HashSet::new(),
            open_roots: HashSet::new(),
            open_folder_roots: HashSet::new(),
            folder_indicators_requested: false,
            current_cwd,
            filter_generation: 0,
        }
    }

    pub fn selected(&self) -> Option<&RepoEntry> {
        let selected = self.list.selected?;
        let index = self.list.filtered.get(selected)?.0;
        self.entries.get(index)
    }

    fn canonical_sort(&mut self) {
        let selected_path = self.selected().map(|entry| entry.repo.path.clone());
        let filtered_paths = self
            .list
            .filtered
            .iter()
            .filter_map(|(index, score)| {
                self.entries
                    .get(*index)
                    .map(|entry| (entry.repo.path.clone(), *score))
            })
            .collect::<Vec<_>>();
        self.entries.sort_by(|left, right| {
            left.repo
                .name
                .to_lowercase()
                .cmp(&right.repo.name.to_lowercase())
                .then(left.repo.name.cmp(&right.repo.name))
                .then(left.repo.path.cmp(&right.repo.path))
        });
        if self.list.input.text.is_empty() {
            self.list.filtered = (0..self.entries.len()).map(|index| (index, 0)).collect();
            self.list.selected = selected_path
                .and_then(|path| {
                    self.entries
                        .iter()
                        .position(|entry| entry.repo.path == path)
                })
                .or_else(|| (!self.entries.is_empty()).then_some(0));
        } else {
            let indices: HashMap<_, _> = self
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.repo.path.as_path(), index))
                .collect();
            self.list.filtered = filtered_paths
                .iter()
                .filter_map(|(path, score)| {
                    indices.get(path.as_path()).map(|index| (*index, *score))
                })
                .collect();
            self.list.selected = selected_path.and_then(|path| {
                self.list
                    .filtered
                    .iter()
                    .position(|(index, _)| self.entries[*index].repo.path == path)
            });
        }
    }

    fn recency_sort(&mut self, recency: &RecencyStore) {
        self.canonical_sort();
        let selected_path = self.selected().map(|entry| entry.repo.path.clone());
        let filtered_paths = self
            .list
            .filtered
            .iter()
            .filter_map(|(index, score)| {
                self.entries
                    .get(*index)
                    .map(|entry| (entry.repo.path.clone(), *score))
            })
            .collect::<Vec<_>>();
        self.entries
            .sort_by_key(|entry| recency.repo_rank(&entry.repo.path).unwrap_or(usize::MAX));
        if self.list.input.text.is_empty() {
            self.list.filtered = (0..self.entries.len()).map(|index| (index, 0)).collect();
        } else {
            let indices: HashMap<_, _> = self
                .entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.repo.path.as_path(), index))
                .collect();
            self.list.filtered = filtered_paths
                .iter()
                .filter_map(|(path, score)| {
                    indices.get(path.as_path()).map(|index| (*index, *score))
                })
                .collect();
        }
        self.list.selected = selected_path
            .and_then(|path| {
                self.list
                    .filtered
                    .iter()
                    .position(|(index, _)| self.entries[*index].repo.path == path)
            })
            .or_else(|| (!self.entries.is_empty()).then_some(0));
    }

    pub(crate) fn apply_current_selection(&mut self) {
        if self.selection_touched {
            return;
        }
        let Some(cwd) = self.current_cwd.as_deref() else {
            return;
        };
        let best = self
            .list
            .filtered
            .iter()
            .enumerate()
            .filter(|(_, (index, _))| {
                crate::path::starts_with(cwd, &self.entries[*index].repo.path)
            })
            .max_by_key(|(_, (index, _))| self.entries[*index].repo.path.components().count())
            .map(|(position, _)| position);
        if best.is_some() {
            self.list.selected = best;
        }
    }

    fn apply_previous_selection(&mut self) {
        if self.selection_touched {
            return;
        }
        let Some(cwd) = self.current_cwd.as_deref() else {
            return;
        };
        if let Some(previous) =
            self.list.filtered.iter().position(|(index, _)| {
                !crate::path::starts_with(cwd, &self.entries[*index].repo.path)
            })
        {
            self.list.selected = Some(previous);
        }
    }
}

pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    changes: &mut TickChanges,
) -> Option<AppEvent> {
    match event {
        AppEvent::ReposFound { repo } => {
            if !repo.is_git && !state.repo_view.folder_indicators_requested {
                state.repo_view.folder_indicators_requested = true;
                changes.load_open_folder_indicators = true;
            }
            changes.repos_changed |= add(state, repo);
        }
        AppEvent::ScanComplete => {
            state.repo_view.loading = false;
            changes.collision_pass = true;
        }
        AppEvent::ScanWarning(_warning) => {
            state.push_scan_warning();
        }
        AppEvent::OpenWorkspacesLoaded { workspaces } => {
            state.repo_view.open_roots = workspaces
                .iter()
                .filter_map(|workspace| workspace.worktree.as_ref())
                .map(|worktree| crate::path::canonical_or_original(Path::new(&worktree.repo_root)))
                .collect();
            apply_open_indicators(state);
        }
        AppEvent::OpenFolderPanesLoaded { panes } => {
            state.repo_view.open_folder_roots = panes
                .iter()
                .filter_map(|pane| pane.cwd.as_deref())
                .map(Path::new)
                .map(crate::path::canonical_or_original)
                .collect();
            apply_open_indicators(state);
        }
        AppEvent::FilterCompleted {
            target: FilterTarget::Repos,
            generation,
            matches,
            selected,
        } if generation == state.repo_view.filter_generation => {
            apply_filter_result(state, &matches, selected.as_ref());
        }
        AppEvent::RepoOpenFailed(message)
            if matches!(state.mode, Mode::Loading { branch: None, .. }) =>
        {
            state.mode = Mode::RepoSelect;
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::OpenWorkspacesFailed(message) | AppEvent::OpenFolderPanesFailed(message) => {
            state.push_toast(ToastKind::Warning, message);
        }
        event => return Some(event),
    }
    None
}

pub(crate) fn apply_changes(
    state: &mut AppState,
    changes: &mut TickChanges,
    worker: &FilterWorker,
) {
    if changes.repos_changed {
        sort_entries(state);
        apply_default_selection(state);
    }
    if changes.collision_pass {
        apply_collisions(state);
        sort_entries(state);
        apply_default_selection(state);
        changes.repos_changed = true;
    }
    if changes.repos_changed && matches!(state.mode, Mode::RepoSelect) {
        queue_filter(state, worker, true);
    }
}

pub(crate) fn move_selection(state: &mut AppState, delta: i32) {
    state.repo_view.selection_touched = true;
    state.repo_view.list.move_selection(delta);
}

pub(crate) fn edit(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    state.repo_view.selection_touched = true;
    edit(&mut state.repo_view.list);
    queue_filter(state, worker, false);
}

pub(crate) fn queue_filter(state: &mut AppState, worker: &FilterWorker, preserve_selection: bool) {
    state.repo_view.filter_generation = state.repo_view.filter_generation.wrapping_add(1);
    if state.repo_view.list.input.text.is_empty() {
        sort_entries(state);
        if !preserve_selection {
            state.repo_view.list.selected = (!state.repo_view.entries.is_empty()).then_some(0);
        }
        if preserve_selection {
            apply_default_selection(state);
        }
        return;
    }
    let selected = preserve_selection
        .then(|| state.selected_repo().map(|entry| entry.repo.path.clone()))
        .flatten()
        .map(FilterKey::Repo);
    worker.request(FilterRequest {
        target: FilterTarget::Repos,
        generation: state.repo_view.filter_generation,
        query: state.repo_view.list.input.text.clone(),
        items: state
            .repo_view
            .entries
            .iter()
            .map(|entry| FilterItem {
                key: FilterKey::Repo(entry.repo.path.clone()),
                text: entry.display_name(),
            })
            .collect(),
        selected,
        ordering: match state.sort_order {
            SortOrder::Alphabetical => FilterOrdering::Alphabetical,
            SortOrder::Recency => FilterOrdering::Recency(
                state
                    .repo_view
                    .entries
                    .iter()
                    .filter_map(|entry| {
                        state
                            .recency
                            .repo_rank(&entry.repo.path)
                            .map(|rank| (FilterKey::Repo(entry.repo.path.clone()), rank))
                    })
                    .collect(),
            ),
        },
    });
}

fn sort_entries(state: &mut AppState) {
    match state.sort_order {
        SortOrder::Alphabetical => state.repo_view.canonical_sort(),
        SortOrder::Recency => state.repo_view.recency_sort(&state.recency),
    }
}

fn apply_default_selection(state: &mut AppState) {
    match state.sort_order {
        SortOrder::Alphabetical => state.repo_view.apply_current_selection(),
        SortOrder::Recency => state.repo_view.apply_previous_selection(),
    }
}

pub(crate) fn open_selected(
    state: &mut AppState,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Some(entry) = state.selected_repo() else {
        return;
    };
    let repo_path = entry.repo.path.clone();
    let repo_name = entry.repo.name.clone();
    let is_git = entry.repo.is_git;
    let Some(provider) = herdr else {
        state.push_toast(ToastKind::Error, "not running inside herdr");
        return;
    };
    state.mode = Mode::Loading {
        message: format!("Opening {repo_name}…"),
        branch: None,
    };
    if is_git {
        spawn_open_repo(provider, sender, repo_path, state.on_open.clone());
    } else {
        spawn_open_folder(provider, sender, repo_path);
    }
}

fn add(state: &mut AppState, repo: Repo) -> bool {
    if !state.repo_view.seen_paths.insert(repo.path.clone()) {
        return false;
    }
    let mut entry = RepoEntry::new(repo);
    let canonical = crate::path::canonical_or_original(&entry.repo.path);
    entry.is_open = if entry.repo.is_git {
        state.repo_view.open_roots.contains(&canonical)
    } else {
        state.repo_view.open_folder_roots.contains(&canonical)
    };
    state.repo_view.entries.push(entry);
    true
}

fn apply_collisions(state: &mut AppState) {
    let repos = state
        .repo_view
        .entries
        .iter()
        .map(|entry| entry.repo.clone())
        .collect::<Vec<_>>();
    let disambiguators = collision_disambiguators(&repos);
    for (entry, disambiguator) in state.repo_view.entries.iter_mut().zip(disambiguators) {
        entry.disambiguator = disambiguator;
    }
}

fn apply_open_indicators(state: &mut AppState) {
    for entry in &mut state.repo_view.entries {
        let repo_path = crate::path::canonical_or_original(&entry.repo.path);
        let open_paths = if entry.repo.is_git {
            &state.repo_view.open_roots
        } else {
            &state.repo_view.open_folder_roots
        };
        entry.is_open = open_paths
            .iter()
            .any(|open_path| crate::path::equivalent(open_path, &repo_path));
    }
}

fn apply_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let current = state.selected_repo().map(|entry| entry.repo.path.clone());
    let indices: HashMap<_, _> = state
        .repo_view
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.repo.path.as_path(), index))
        .collect();
    state.repo_view.list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Repo(path) => indices.get(path.as_path()).map(|index| (*index, *score)),
            FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
        })
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Repo(path) => Some(path.clone()),
        FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
    });
    state.repo_view.list.selected = current
        .or(requested)
        .as_ref()
        .and_then(|path| {
            state
                .repo_view
                .list
                .filtered
                .iter()
                .position(|(index, _)| state.repo_view.entries[*index].repo.path == *path)
        })
        .or_else(|| (!state.repo_view.list.filtered.is_empty()).then_some(0));
    state.repo_view.list.scroll_offset = 0;
}

fn collision_disambiguators(repos: &[Repo]) -> Vec<Option<String>> {
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

#[cfg(test)]
#[path = "repo/tests.rs"]
mod tests;
