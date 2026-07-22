use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use fuzzy_matcher::{FuzzyMatcher, skim::SkimMatcherV2};
use ratatui::{
    DefaultTerminal, Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::{
    components,
    event::{AppEvent, FilterKey, FilterTarget},
    git::{GitProvider, Repo},
    herdr::HerdrProvider,
    keyboard::Action,
    spawn::{
        EventSender, spawn_branch_loading, spawn_git_fetch, spawn_open_branch,
        spawn_open_remote_branch, spawn_open_repo, spawn_open_worktrees,
        spawn_remote_branch_loading, spawn_repo_discovery, spawn_workspace_list,
    },
    state::{
        AppState, BranchContext, Mode, RepoEntry, SearchableList, ToastKind,
        collision_disambiguators,
    },
    theme::Theme,
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(40);
const MAX_EVENTS_PER_TICK: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Quit,
    Opened,
}

#[derive(Clone)]
struct FilterItem {
    key: FilterKey,
    text: String,
}

struct FilterRequest {
    target: FilterTarget,
    generation: u64,
    query: String,
    items: Vec<FilterItem>,
    selected: Option<FilterKey>,
}

struct FilterWorker {
    pending: Arc<(Mutex<Option<FilterRequest>>, Condvar)>,
    cancel: Arc<AtomicBool>,
}

impl FilterWorker {
    fn spawn(sender: EventSender) -> Self {
        let pending = Arc::new((Mutex::new(None::<FilterRequest>), Condvar::new()));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_pending = Arc::clone(&pending);
        let thread_cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            let matcher = SkimMatcherV2::default();
            loop {
                let request = {
                    let (lock, condition) = &*thread_pending;
                    let mut request = lock.lock().unwrap();
                    while request.is_none() && !thread_cancel.load(Ordering::Relaxed) {
                        request = condition.wait(request).unwrap();
                    }
                    if thread_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    request.take().unwrap()
                };
                let filtered = fuzzy_filter(&request.query, &request.items, &matcher);
                sender.send(AppEvent::FilterCompleted {
                    target: request.target,
                    generation: request.generation,
                    matches: filtered,
                    selected: request.selected,
                });
            }
        });
        Self { pending, cancel }
    }

    fn request(&self, request: FilterRequest) {
        let (lock, condition) = &*self.pending;
        *lock.lock().unwrap() = Some(request);
        condition.notify_one();
    }
}

impl Drop for FilterWorker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.pending.1.notify_one();
    }
}

fn fuzzy_filter(
    query: &str,
    items: &[FilterItem],
    matcher: &SkimMatcherV2,
) -> Vec<(FilterKey, i64)> {
    if query.is_empty() {
        return items.iter().map(|item| (item.key.clone(), 0)).collect();
    }
    let mut scored: Vec<_> = items
        .iter()
        .filter_map(|item| {
            matcher
                .fuzzy_match(&item.text, query)
                .map(|score| (item, score))
        })
        .collect();
    scored.sort_by(|(left, left_score), (right, right_score)| {
        right_score
            .cmp(left_score)
            .then(left.text.len().cmp(&right.text.len()))
            .then(left.text.cmp(&right.text))
    });
    scored
        .into_iter()
        .map(|(item, score)| (item.key.clone(), score))
        .collect()
}

pub fn run(
    terminal: &mut DefaultTerminal,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    search_dirs: Vec<(PathBuf, u16)>,
    theme: &Theme,
) -> Result<RunOutcome> {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let sender = EventSender::new(tx, Arc::clone(&cancel));
    let filter_worker = FilterWorker::spawn(sender.clone());
    let spinner_start = Instant::now();

    spawn_repo_discovery(git, &sender, search_dirs);
    if let Some(provider) = herdr {
        spawn_workspace_list(provider, &sender);
    }

    let outcome = loop {
        terminal.draw(|frame| draw(frame, state, theme, spinner_start))?;

        let mut changes = TickChanges::default();
        for app_event in rx.try_iter().take(MAX_EVENTS_PER_TICK) {
            process_app_event(app_event, state, &mut changes);
        }

        if changes.workspace_opened {
            break RunOutcome::Opened;
        }

        if changes.repos_changed {
            state.canonical_sort();
            state.apply_current_repo_selection();
        }
        if changes.collision_pass {
            apply_collisions(state);
            state.canonical_sort();
            state.apply_current_repo_selection();
            changes.repos_changed = true;
        }
        if changes.repos_changed && matches!(state.mode, Mode::RepoSelect) {
            queue_repo_filter(state, &filter_worker, true);
        }
        if changes.branches_changed {
            queue_branch_filter(
                state,
                &filter_worker,
                changes.pinned_branch_selection.take(),
            );
        }
        if let Some((repo_path, local_names)) = changes.start_remote_loading.take() {
            spawn_remote_branch_loading(git, &sender, repo_path.clone(), local_names.clone());
            spawn_git_fetch(git, &sender, repo_path, local_names);
        }

        if event::poll(EVENT_POLL_INTERVAL)?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let Some(action) = crate::keymap::resolve_action(key, state) else {
                continue;
            };
            if let Some(outcome) =
                process_action(action, state, git, herdr, &sender, &filter_worker)
            {
                break outcome;
            }
        }
    };

    cancel.store(true, Ordering::Relaxed);
    drop(filter_worker);
    Ok(outcome)
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
struct TickChanges {
    repos_changed: bool,
    branches_changed: bool,
    collision_pass: bool,
    workspace_opened: bool,
    pinned_branch_selection: Option<String>,
    start_remote_loading: Option<(PathBuf, Vec<String>)>,
}

