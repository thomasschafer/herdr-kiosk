use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    app::{FilterItem, FilterRequest, FilterWorker, TickChanges},
    event::{AppEvent, FilterKey, FilterTarget},
    git::Repo,
    herdr::HerdrProvider,
    spawn::{EventSender, spawn_open_repo},
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
}

pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    changes: &mut TickChanges,
) -> Option<AppEvent> {
    match event {
        AppEvent::ReposFound { repo } => {
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
                .map(|worktree| canonical_or_original(Path::new(&worktree.repo_root)))
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
        AppEvent::OpenWorkspacesFailed(message) => {
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
        state.repo_view.canonical_sort();
        state.repo_view.apply_current_selection();
    }
    if changes.collision_pass {
        apply_collisions(state);
        state.repo_view.canonical_sort();
        state.repo_view.apply_current_selection();
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
        state.repo_view.canonical_sort();
        if !preserve_selection {
            state.repo_view.list.selected = (!state.repo_view.entries.is_empty()).then_some(0);
        }
        if preserve_selection {
            state.repo_view.apply_current_selection();
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
    });
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
    let Some(provider) = herdr else {
        state.push_toast(ToastKind::Error, "not running inside herdr");
        return;
    };
    state.mode = Mode::Loading {
        message: format!("Opening {repo_name}…"),
        branch: None,
    };
    spawn_open_repo(provider, sender, repo_path, state.on_open.clone());
}

fn add(state: &mut AppState, repo: Repo) -> bool {
    if !state.repo_view.seen_paths.insert(repo.path.clone()) {
        return false;
    }
    let mut entry = RepoEntry::new(repo);
    entry.is_open = state
        .repo_view
        .open_roots
        .contains(&canonical_or_original(&entry.repo.path));
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

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn apply_open_indicators(state: &mut AppState) {
    for entry in &mut state.repo_view.entries {
        let repo_path = canonical_or_original(&entry.repo.path);
        entry.is_open = state
            .repo_view
            .open_roots
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