#[allow(clippy::too_many_lines)]
fn process_app_event(event: AppEvent, state: &mut AppState, changes: &mut TickChanges) {
    match event {
        AppEvent::ReposFound { repo } => changes.repos_changed |= add_repo(state, repo),
        AppEvent::RepoEnriched {
            repo_path,
            worktrees,
        } => {
            if let Some(entry) = state
                .repos
                .iter_mut()
                .find(|entry| entry.repo.path == repo_path)
            {
                entry.repo.worktrees = worktrees;
            }
        }
        AppEvent::ScanComplete { .. } => {
            state.loading_repos = false;
            changes.collision_pass = true;
        }
        AppEvent::ScanWarning(warning) => {
            let message = if warning.path.as_os_str().is_empty() {
                warning.message
            } else {
                format!("{}: {}", warning.path.display(), warning.message)
            };
            state.push_toast(ToastKind::Warning, message);
        }
        AppEvent::OpenWorkspacesLoaded { workspaces } => {
            state.open_repo_roots = workspaces
                .iter()
                .filter_map(|workspace| workspace.worktree.as_ref())
                .map(|worktree| canonical_or_original(Path::new(&worktree.repo_root)))
                .collect();
            apply_open_indicators(state);
        }
        AppEvent::FilterCompleted {
            target,
            generation,
            matches,
            selected,
        } => match target {
            FilterTarget::Repos if generation == state.repo_filter_generation => {
                apply_repo_filter_result(state, &matches, selected.as_ref());
            }
            FilterTarget::Branches if generation == state.branch_filter_generation => {
                apply_branch_filter_result(state, &matches, selected.as_ref());
            }
            FilterTarget::Repos | FilterTarget::Branches => {}
        },
        AppEvent::BranchesLoaded {
            repo_path,
            branches,
            worktrees,
        } if branch_view_matches(state, &repo_path) => {
            if let Some(entry) = state
                .repos
                .iter_mut()
                .find(|entry| entry.repo.path == repo_path)
            {
                entry.repo.worktrees = worktrees;
            }
            state.branches = branches;
            state.remote_branches.clear();
            apply_branch_open_indicators(state);
            state.loading_branches = false;
            state.fetching_remote_repo = Some(repo_path.clone());
            changes.start_remote_loading = Some((
                repo_path,
                state
                    .branches
                    .iter()
                    .map(|branch| branch.name.clone())
                    .collect(),
            ));
            changes.branches_changed = true;
        }
        AppEvent::RemoteBranchesLoaded {
            repo_path,
            remote,
            branches,
        } if branch_context_matches(state, &repo_path) => {
            merge_remote_snapshot(state, changes, remote, branches);
            apply_branch_open_indicators(state);
            changes.branches_changed = true;
        }
        AppEvent::RemoteBranchLoadFailed { repo_path, message }
            if branch_context_matches(state, &repo_path) =>
        {
            state.push_toast(ToastKind::Warning, message);
        }
        AppEvent::GitFetchCompleted {
            remote,
            branches,
            repo_path,
            error,
            is_final,
        } if branch_context_matches(state, &repo_path) => {
            if let Some(remote) = remote {
                merge_remote_snapshot(state, changes, remote.clone(), branches);
                apply_branch_open_indicators(state);
                changes.branches_changed = true;
                if let Some(error) = error
                    && state.fetch_warning_remotes.insert(remote.clone())
                {
                    state.push_toast(
                        ToastKind::Warning,
                        format!("could not fetch remote {remote}: {error}"),
                    );
                }
            } else if let Some(error) = error {
                state.push_toast(ToastKind::Warning, error);
            }
            if is_final && state.fetching_remote_repo.as_deref() == Some(repo_path.as_path()) {
                state.fetching_remote_repo = None;
            }
        }
        AppEvent::BranchLoadFailed { repo_path, message }
            if branch_view_matches(state, &repo_path) =>
        {
            state.loading_branches = false;
            state.mode = Mode::RepoSelect;
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::OpenWorktreesLoaded {
            repo_path,
            worktrees,
        } if branch_context_matches(state, &repo_path) => {
            state.open_worktrees = worktrees;
            apply_branch_open_indicators(state);
        }
        AppEvent::OpenWorktreesFailed { repo_path, message }
            if branch_context_matches(state, &repo_path) =>
        {
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::RepoOpened => changes.workspace_opened = true,
        AppEvent::RepoOpenFailed(message)
            if matches!(state.mode, Mode::Loading { branch: None, .. }) =>
        {
            state.mode = Mode::RepoSelect;
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::BranchOperationFailed { repo_path, message }
            if branch_context_matches(state, &repo_path)
                && matches!(
                    state.mode,
                    Mode::Loading {
                        branch: Some(_),
                        ..
                    }
                ) =>
        {
            let context = state.branch_context().cloned().unwrap();
            state.mode = Mode::BranchSelect(context);
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::OpenWorkspacesFailed(message) | AppEvent::GitError(message) => {
            state.push_toast(ToastKind::Warning, message);
        }
        _ => {}
    }
}

fn process_action(
    action: Action,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
    filter_worker: &FilterWorker,
) -> Option<RunOutcome> {
    match action {
        Action::Quit => return Some(RunOutcome::Quit),
        Action::MoveSelection(delta) => {
            if matches!(state.mode, Mode::RepoSelect) {
                state.selection_touched = true;
                state.repo_list.move_selection(delta);
            } else if matches!(state.mode, Mode::BranchSelect(_)) {
                state.branch_list.move_selection(delta);
            }
        }
        Action::Insert(character) => {
            edit_active_list(state, filter_worker, |list| {
                list.input.insert_char(character);
            });
        }
        Action::Backspace => {
            edit_active_list(state, filter_worker, |list| list.input.backspace());
        }
        Action::DeleteWord => {
            edit_active_list(state, filter_worker, |list| list.input.delete_word());
        }
        Action::CursorLeft => active_list_mut(state).input.cursor_left(),
        Action::CursorRight => active_list_mut(state).input.cursor_right(),
        Action::ClearQuery => {
            edit_active_list(state, filter_worker, |list| list.input.clear());
        }
        Action::OpenRepo => begin_open_selected(state, herdr, sender),
        Action::OpenBranches => begin_branch_select(state, git, herdr, sender),
        Action::OpenBranch => begin_open_selected_branch(state, git, herdr, sender),
        Action::BackToRepos => {
            state.mode = Mode::RepoSelect;
            state.reset_remote_branches();
            queue_repo_filter(state, filter_worker, true);
        }
        Action::DismissToast => {
            state.toasts.pop_front();
        }
        Action::Noop => {}
    }
    None
}

fn active_list_mut(state: &mut AppState) -> &mut SearchableList {
    match state.mode {
        Mode::BranchSelect(_) => &mut state.branch_list,
        Mode::RepoSelect | Mode::Loading { .. } => &mut state.repo_list,
    }
}

fn edit_active_list(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    match state.mode {
        Mode::RepoSelect => {
            state.selection_touched = true;
            edit(&mut state.repo_list);
            queue_repo_filter(state, worker, false);
        }
        Mode::BranchSelect(_) => {
            edit(&mut state.branch_list);
            queue_branch_filter(state, worker, None);
        }
        Mode::Loading { .. } => {}
    }
}

fn branch_view_matches(state: &AppState, repo_path: &Path) -> bool {
    matches!(&state.mode, Mode::BranchSelect(context) if context.repo_path == repo_path)
}

fn branch_context_matches(state: &AppState, repo_path: &Path) -> bool {
    state
        .branch_context()
        .is_some_and(|context| context.repo_path == repo_path)
}

fn add_repo(state: &mut AppState, repo: Repo) -> bool {
    if !state.seen_repo_paths.insert(repo.path.clone()) {
        return false;
    }
    let mut entry = RepoEntry::new(repo);
    entry.is_open = state
        .open_repo_roots
        .contains(&canonical_or_original(&entry.repo.path));
    state.repos.push(entry);
    true
}

fn apply_collisions(state: &mut AppState) {
    let repos = state
        .repos
        .iter()
        .map(|entry| entry.repo.clone())
        .collect::<Vec<_>>();
    let disambiguators = collision_disambiguators(&repos);
    for (entry, disambiguator) in state.repos.iter_mut().zip(disambiguators) {
        entry.disambiguator = disambiguator;
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn apply_open_indicators(state: &mut AppState) {
    for entry in &mut state.repos {
        entry.is_open = state
            .open_repo_roots
            .contains(&canonical_or_original(&entry.repo.path));
    }
}

fn apply_branch_open_indicators(state: &mut AppState) {
    for branch in &mut state.branches {
        branch.apply_open_worktrees(&state.open_worktrees);
    }
}

fn pin_branch_selection(state: &AppState, changes: &mut TickChanges) {
    if changes.pinned_branch_selection.is_none() {
        changes.pinned_branch_selection = state.selected_branch().map(|branch| branch.name.clone());
    }
}

fn merge_remote_snapshot(
    state: &mut AppState,
    changes: &mut TickChanges,
    remote: String,
    branches: Vec<crate::state::BranchEntry>,
) {
    let selected = state.selected_branch().map(|branch| branch.name.clone());
    pin_branch_selection(state, changes);
    let visible = state
        .branch_list
        .filtered
        .iter()
        .filter_map(|(index, score)| {
            state
                .branches
                .get(*index)
                .map(|branch| (branch.name.clone(), *score))
        })
        .collect::<Vec<_>>();

    state.merge_remote_branches(remote, branches);
    let indices: HashMap<_, _> = state
        .branches
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.name.as_str(), index))
        .collect();
    state.branch_list.filtered = visible
        .iter()
        .filter_map(|(name, score)| indices.get(name.as_str()).map(|index| (*index, *score)))
        .collect();
    state.branch_list.selected = selected
        .as_deref()
        .and_then(|name| {
            state
                .branch_list
                .filtered
                .iter()
                .position(|(index, _)| state.branches[*index].name == name)
        })
        .or_else(|| (!state.branch_list.filtered.is_empty()).then_some(0));
}

fn queue_repo_filter(state: &mut AppState, worker: &FilterWorker, preserve_selection: bool) {
    state.repo_filter_generation = state.repo_filter_generation.wrapping_add(1);
    if state.repo_list.input.text.is_empty() {
        state.canonical_sort();
        if !preserve_selection {
            state.repo_list.selected = (!state.repos.is_empty()).then_some(0);
        }
        if preserve_selection {
            state.apply_current_repo_selection();
        }
        return;
    }
    let selected = preserve_selection
        .then(|| state.selected_repo().map(|entry| entry.repo.path.clone()))
        .flatten()
        .map(FilterKey::Repo);
    worker.request(FilterRequest {
        target: FilterTarget::Repos,
        generation: state.repo_filter_generation,
        query: state.repo_list.input.text.clone(),
        items: state
            .repos
            .iter()
            .map(|entry| FilterItem {
                key: FilterKey::Repo(entry.repo.path.clone()),
                text: entry.display_name(),
            })
            .collect(),
        selected,
    });
}

fn queue_branch_filter(state: &mut AppState, worker: &FilterWorker, selected_name: Option<String>) {
    state.branch_filter_generation = state.branch_filter_generation.wrapping_add(1);
    if state.branch_list.input.text.is_empty() {
        state.branch_list.filtered = (0..state.branches.len()).map(|index| (index, 0)).collect();
        if state.branches.is_empty() {
            state.branch_list.selected = None;
        } else {
            state.branch_list.selected = selected_name
                .as_deref()
                .and_then(|name| state.branches.iter().position(|branch| branch.name == name))
                .or(Some(0));
        }
        state.branch_list.scroll_offset = 0;
        return;
    }
    let selected = selected_name.map(FilterKey::Branch);
    worker.request(FilterRequest {
        target: FilterTarget::Branches,
        generation: state.branch_filter_generation,
        query: state.branch_list.input.text.clone(),
        items: state
            .branches
            .iter()
            .map(|branch| FilterItem {
                key: FilterKey::Branch(branch.name.clone()),
                text: branch.name.clone(),
            })
            .collect(),
        selected,
    });
}

fn apply_repo_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let indices: HashMap<_, _> = state
        .repos
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.repo.path.as_path(), index))
        .collect();
    state.repo_list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Repo(path) => indices.get(path.as_path()).map(|index| (*index, *score)),
            FilterKey::Branch(_) => None,
        })
        .collect();
    state.repo_list.selected = selected
        .and_then(|key| match key {
            FilterKey::Repo(path) => Some(path),
            FilterKey::Branch(_) => None,
        })
        .and_then(|path| {
            state
                .repo_list
                .filtered
                .iter()
                .position(|(index, _)| state.repos[*index].repo.path == *path)
        })
        .or_else(|| (!state.repo_list.filtered.is_empty()).then_some(0));
    state.repo_list.scroll_offset = 0;
}

fn apply_branch_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let indices: HashMap<_, _> = state
        .branches
        .iter()
        .enumerate()
        .map(|(index, branch)| (branch.name.as_str(), index))
        .collect();
    state.branch_list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Branch(name) => indices.get(name.as_str()).map(|index| (*index, *score)),
            FilterKey::Repo(_) => None,
        })
        .collect();
    state.branch_list.selected = selected
        .and_then(|key| match key {
            FilterKey::Branch(name) => Some(name),
            FilterKey::Repo(_) => None,
        })
        .and_then(|name| {
            state
                .branch_list
                .filtered
                .iter()
                .position(|(index, _)| state.branches[*index].name == *name)
        })
        .or_else(|| (!state.branch_list.filtered.is_empty()).then_some(0));
    state.branch_list.scroll_offset = 0;
}

fn begin_open_selected(
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
    spawn_open_repo(provider, sender, repo_path);
}

fn begin_branch_select(
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
    state.branches.clear();
    state.branch_list = SearchableList::new(0);
    state.open_worktrees.clear();
    state.loading_branches = true;
    state.reset_remote_branches();
    state.branch_filter_generation = state.branch_filter_generation.wrapping_add(1);
    spawn_branch_loading(git, sender, repo, state.current_cwd.clone());
    if let Some(provider) = herdr {
        spawn_open_worktrees(provider, sender, repo_path);
    }
}

fn begin_open_selected_branch(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Some(context) = state.branch_context().cloned() else {
        return;
    };
    let Some(branch) = state.selected_branch() else {
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
        );
    } else {
        spawn_open_branch(
            provider,
            sender,
            context.repo_path,
            branch_name,
            has_worktree,
        );
    }
}

fn draw(frame: &mut Frame, state: &mut AppState, theme: &Theme, spinner_start: Instant) {
    if let Mode::Loading { message, .. } = &state.mode {
        let spinner =
            components::repo_list::SPINNER_FOR_LOADING[(spinner_start.elapsed().as_millis() / 80)
                as usize
                % components::repo_list::SPINNER_FOR_LOADING.len()];
        let [_, area, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(2),
            Constraint::Fill(1),
        ])
        .areas(frame.area());
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("{spinner} {message}"),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "Ctrl+C to cancel",
                    Style::default().fg(theme.muted),
                )),
            ])
            .alignment(Alignment::Center),
            area,
        );
        return;
    }

    let [main_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    match &state.mode {
        Mode::RepoSelect => {
            components::repo_list::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::BranchSelect(_) => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::Loading { .. } => unreachable!("loading mode returned above"),
    }
    let mut footer = vec![
        Span::styled("↑/↓", Style::default().fg(theme.hint)),
        Span::raw(" move  "),
        Span::styled("Enter", Style::default().fg(theme.hint)),
        Span::raw(" open  "),
    ];
    if matches!(state.mode, Mode::RepoSelect) {
        footer.extend([
            Span::styled("Tab", Style::default().fg(theme.hint)),
            Span::raw(" branches  "),
        ]);
    }
    let escape_hint = if matches!(state.mode, Mode::BranchSelect(_)) {
        "back"
    } else {
        "clear/quit"
    };
    footer.extend([
        Span::styled("Esc", Style::default().fg(theme.hint)),
        Span::raw(format!(" {escape_hint}  ")),
        Span::styled("Ctrl+C", Style::default().fg(theme.hint)),
        Span::raw(" quit"),
    ]);
    frame.render_widget(
        Paragraph::new(Line::from(footer)).alignment(Alignment::Center),
        footer_area,
    );
    components::error_toast::draw(frame, frame.area(), state, theme);
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    use crate::{
        git::{Repo, Worktree, mock::MockGitProvider},
        herdr::{
            AgentStatus, HerdrError, HerdrProvider, WorkspaceInfo, WorktreeCreateResponse,
            WorktreeInfo, WorktreeOpenResponse,
            mock::{HerdrCall, MockHerdrProvider},
        },
        state::BranchEntry,
    };

    use super::*;

    fn items(names: &[&str]) -> Vec<FilterItem> {
        names
            .iter()
            .map(|name| FilterItem {
                key: FilterKey::Branch((*name).into()),
                text: (*name).into(),
            })
            .collect()
    }

    fn names(matches: &[(FilterKey, i64)]) -> Vec<String> {
        matches
            .iter()
            .map(|(key, _)| match key {
                FilterKey::Repo(path) => path.file_name().unwrap().to_string_lossy().into_owned(),
                FilterKey::Branch(name) => name.clone(),
            })
            .collect()
    }

    #[test]
    fn empty_search_preserves_canonical_order() {
        let items = items(&["zebra", "apple", "mango"]);
        assert_eq!(
            names(&fuzzy_filter("", &items, &SkimMatcherV2::default())),
            ["zebra", "apple", "mango"]
        );
    }

    #[test]
    fn fuzzy_order_is_score_then_length_then_alphabetical() {
        let cli_items = items(&["cli-extension-dep-graph", "cli-tools", "cli", "cli-abc"]);
        assert_eq!(
            names(&fuzzy_filter("cli", &cli_items, &SkimMatcherV2::default())),
            ["cli", "cli-abc", "cli-tools", "cli-extension-dep-graph"]
        );
        let foo_items = items(&["bfoo", "afoo", "cfoo"]);
        assert_eq!(
            names(&fuzzy_filter("foo", &foo_items, &SkimMatcherV2::default())),
            ["afoo", "bfoo", "cfoo"]
        );
    }

    #[test]
    fn fuzzy_search_matches_collision_disambiguator() {
        let items = vec![FilterItem {
            key: FilterKey::Repo("/repo".into()),
            text: "demo (…/customer-one)".into(),
        }];
        assert_eq!(
            names(&fuzzy_filter(
                "customer-one",
                &items,
                &SkimMatcherV2::default()
            )),
            ["repo"]
        );
    }

    #[test]
    fn no_matches_returns_an_empty_result() {
        assert!(
            fuzzy_filter(
                "zzzzz",
                &items(&["alpha", "beta"]),
                &SkimMatcherV2::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn branch_filter_uses_the_shared_score_length_and_name_ranking() {
        let branches = items(&["feature/very-long", "feature", "feature-short"]);
        assert_eq!(
            names(&fuzzy_filter(
                "feature",
                &branches,
                &SkimMatcherV2::default()
            )),
            ["feature", "feature-short", "feature/very-long"]
        );
    }

    fn state_with_repo() -> AppState {
        let mut state = AppState::new(None);
        state.repos.push(RepoEntry::new(Repo {
            name: "repo".into(),
            path: "/repo".into(),
            worktrees: Vec::new(),
        }));
        state.repo_list = crate::state::SearchableList::new(1);
        state
    }

    fn state_with_branch(has_worktree: bool) -> AppState {
        let mut state = state_with_repo();
        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        state.branches = vec![BranchEntry {
            name: "feature".into(),
            worktree_path: has_worktree.then(|| PathBuf::from("/repo-feature")),
            is_current: false,
            is_default: false,
            remote: None,
            open_workspace_id: None,
        }];
        state.branch_list = SearchableList::new(1);
        state
    }

    fn workspace() -> WorkspaceInfo {
        WorkspaceInfo {
            workspace_id: "w_1".into(),
            number: 1,
            label: "repo".into(),
            focused: true,
            pane_count: 1,
            tab_count: 1,
            active_tab_id: "w_1:1".into(),
            agent_status: AgentStatus::Idle,
            tokens: HashMap::new(),
            worktree: None,
        }
    }

    fn worktree() -> WorktreeInfo {
        WorktreeInfo {
            path: "/repo-feature".into(),
            branch: Some("feature".into()),
            is_bare: false,
            is_detached: false,
            is_prunable: false,
            is_linked_worktree: true,
            open_workspace_id: Some("w_1".into()),
            label: "repo feature".into(),
        }
    }

    fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
        let (tx, rx) = mpsc::channel();
        (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
    }

    fn git_provider() -> Arc<dyn GitProvider> {
        Arc::new(MockGitProvider::default())
    }

    #[test]
    fn opening_transitions_to_loading_and_dispatches_through_mock_provider() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_open_results
            .lock()
            .unwrap()
            .push_back(Err(HerdrError::WorktreeOpenFailed("boom".into())));
        let provider: Arc<dyn HerdrProvider> = mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_repo();

        begin_open_selected(&mut state, Some(&provider), &sender);

        assert_eq!(
            state.mode,
            Mode::Loading {
                message: "Opening repo…".into(),
                branch: None,
            }
        );
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            &event,
            AppEvent::RepoOpenFailed(message) if message.contains("boom")
        ));
        process_app_event(event, &mut state, &mut TickChanges::default());
        assert_eq!(state.mode, Mode::RepoSelect);
        assert!(state.toasts.front().unwrap().message.contains("boom"));
        assert_eq!(mock.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn opening_without_herdr_keeps_picker_usable_and_shows_error() {
        let (sender, _rx) = sender();
        let mut state = state_with_repo();
        begin_open_selected(&mut state, None, &sender);
        assert_eq!(state.mode, Mode::RepoSelect);
        assert_eq!(
            state.toasts.front().unwrap().message,
            "not running inside herdr"
        );
    }

    #[test]
    fn branch_view_transition_and_back_preserve_repo_filter_and_selection() {
        let mut state = state_with_repo();
        state.repo_list.input.text = "repo".into();
        state.repo_list.input.cursor = 4;
        state.repo_list.scroll_offset = 3;
        state.selection_touched = true;
        let git = Arc::new(MockGitProvider {
            branches: vec!["main".into()],
            worktrees: vec![Worktree {
                path: "/repo".into(),
                branch: Some("main".into()),
                is_main: true,
            }],
            ..MockGitProvider::default()
        }) as Arc<dyn GitProvider>;
        let (sender, _rx) = sender();

        begin_branch_select(&mut state, &git, None, &sender);
        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert_eq!(state.repo_list.input.text, "repo");
        assert_eq!(state.repo_list.selected, Some(0));
        assert_eq!(state.repo_list.scroll_offset, 3);

        let filter_worker = FilterWorker::spawn(sender.clone());
        process_action(
            Action::BackToRepos,
            &mut state,
            &git,
            None,
            &sender,
            &filter_worker,
        );
        assert_eq!(state.mode, Mode::RepoSelect);
        assert_eq!(state.repo_list.input.text, "repo");
        assert_eq!(state.repo_list.selected, Some(0));
        assert_eq!(state.repo_list.scroll_offset, 3);
    }

    #[test]
    fn existing_checkout_routes_to_open_and_success_exits() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_open_results
            .lock()
            .unwrap()
            .push_back(Ok(WorktreeOpenResponse {
                workspace: workspace(),
                worktree: worktree(),
                already_open: false,
            }));
        let provider: Arc<dyn HerdrProvider> = mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(true);

        begin_open_selected_branch(&mut state, &git_provider(), Some(&provider), &sender);
        assert!(matches!(
            &state.mode,
            Mode::Loading { message, branch: Some(_) } if message == "Opening feature…"
        ));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut changes = TickChanges::default();
        process_app_event(event, &mut state, &mut changes);
        assert!(changes.workspace_opened);
        assert_eq!(
            *mock.calls.lock().unwrap(),
            [HerdrCall::WorktreeOpen {
                cwd: "/repo".into(),
                target: crate::herdr::WorktreeOpenTarget::Branch("feature".into()),
                focus: true,
            }]
        );
    }

    #[test]
    fn missing_checkout_routes_to_create_without_base_or_path_and_success_exits() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_create_results
            .lock()
            .unwrap()
            .push_back(Ok(WorktreeCreateResponse {
                workspace: workspace(),
                worktree: worktree(),
            }));
        let provider: Arc<dyn HerdrProvider> = mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);

        begin_open_selected_branch(&mut state, &git_provider(), Some(&provider), &sender);
        assert!(matches!(
            &state.mode,
            Mode::Loading { message, branch: Some(_) }
                if message == "Creating worktree for feature…"
        ));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut changes = TickChanges::default();
        process_app_event(event, &mut state, &mut changes);
        assert!(changes.workspace_opened);
        let calls = mock.calls.lock().unwrap();
        let HerdrCall::WorktreeCreate(request) = &calls[0] else {
            panic!("expected worktree create")
        };
        assert_eq!(request.cwd, Path::new("/repo"));
        assert_eq!(request.branch, "feature");
        assert!(request.base.is_none());
        assert!(request.path.is_none());
        assert!(request.focus);
    }

    #[test]
    fn open_failure_returns_to_branch_view() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_open_results
            .lock()
            .unwrap()
            .push_back(Err(HerdrError::WorktreeOpenFailed("boom".into())));
        let provider: Arc<dyn HerdrProvider> = mock;
        let (sender, rx) = sender();
        let mut state = state_with_branch(true);
        begin_open_selected_branch(&mut state, &git_provider(), Some(&provider), &sender);
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &mut TickChanges::default());
        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(state.toasts.front().unwrap().message.contains("boom"));
    }

    #[test]
    fn create_in_progress_failure_is_friendly_and_returns_to_branch_view() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_create_results.lock().unwrap().push_back(Err(
            HerdrError::WorktreeOperationInProgress("raw details".into()),
        ));
        let provider: Arc<dyn HerdrProvider> = mock;
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        begin_open_selected_branch(&mut state, &git_provider(), Some(&provider), &sender);
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &mut TickChanges::default());
        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        let message = &state.toasts.front().unwrap().message;
        assert!(message.contains("already in progress"));
        assert!(!message.contains("worktree_operation_in_progress"));
    }

    #[test]
    fn remote_branch_race_falls_through_to_worktree_create_with_entry_remote() {
        let git_mock = Arc::new(MockGitProvider::default());
        git_mock
            .tracking_already_exists
            .store(true, Ordering::Release);
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock
            .worktree_create_results
            .lock()
            .unwrap()
            .push_back(Ok(WorktreeCreateResponse {
                workspace: workspace(),
                worktree: worktree(),
            }));
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        state.branches[0].remote = Some("upstream".into());

        begin_open_selected_branch(&mut state, &git, Some(&herdr), &sender);

        assert!(matches!(
            &state.mode,
            Mode::Loading { message, branch: Some(_) }
                if message == "Checking out remote branch feature…"
        ));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut changes = TickChanges::default();
        process_app_event(event, &mut state, &mut changes);
        assert!(changes.workspace_opened);
        assert_eq!(
            *git_mock.tracking_calls.lock().unwrap(),
            [(PathBuf::from("/repo"), "feature".into(), "upstream".into())]
        );
        let calls = herdr_mock.calls.lock().unwrap();
        let HerdrCall::WorktreeCreate(request) = &calls[0] else {
            panic!("expected worktree create")
        };
        assert_eq!(request.branch, "feature");
        assert!(request.base.is_none());
        assert!(request.path.is_none());
        assert!(request.focus);
    }

    #[test]
    fn remote_tracking_failure_returns_to_branch_view_without_creating_worktree() {
        let git_mock = Arc::new(MockGitProvider::default());
        *git_mock.failure.lock().unwrap() = Some("remote ref is missing".into());
        let git: Arc<dyn GitProvider> = git_mock;
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        state.branches[0].remote = Some("upstream".into());

        begin_open_selected_branch(&mut state, &git, Some(&herdr), &sender);
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &mut TickChanges::default());

        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(
            state
                .toasts
                .front()
                .unwrap()
                .message
                .contains("remote ref is missing")
        );
        assert!(herdr_mock.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn remote_merge_reapplies_filter_and_preserves_selection() {
        let (sender, rx) = sender();
        let worker = FilterWorker::spawn(sender);
        let mut state = state_with_branch(false);
        state.merge_remote_branches(
            "upstream".into(),
            BranchEntry::build_remote("upstream", &["z-feature".into()], &["feature".into()]),
        );
        state.branch_list.input.text = "feature".into();
        state.branch_list.input.cursor = 7;
        state.branch_list.filtered = vec![(0, 0), (1, 0)];
        state.branch_list.selected = Some(1);
        state.fetching_remote_repo = Some("/repo".into());
        let mut changes = TickChanges::default();

        process_app_event(
            AppEvent::GitFetchCompleted {
                remote: Some("origin".into()),
                branches: BranchEntry::build_remote(
                    "origin",
                    &["a-feature".into()],
                    &["feature".into()],
                ),
                repo_path: "/repo".into(),
                error: None,
                is_final: false,
            },
            &mut state,
            &mut changes,
        );
        assert!(changes.branches_changed);
        assert_eq!(
            state.selected_branch().unwrap().name,
            "z-feature",
            "selection must remain safe while the async filter catches up"
        );
        queue_branch_filter(&mut state, &worker, changes.pinned_branch_selection.take());
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &mut TickChanges::default());

        assert_eq!(state.branch_list.input.text, "feature");
        assert_eq!(state.branch_list.filtered.len(), 3);
        assert_eq!(state.selected_branch().unwrap().name, "z-feature");
    }

    #[test]
    fn stale_remote_and_fetch_events_are_dropped() {
        let mut state = state_with_branch(false);
        state.fetching_remote_repo = Some("/repo".into());
        let mut changes = TickChanges::default();
        let remote_branch = BranchEntry::build_remote("origin", &["remote".into()], &[]);

        process_app_event(
            AppEvent::RemoteBranchesLoaded {
                repo_path: "/other".into(),
                remote: "origin".into(),
                branches: remote_branch.clone(),
            },
            &mut state,
            &mut changes,
        );
        process_app_event(
            AppEvent::GitFetchCompleted {
                remote: Some("origin".into()),
                branches: remote_branch,
                repo_path: "/other".into(),
                error: Some("boom".into()),
                is_final: true,
            },
            &mut state,
            &mut changes,
        );

        assert_eq!(state.branches.len(), 1);
        assert_eq!(
            state.fetching_remote_repo.as_deref(),
            Some(Path::new("/repo"))
        );
        assert!(state.toasts.is_empty());
        assert!(!changes.branches_changed);
    }

    #[test]
    fn fetch_failure_toasts_are_deduplicated_per_remote() {
        let mut state = state_with_branch(false);
        state.fetching_remote_repo = Some("/repo".into());
        for message in ["first failure", "second failure"] {
            process_app_event(
                AppEvent::GitFetchCompleted {
                    remote: Some("origin".into()),
                    branches: Vec::new(),
                    repo_path: "/repo".into(),
                    error: Some(message.into()),
                    is_final: false,
                },
                &mut state,
                &mut TickChanges::default(),
            );
        }

        assert_eq!(state.toasts.len(), 1);
        assert!(state.toasts[0].message.contains("first failure"));
    }

    #[test]
    fn final_fetch_for_current_repo_clears_indicator() {
        let mut state = state_with_branch(false);
        state.fetching_remote_repo = Some("/repo".into());

        process_app_event(
            AppEvent::GitFetchCompleted {
                remote: None,
                branches: Vec::new(),
                repo_path: "/repo".into(),
                error: None,
                is_final: true,
            },
            &mut state,
            &mut TickChanges::default(),
        );

        assert!(state.fetching_remote_repo.is_none());
    }

    #[test]
    fn branch_git_failure_returns_to_repo_but_indicator_failure_does_not() {
        let mut state = state_with_branch(false);
        let mut changes = TickChanges::default();
        process_app_event(
            AppEvent::OpenWorktreesFailed {
                repo_path: "/repo".into(),
                message: "indicator unavailable".into(),
            },
            &mut state,
            &mut changes,
        );
        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert_eq!(state.toasts.len(), 1);

        process_app_event(
            AppEvent::BranchLoadFailed {
                repo_path: "/repo".into(),
                message: "git unavailable".into(),
            },
            &mut state,
            &mut changes,
        );
        assert_eq!(state.mode, Mode::RepoSelect);
        assert_eq!(state.toasts.len(), 2);
    }
}
