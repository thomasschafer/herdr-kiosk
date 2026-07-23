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
    config::keys::{BindingMode, Command, KeysConfig},
    event::{AppEvent, FilterKey, FilterTarget, WorktreeRemovalOutcome},
    git::{GitProvider, Repo},
    herdr::HerdrProvider,
    keyboard::Action,
    pending_delete::PendingWorktreeDelete,
    spawn::{
        EventSender, FetchDeduplicator, spawn_create_new_branch, spawn_git_fetch, spawn_open_repo,
        spawn_remote_branch_loading, spawn_repo_discovery, spawn_validate_branch_name,
        spawn_workspace_list, spawn_worktree_removal,
    },
    state::{
        AppState, BaseBranchSelection, BranchContext, BranchId, DeleteWorktreeTarget, Mode,
        NewBranchRoute, OpenWorktreeLoadState, RepoEntry, SearchableList, ToastKind,
        collision_disambiguators,
    },
    theme::Theme,
};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(40);
const MAX_EVENTS_PER_TICK: usize = 256;

#[derive(Debug)]
struct RedrawState {
    dirty: bool,
}

impl RedrawState {
    fn new() -> Self {
        Self { dirty: true }
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn take(&mut self, animation_active: bool) -> bool {
        let redraw = self.dirty || animation_active;
        self.dirty = false;
        redraw
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Quit,
    Opened,
}

#[derive(Clone)]
pub(crate) struct FilterItem {
    pub(crate) key: FilterKey,
    pub(crate) text: String,
}

pub(crate) struct FilterRequest {
    pub(crate) target: FilterTarget,
    pub(crate) generation: u64,
    pub(crate) query: String,
    pub(crate) items: Vec<FilterItem>,
    pub(crate) selected: Option<FilterKey>,
}

pub(crate) struct FilterWorker {
    pending: Arc<(Mutex<Option<FilterRequest>>, Condvar)>,
    cancel: Arc<AtomicBool>,
}

impl FilterWorker {
    pub(crate) fn spawn(sender: EventSender) -> Self {
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

    pub(crate) fn request(&self, request: FilterRequest) {
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
    keys: &KeysConfig,
) -> Result<RunOutcome> {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let sender = EventSender::new(tx, Arc::clone(&cancel));
    let filter_worker = FilterWorker::spawn(sender.clone());
    let fetch_deduplicator = FetchDeduplicator::default();
    let spinner_start = Instant::now();
    let mut redraw = RedrawState::new();

    spawn_repo_discovery(git, &sender, search_dirs);
    if let Some(provider) = herdr {
        spawn_workspace_list(provider, &sender);
    }

    let outcome = loop {
        if redraw.take(animation_active(state)) {
            terminal.draw(|frame| draw(frame, state, theme, keys, spinner_start))?;
        }

        let mut changes = TickChanges::default();
        let mut event_received = false;
        for app_event in rx.try_iter().take(MAX_EVENTS_PER_TICK) {
            event_received = true;
            process_app_event(app_event, state, &mut changes);
        }
        if event_received {
            redraw.mark_dirty();
        }

        if let Some(outcome) = apply_exit_effects(&mut changes, herdr) {
            break outcome;
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
            crate::screens::branch::queue_filter(
                state,
                &filter_worker,
                changes.pinned_branch_selection.take(),
            );
        }
        if let Some((repo_path, generation, local_names)) = changes.start_remote_loading.take() {
            spawn_remote_branch_loading(
                git,
                &sender,
                repo_path.clone(),
                local_names.clone(),
                generation,
            );
            spawn_git_fetch(
                git,
                &sender,
                &fetch_deduplicator,
                repo_path,
                local_names,
                generation,
            );
        }
        if let Some(repo) = changes.refresh_branch.take() {
            crate::screens::branch::refresh(state, git, herdr, &sender, repo);
        }
        if changes.resume_pending_deletes {
            resume_pending_deletes(state, git, herdr, &sender);
        }

        if event::poll(EVENT_POLL_INTERVAL)? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let Some(action) = crate::keymap::resolve_action(key, state, keys) else {
                        continue;
                    };
                    redraw.mark_dirty();
                    if let Some(outcome) =
                        process_action(action, state, git, herdr, &sender, &filter_worker, keys)
                    {
                        break outcome;
                    }
                }
                Event::Resize(_, _) => redraw.mark_dirty(),
                _ => {}
            }
        }
    };

    cancel.store(true, Ordering::Relaxed);
    drop(filter_worker);
    Ok(outcome)
}

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct TickChanges {
    pub(crate) repos_changed: bool,
    pub(crate) branches_changed: bool,
    pub(crate) collision_pass: bool,
    pub(crate) workspace_opened: bool,
    pub(crate) open_warning: Option<String>,
    pub(crate) pinned_branch_selection: Option<BranchId>,
    pub(crate) start_remote_loading: Option<(PathBuf, u64, Vec<String>)>,
    pub(crate) refresh_branch: Option<Repo>,
    pub(crate) resume_pending_deletes: bool,
}

#[allow(clippy::too_many_lines)]
pub(crate) fn process_app_event(event: AppEvent, state: &mut AppState, changes: &mut TickChanges) {
    let Some(event) = crate::screens::branch::handle_event(event, state, changes) else {
        return;
    };
    match event {
        AppEvent::ReposFound { repo } => changes.repos_changed |= add_repo(state, repo),
        AppEvent::ScanComplete => {
            state.loading_repos = false;
            changes.collision_pass = true;
        }
        AppEvent::ScanWarning(_warning) => {
            state.push_scan_warning();
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
            FilterTarget::Bases if generation == state.base_filter_generation => {
                apply_base_filter_result(state, &matches, selected.as_ref());
            }
            FilterTarget::Help if generation == state.help_filter_generation => {
                apply_help_filter_result(state, &matches, selected.as_ref());
            }
            FilterTarget::Repos
            | FilterTarget::Branches
            | FilterTarget::Bases
            | FilterTarget::Help => {}
        },
        AppEvent::BranchNameValidated {
            repo_path,
            branch_name,
            valid,
            error,
        } if matches!(
            &state.mode,
            Mode::ValidatingNewBranch { context, name }
                if context.repo_path == repo_path && name == &branch_name
        ) =>
        {
            let context = state.branch_context().cloned().unwrap();
            if let Some(error) = error {
                state.mode = Mode::BranchSelect(context);
                state.push_toast(ToastKind::Error, error);
            } else if !valid {
                state.mode = Mode::BranchSelect(context);
                state.push_toast(
                    ToastKind::Error,
                    format!("Invalid branch name: {branch_name}"),
                );
            } else {
                let local = state
                    .branch_view
                    .entries
                    .iter()
                    .filter(|branch| branch.remote.is_none())
                    .collect::<Vec<_>>();
                if local.is_empty() {
                    state.mode = Mode::BranchSelect(context);
                    state.push_toast(ToastKind::Error, "No local branches to use as base");
                } else {
                    let bases = local
                        .iter()
                        .map(|branch| branch.name.clone())
                        .collect::<Vec<_>>();
                    let mut list = SearchableList::new(bases.len());
                    list.selected = local
                        .iter()
                        .position(|branch| branch.is_default)
                        .or(Some(0));
                    state.base_filter_generation = state.base_filter_generation.wrapping_add(1);
                    state.mode = Mode::SelectBaseBranch {
                        context,
                        flow: BaseBranchSelection {
                            new_name: branch_name,
                            bases,
                            list,
                        },
                    };
                }
            }
        }
        AppEvent::RepoOpened { warning } => {
            changes.open_warning = warning;
            changes.workspace_opened = true;
        }
        AppEvent::RepoOpenFailed(message)
            if matches!(state.mode, Mode::Loading { branch: None, .. }) =>
        {
            state.mode = Mode::RepoSelect;
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::WorktreeRemovalFinished {
            repo_path,
            branch_name,
            worktree_path,
            outcome,
        } if delete_event_matches(state, &repo_path, &branch_name, &worktree_path)
            || state.in_flight_worktree_removals.contains(&worktree_path) =>
        {
            let ui_matches = delete_event_matches(state, &repo_path, &branch_name, &worktree_path)
                || crate::screens::branch::matches_repo(state, &repo_path);
            state.in_flight_worktree_removals.remove(&worktree_path);
            match outcome {
                WorktreeRemovalOutcome::DirtyRequiresForce => {
                    if let Mode::ConfirmWorktreeDelete { target, .. } = &mut state.mode {
                        if target.worktree_path == worktree_path {
                            target.force = true;
                            target.in_progress = false;
                        }
                    } else if ui_matches && let Mode::BranchSelect(context) = &state.mode {
                        let context = context.clone();
                        let open_workspace_id = state
                            .branch_view
                            .entries
                            .iter()
                            .find(|branch| branch.name == branch_name)
                            .and_then(|branch| branch.open_workspace_id.clone());
                        state.mode = Mode::ConfirmWorktreeDelete {
                            context,
                            target: DeleteWorktreeTarget {
                                branch_name: branch_name.clone(),
                                worktree_path: worktree_path.clone(),
                                open_workspace_id,
                                force: true,
                                in_progress: false,
                            },
                        };
                    }
                }
                WorktreeRemovalOutcome::Removed { warning } => {
                    state.clear_pending_worktree_delete(&worktree_path);
                    persist_pending_deletes(state);
                    if let Some(entry) = state
                        .repos
                        .iter_mut()
                        .find(|entry| entry.repo.path == repo_path)
                    {
                        entry
                            .repo
                            .worktrees
                            .retain(|worktree| worktree.path != worktree_path);
                    }
                    if !ui_matches {
                        if let Some(warning) = warning {
                            state.push_toast(ToastKind::Warning, warning);
                        }
                        return;
                    }
                    let context = BranchContext {
                        repo_path: repo_path.clone(),
                        repo_name: state
                            .repos
                            .iter()
                            .find(|entry| entry.repo.path == repo_path)
                            .map_or_else(|| "repository".into(), |entry| entry.repo.name.clone()),
                    };
                    state.mode = Mode::BranchSelect(context);
                    state.branch_view.loading = true;
                    state.branch_view.reset_remotes();
                    state.branch_view.open_worktrees.clear();
                    state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Unknown;
                    if let Some(branch) = state
                        .branch_view
                        .entries
                        .iter_mut()
                        .find(|branch| branch.name == branch_name)
                    {
                        branch.worktree_path = None;
                        branch.open_workspace_id = None;
                    }
                    changes.refresh_branch = state
                        .repos
                        .iter()
                        .find(|entry| entry.repo.path == repo_path)
                        .map(|entry| entry.repo.clone());
                    if let Some(warning) = warning {
                        state.push_toast(ToastKind::Warning, warning);
                    }
                }
                WorktreeRemovalOutcome::Failed(error) => {
                    state.clear_pending_worktree_delete(&worktree_path);
                    persist_pending_deletes(state);
                    if !ui_matches {
                        state.push_toast(
                            ToastKind::Error,
                            format!("Failed to remove checkout for {branch_name}: {error}"),
                        );
                        return;
                    }
                    let context = BranchContext {
                        repo_path,
                        repo_name: state.branch_context().map_or_else(
                            || "repository".into(),
                            |context| context.repo_name.clone(),
                        ),
                    };
                    state.mode = Mode::BranchSelect(context);
                    state.push_toast(
                        ToastKind::Error,
                        format!("Failed to remove checkout for {branch_name}: {error}"),
                    );
                }
            }
        }
        AppEvent::OpenWorkspacesFailed(message) => {
            state.push_toast(ToastKind::Warning, message);
        }
        _ => {}
    }
}

pub(crate) fn apply_exit_effects(
    changes: &mut TickChanges,
    herdr: Option<&Arc<dyn HerdrProvider>>,
) -> Option<RunOutcome> {
    if !changes.workspace_opened {
        return None;
    }
    if let Some(warning) = changes.open_warning.take() {
        if let Some(provider) = herdr {
            if let Err(error) = provider.notification_show("herdr-kiosk", &warning) {
                eprintln!("herdr-kiosk: {warning} (notification failed: {error})");
            }
        } else {
            eprintln!("herdr-kiosk: {warning}");
        }
    }
    Some(RunOutcome::Opened)
}

pub(crate) fn process_action(
    action: Action,
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
    filter_worker: &FilterWorker,
    keys: &KeysConfig,
) -> Option<RunOutcome> {
    match action {
        Action::Quit => return Some(RunOutcome::Quit),
        Action::MoveSelection(delta) => {
            if let Some(overlay) = &mut state.help_overlay {
                overlay.list.move_selection(delta);
            } else if matches!(state.mode, Mode::RepoSelect) {
                state.selection_touched = true;
                state.repo_list.move_selection(delta);
            } else if matches!(state.mode, Mode::BranchSelect(_)) {
                crate::screens::branch::move_selection(state, delta);
            } else if let Mode::SelectBaseBranch { flow, .. } = &mut state.mode {
                flow.list.move_selection(delta);
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
        Action::OpenBranches => crate::screens::branch::enter(state, git, herdr, sender),
        Action::OpenBranch => crate::screens::branch::open_selected(state, git, herdr, sender),
        Action::StartNewBranch => begin_start_new_branch(state, git, herdr, sender),
        Action::CreateNewBranch => begin_create_new_branch(state, herdr, sender),
        Action::DeleteWorktree => begin_delete_worktree(state),
        Action::ConfirmDeleteWorktree => {
            confirm_delete_worktree(state, git, herdr, sender);
        }
        Action::CancelOverlay => cancel_overlay(state),
        Action::BackToRepos => {
            crate::screens::branch::leave(state);
            queue_repo_filter(state, filter_worker, true);
        }
        Action::DismissToast => {
            state.dismiss_toast();
        }
        Action::ShowHelp => {
            let binding_mode = KeysConfig::mode_for(&state.mode);
            state.help_overlay = Some(components::help::overlay(keys, binding_mode));
            state.help_filter_generation = state.help_filter_generation.wrapping_add(1);
        }
        Action::CloseHelp => state.help_overlay = None,
        Action::Noop => {}
    }
    None
}

fn active_list_mut(state: &mut AppState) -> &mut SearchableList {
    if let Some(overlay) = &mut state.help_overlay {
        return &mut overlay.list;
    }
    match &mut state.mode {
        Mode::BranchSelect(_) => &mut state.branch_view.list,
        Mode::SelectBaseBranch { flow, .. } => &mut flow.list,
        Mode::RepoSelect
        | Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmWorktreeDelete { .. } => &mut state.repo_list,
    }
}

fn edit_active_list(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    if let Some(overlay) = &mut state.help_overlay {
        edit(&mut overlay.list);
        queue_help_filter(state, worker, None);
        return;
    }
    match state.mode {
        Mode::RepoSelect => {
            state.selection_touched = true;
            edit(&mut state.repo_list);
            queue_repo_filter(state, worker, false);
        }
        Mode::BranchSelect(_) => {
            crate::screens::branch::edit(state, worker, edit);
        }
        Mode::SelectBaseBranch { .. } => {
            if let Mode::SelectBaseBranch { flow, .. } = &mut state.mode {
                edit(&mut flow.list);
            }
            queue_base_filter(state, worker, None);
        }
        Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmWorktreeDelete { .. } => {}
    }
}

fn delete_event_matches(
    state: &AppState,
    repo_path: &Path,
    branch_name: &str,
    worktree_path: &Path,
) -> bool {
    matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete { context, target }
            if context.repo_path == repo_path
                && target.branch_name == branch_name
                && target.worktree_path == worktree_path
                && target.in_progress
    )
}

pub(crate) fn persist_pending_deletes(state: &mut AppState) {
    if let Err(error) = save_pending_deletes(&state.pending_worktree_deletes) {
        state.push_toast(
            ToastKind::Error,
            format!("Could not persist pending deletions: {error:#}"),
        );
    }
}

#[cfg(not(test))]
fn save_pending_deletes(entries: &[PendingWorktreeDelete]) -> anyhow::Result<()> {
    crate::pending_delete::save_pending_worktree_deletes(entries)
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
fn save_pending_deletes(_entries: &[PendingWorktreeDelete]) -> anyhow::Result<()> {
    Ok(())
}

fn resume_pending_deletes(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    if state.branch_view.loading {
        return;
    }
    let Mode::BranchSelect(context) = &state.mode else {
        return;
    };
    let repo_path = context.repo_path.clone();
    if !matches!(
        &state.branch_view.open_worktree_load_state,
        OpenWorktreeLoadState::Loaded {
            repo_path: loaded_repo,
            generation,
        } if loaded_repo == &repo_path && *generation == state.branch_view.generation
    ) {
        return;
    }
    let pending = state
        .pending_worktree_deletes
        .iter()
        .filter(|pending| pending.repo_path == repo_path)
        .cloned()
        .collect::<Vec<_>>();
    for pending in pending {
        if !state
            .in_flight_worktree_removals
            .insert(pending.worktree_path.clone())
        {
            continue;
        }
        spawn_worktree_removal(
            git,
            herdr,
            sender,
            repo_path.clone(),
            pending.branch_name,
            pending.worktree_path,
            pending.force,
        );
    }
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
        let repo_path = canonical_or_original(&entry.repo.path);
        entry.is_open = state
            .open_repo_roots
            .iter()
            .any(|open_path| crate::path::equivalent(open_path, &repo_path));
    }
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

fn queue_base_filter(state: &mut AppState, worker: &FilterWorker, selected_name: Option<String>) {
    state.base_filter_generation = state.base_filter_generation.wrapping_add(1);
    let Mode::SelectBaseBranch { flow, .. } = &mut state.mode else {
        return;
    };
    if flow.list.input.text.is_empty() {
        flow.list.filtered = (0..flow.bases.len()).map(|index| (index, 0)).collect();
        flow.list.selected = if flow.bases.is_empty() {
            None
        } else {
            selected_name
                .as_deref()
                .and_then(|name| flow.bases.iter().position(|base| base == name))
                .or(Some(0))
        };
        flow.list.scroll_offset = 0;
        return;
    }
    worker.request(FilterRequest {
        target: FilterTarget::Bases,
        generation: state.base_filter_generation,
        query: flow.list.input.text.clone(),
        items: flow
            .bases
            .iter()
            .map(|base| FilterItem {
                key: FilterKey::Base(base.clone()),
                text: base.clone(),
            })
            .collect(),
        selected: selected_name.map(FilterKey::Base),
    });
}

fn queue_help_filter(state: &mut AppState, worker: &FilterWorker, selected_index: Option<usize>) {
    state.help_filter_generation = state.help_filter_generation.wrapping_add(1);
    let Some(overlay) = &mut state.help_overlay else {
        return;
    };
    if overlay.list.input.text.is_empty() {
        overlay.list.filtered = (0..overlay.rows.len()).map(|index| (index, 0)).collect();
        overlay.list.selected = (!overlay.rows.is_empty()).then_some(0);
        overlay.list.scroll_offset = 0;
        return;
    }
    worker.request(FilterRequest {
        target: FilterTarget::Help,
        generation: state.help_filter_generation,
        query: overlay.list.input.text.clone(),
        items: overlay
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| FilterItem {
                key: FilterKey::Help(index),
                text: row.search_text(),
            })
            .collect(),
        selected: selected_index.map(FilterKey::Help),
    });
}

fn apply_repo_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let current = state.selected_repo().map(|entry| entry.repo.path.clone());
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
            FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
        })
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Repo(path) => Some(path.clone()),
        FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => None,
    });
    state.repo_list.selected = current
        .or(requested)
        .as_ref()
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

fn apply_base_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let Mode::SelectBaseBranch { flow, .. } = &mut state.mode else {
        return;
    };
    let current = flow
        .list
        .selected
        .and_then(|selected| flow.list.filtered.get(selected))
        .and_then(|(index, _)| flow.bases.get(*index))
        .cloned();
    let indices: HashMap<_, _> = flow
        .bases
        .iter()
        .enumerate()
        .map(|(index, base)| (base.as_str(), index))
        .collect();
    flow.list.filtered = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Base(name) => indices.get(name.as_str()).map(|index| (*index, *score)),
            FilterKey::Repo(_) | FilterKey::Branch(_) | FilterKey::Help(_) => None,
        })
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Base(name) => Some(name.clone()),
        FilterKey::Repo(_) | FilterKey::Branch(_) | FilterKey::Help(_) => None,
    });
    flow.list.selected = current
        .or(requested)
        .as_ref()
        .and_then(|name| {
            flow.list
                .filtered
                .iter()
                .position(|(index, _)| flow.bases[*index] == *name)
        })
        .or_else(|| (!flow.list.filtered.is_empty()).then_some(0));
    flow.list.scroll_offset = 0;
}

fn apply_help_filter_result(
    state: &mut AppState,
    matches: &[(FilterKey, i64)],
    selected: Option<&FilterKey>,
) {
    let Some(overlay) = &mut state.help_overlay else {
        return;
    };
    let current = overlay
        .list
        .selected
        .and_then(|selected| overlay.list.filtered.get(selected))
        .map(|(index, _)| *index);
    let scores = matches
        .iter()
        .filter_map(|(key, score)| match key {
            FilterKey::Help(index) if *index < overlay.rows.len() => Some((*index, *score)),
            FilterKey::Repo(_) | FilterKey::Branch(_) | FilterKey::Base(_) | FilterKey::Help(_) => {
                None
            }
        })
        .collect::<HashMap<_, _>>();
    overlay.list.filtered = (0..overlay.rows.len())
        .filter_map(|index| scores.get(&index).map(|score| (index, *score)))
        .collect();
    let requested = selected.and_then(|key| match key {
        FilterKey::Help(index) => Some(*index),
        FilterKey::Repo(_) | FilterKey::Branch(_) | FilterKey::Base(_) => None,
    });
    overlay.list.selected = current
        .or(requested)
        .and_then(|selected| {
            overlay
                .list
                .filtered
                .iter()
                .position(|(index, _)| *index == selected)
        })
        .or_else(|| (!overlay.list.filtered.is_empty()).then_some(0));
    overlay.list.scroll_offset = 0;
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
    spawn_open_repo(provider, sender, repo_path, state.on_open.clone());
}

fn begin_start_new_branch(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    match state.new_branch_route() {
        Err(message) => state.push_toast(ToastKind::Error, message),
        Ok(NewBranchRoute::Existing(branch)) => {
            crate::screens::branch::open(state, git, herdr, sender, &branch);
        }
        Ok(NewBranchRoute::Validate { context, name }) => {
            let repo_path = context.repo_path.clone();
            state.mode = Mode::ValidatingNewBranch {
                context,
                name: name.clone(),
            };
            spawn_validate_branch_name(git, sender, repo_path, name);
        }
    }
}

fn begin_create_new_branch(
    state: &mut AppState,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Mode::SelectBaseBranch { context, flow } = &state.mode else {
        return;
    };
    let Some(selected) = flow.list.selected else {
        return;
    };
    let Some((base_index, _)) = flow.list.filtered.get(selected) else {
        return;
    };
    let Some(base) = flow.bases.get(*base_index).cloned() else {
        return;
    };
    let context = context.clone();
    let branch_name = flow.new_name.clone();
    let Some(provider) = herdr else {
        state.mode = Mode::BranchSelect(context);
        state.push_toast(ToastKind::Error, "not running inside herdr");
        return;
    };
    state.mode = Mode::Loading {
        message: format!("Creating {branch_name} from {base}…"),
        branch: Some(context.clone()),
    };
    spawn_create_new_branch(
        provider,
        sender,
        context.repo_path,
        branch_name,
        base,
        state.on_open.clone(),
    );
}

fn begin_delete_worktree(state: &mut AppState) {
    let context = match &state.mode {
        Mode::BranchSelect(context) => context.clone(),
        _ => return,
    };
    match state.selected_worktree_for_delete() {
        Ok(target) => {
            state.mode = Mode::ConfirmWorktreeDelete { context, target };
        }
        Err(message) => state.push_toast(ToastKind::Error, message),
    }
}

fn confirm_delete_worktree(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Mode::ConfirmWorktreeDelete { context, target } = &state.mode else {
        return;
    };
    if target.in_progress {
        return;
    }
    let context = context.clone();
    let mut target_snapshot = target.clone();
    match &state.branch_view.open_worktree_load_state {
        OpenWorktreeLoadState::Loaded {
            repo_path,
            generation,
        } if repo_path == &context.repo_path && *generation == state.branch_view.generation => {}
        OpenWorktreeLoadState::Failed { .. } => {
            state.push_toast(
                ToastKind::Error,
                "Cannot delete checkout because open checkout state could not be loaded",
            );
            return;
        }
        OpenWorktreeLoadState::Unknown | OpenWorktreeLoadState::Loaded { .. } => {
            state.push_toast(
                ToastKind::Error,
                "Open checkout state is stale or still loading; deletion was not started",
            );
            return;
        }
    }
    let Some(current_branch) = state.branch_view.entries.iter().find(|branch| {
        branch.name == target_snapshot.branch_name
            && branch.worktree_path.as_ref() == Some(&target_snapshot.worktree_path)
    }) else {
        state.push_toast(
            ToastKind::Error,
            "Checkout state changed; cancel deletion and try again",
        );
        return;
    };
    target_snapshot
        .open_workspace_id
        .clone_from(&current_branch.open_workspace_id);
    if let Mode::ConfirmWorktreeDelete { target, .. } = &mut state.mode {
        target
            .open_workspace_id
            .clone_from(&target_snapshot.open_workspace_id);
    }
    let mut pending = PendingWorktreeDelete::new(
        context.repo_path.clone(),
        target_snapshot.branch_name.clone(),
        target_snapshot.worktree_path.clone(),
    );
    pending.force = target_snapshot.force;
    state.mark_pending_worktree_delete(pending);
    if let Err(error) = save_pending_deletes(&state.pending_worktree_deletes) {
        state.clear_pending_worktree_delete(&target_snapshot.worktree_path);
        state.push_toast(
            ToastKind::Error,
            format!("Could not persist pending deletion: {error:#}"),
        );
        return;
    }
    if let Mode::ConfirmWorktreeDelete { target, .. } = &mut state.mode {
        target.in_progress = true;
    }
    state
        .in_flight_worktree_removals
        .insert(target_snapshot.worktree_path.clone());
    spawn_worktree_removal(
        git,
        herdr,
        sender,
        context.repo_path,
        target_snapshot.branch_name,
        target_snapshot.worktree_path,
        target_snapshot.force,
    );
}

fn cancel_overlay(state: &mut AppState) {
    let mode = state.mode.clone();
    match mode {
        Mode::SelectBaseBranch { context, .. } => state.mode = Mode::BranchSelect(context),
        Mode::ConfirmWorktreeDelete { context, target } if !target.in_progress => {
            if target.force {
                state.clear_pending_worktree_delete(&target.worktree_path);
                persist_pending_deletes(state);
            }
            state.mode = Mode::BranchSelect(context);
        }
        _ => {}
    }
}

fn draw(
    frame: &mut Frame,
    state: &mut AppState,
    theme: &Theme,
    keys: &KeysConfig,
    spinner_start: Instant,
) {
    let loading_message = match &state.mode {
        Mode::Loading { message, .. } => Some(message.clone()),
        _ => None,
    };
    if let Some(message) = loading_message {
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
        let mut lines = vec![Line::from(Span::styled(
            format!("{spinner} {}", crate::display::sanitize(&message)),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))];
        if let Some(hint) = loading_hint(keys) {
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(theme.muted),
            )));
        }
        frame.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
        return;
    }

    let [main_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(frame.area());
    let mode = state.mode.clone();
    match &mode {
        Mode::RepoSelect => {
            components::repo_list::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::BranchSelect(_) => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::SelectBaseBranch { .. } | Mode::ValidatingNewBranch { .. } => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
            components::new_branch::draw(frame, main_area, state, theme, spinner_start);
        }
        Mode::ConfirmWorktreeDelete { target, .. } => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
            draw_delete_dialog(frame, main_area, target, theme, keys, spinner_start);
        }
        Mode::Loading { .. } => {
            unreachable!("loading mode returned above")
        }
    }
    let binding_mode = KeysConfig::mode_for(&state.mode);
    let footer = footer_spans(keys, binding_mode, &state.mode, theme);
    frame.render_widget(
        Paragraph::new(Line::from(footer)).alignment(Alignment::Center),
        footer_area,
    );
    components::error_toast::draw(frame, frame.area(), state, theme, keys);
    let toast_visible = !state.toasts.is_empty();
    if let Some(overlay) = &mut state.help_overlay {
        components::help::draw(frame, frame.area(), overlay, toast_visible, theme);
    }
}

fn animation_active(state: &AppState) -> bool {
    let branch_spinner =
        state.branch_view.loading || state.branch_view.fetching_remote_repo.is_some();
    match &state.mode {
        Mode::RepoSelect => state.loading_repos,
        Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. } => branch_spinner,
        Mode::ValidatingNewBranch { .. } | Mode::Loading { .. } => true,
        Mode::ConfirmWorktreeDelete { target, .. } => branch_spinner || target.in_progress,
    }
}

fn loading_hint(keys: &KeysConfig) -> Option<String> {
    keys.first_key(BindingMode::Modal, Command::Quit)
        .map(|key| format!("{key} to close (operation continues)"))
}

fn delete_dialog_hints(keys: &KeysConfig) -> (Option<String>, Option<String>) {
    (
        keys.first_key(BindingMode::Modal, Command::Open)
            .map(|key| key.to_string()),
        keys.first_key(BindingMode::Modal, Command::Back)
            .map(|key| key.to_string()),
    )
}

fn footer_spans<'a>(
    keys: &KeysConfig,
    binding_mode: BindingMode,
    mode: &Mode,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let mut hints = Vec::new();
    let mut add = |command, label: &'static str| {
        if let Some(key) = keys.first_key(binding_mode, command) {
            if !hints.is_empty() {
                hints.push(Span::raw("  "));
            }
            hints.push(Span::styled(
                key.to_string(),
                Style::default().fg(theme.hint),
            ));
            hints.push(Span::raw(format!(" {label}")));
        }
    };
    if matches!(
        mode,
        Mode::ValidatingNewBranch { .. } | Mode::Loading { .. }
    ) {
        add(Command::Quit, "quit");
        return hints;
    }
    if !matches!(binding_mode, BindingMode::Modal) {
        add(Command::MoveUp, "move");
    }
    add(
        Command::Open,
        if matches!(
            mode,
            Mode::SelectBaseBranch { .. } | Mode::ConfirmWorktreeDelete { .. }
        ) {
            "confirm"
        } else {
            "open"
        },
    );
    if matches!(mode, Mode::RepoSelect) {
        add(Command::BranchesView, "branches");
    }
    if matches!(mode, Mode::BranchSelect(_)) {
        add(Command::NewBranch, "new");
        add(Command::Delete, "delete");
    }
    if matches!(
        mode,
        Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. } | Mode::ConfirmWorktreeDelete { .. }
    ) {
        add(Command::Back, "back");
    } else {
        add(Command::Clear, "clear/quit");
    }
    add(Command::Help, "help");
    add(Command::Quit, "quit");
    hints
}

fn draw_delete_dialog(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    target: &DeleteWorktreeTarget,
    theme: &Theme,
    keys: &KeysConfig,
    spinner_start: Instant,
) {
    let mut lines = if target.force {
        vec![
            Line::from(Span::styled(
                "This checkout has uncommitted changes.",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::raw(format!(
                "Force-remove {}?",
                crate::display::sanitize(&crate::path::display(&target.worktree_path))
            )),
        ]
    } else {
        vec![Line::raw(format!(
            "Remove checkout {}?",
            crate::display::sanitize(&crate::path::display(&target.worktree_path))
        ))]
    };
    if target.open_workspace_id.is_some() {
        lines.push(Line::raw("Its herdr workspace will also be closed."));
    }
    lines.push(Line::raw(""));
    if target.in_progress {
        let spinner =
            components::repo_list::SPINNER_FOR_LOADING[(spinner_start.elapsed().as_millis() / 80)
                as usize
                % components::repo_list::SPINNER_FOR_LOADING.len()];
        lines.push(Line::from(Span::styled(
            format!("{spinner} Removing checkout…"),
            Style::default().fg(theme.secondary),
        )));
    } else {
        let (confirm, cancel) = delete_dialog_hints(keys);
        let mut hints = Vec::new();
        if let Some(confirm) = confirm {
            hints.push(Span::styled(confirm, Style::default().fg(theme.hint)));
            hints.push(Span::raw(" confirm"));
        }
        if let Some(cancel) = cancel {
            if !hints.is_empty() {
                hints.push(Span::raw(" / "));
            }
            hints.push(Span::styled(cancel, Style::default().fg(theme.hint)));
            hints.push(Span::raw(" cancel"));
        }
        if !hints.is_empty() {
            lines.push(Line::from(hints));
        }
    }
    components::dialog::Dialog::new(" Confirm delete ", lines, theme.secondary).render(frame, area);
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };

    use crate::{
        git::{Repo, Worktree, mock::MockGitProvider},
        herdr::{
            HerdrError, HerdrProvider, OpenedWorktree, WorktreeCreateResponse, WorktreeInfo,
            WorktreeListResponse, WorktreeRemoveResponse,
            mock::{HerdrCall, MockHerdrProvider},
        },
        state::{BranchEntry, BranchId},
    };
    use ratatui::{Terminal, backend::TestBackend};

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

    #[test]
    fn clean_idle_tick_does_not_redraw() {
        let mut redraw = RedrawState::new();

        assert!(redraw.take(false));
        assert!(!redraw.take(false));
        redraw.mark_dirty();
        assert!(redraw.take(false));
        assert!(!redraw.take(false));
    }

    #[test]
    fn active_animation_redraws_without_dirty_state() {
        let mut redraw = RedrawState::new();

        assert!(redraw.take(true));
        assert!(redraw.take(true));
        assert!(!redraw.take(false));
    }

    fn names(matches: &[(FilterKey, i64)]) -> Vec<String> {
        matches
            .iter()
            .map(|(key, _)| match key {
                FilterKey::Repo(path) => path.file_name().unwrap().to_string_lossy().into_owned(),
                FilterKey::Branch(id) => id.display_name(),
                FilterKey::Base(name) => name.clone(),
                FilterKey::Help(index) => index.to_string(),
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
    fn fuzzy_search_matches_remote_qualified_branch_display() {
        let id = BranchId::Remote {
            remote: "upstream".into(),
            name: "feature".into(),
        };
        let items = vec![FilterItem {
            key: FilterKey::Branch(id.clone()),
            text: id.display_name(),
        }];

        let matches = fuzzy_filter("upstream", &items, &SkimMatcherV2::default());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, FilterKey::Branch(id));
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

    #[test]
    fn help_fuzzy_filter_matches_key_command_and_description() {
        let overlay = components::help::overlay(&KeysConfig::default(), BindingMode::Branch);
        let items = overlay
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| FilterItem {
                key: FilterKey::Help(index),
                text: row.search_text(),
            })
            .collect::<Vec<_>>();
        let matcher = SkimMatcherV2::default();
        for query in ["ctrl+o", "new_branch", "Create a new branch"] {
            let filtered = fuzzy_filter(query, &items, &matcher);
            assert!(filtered.iter().any(|(key, _)| {
                matches!(key, FilterKey::Help(index) if overlay.rows[*index].command_name == "new_branch")
            }));
        }
    }

    #[test]
    fn modal_hints_follow_effective_remapped_bindings() {
        let keys = toml::from_str::<KeysConfig>(
            "[general]\n\"C-c\" = \"noop\"\n\"C-q\" = \"quit\"\n[modal]\nenter = \"noop\"\nesc = \"noop\"\n\"C-y\" = \"open\"\n\"C-g\" = \"back\"",
        )
        .unwrap();
        assert_eq!(
            loading_hint(&keys).as_deref(),
            Some("ctrl+q to close (operation continues)")
        );
        assert_eq!(
            delete_dialog_hints(&keys),
            (Some("ctrl+y".into()), Some("ctrl+g".into()))
        );
    }

    #[test]
    fn current_logical_selection_survives_current_generation_filter_results() {
        let mut repo_state = AppState::new(None);
        repo_state.repos = ["alpha", "beta", "gamma"]
            .into_iter()
            .map(|name| {
                RepoEntry::new(Repo {
                    name: name.into(),
                    path: PathBuf::from(format!("/{name}")),
                    worktrees: Vec::new(),
                })
            })
            .collect();
        repo_state.repo_list = SearchableList::new(3);
        repo_state.repo_list.selected = Some(1);
        repo_state.repo_filter_generation = 7;
        process_app_event(
            AppEvent::FilterCompleted {
                target: FilterTarget::Repos,
                generation: 7,
                matches: vec![
                    (FilterKey::Repo("/gamma".into()), 3),
                    (FilterKey::Repo("/beta".into()), 2),
                    (FilterKey::Repo("/alpha".into()), 1),
                ],
                selected: None,
            },
            &mut repo_state,
            &mut TickChanges::default(),
        );
        assert_eq!(repo_state.selected_repo().unwrap().repo.name, "beta");

        let mut branch_state = state_with_branch(false);
        branch_state.branch_view.entries = ["alpha", "beta", "gamma"]
            .into_iter()
            .map(|name| BranchEntry {
                name: name.into(),
                worktree_path: None,
                is_current: false,
                is_default: false,
                remote: None,
                open_workspace_id: None,
            })
            .collect();
        branch_state.branch_view.list = SearchableList::new(3);
        branch_state.branch_view.list.selected = Some(1);
        branch_state.branch_view.filter_generation = 9;
        process_app_event(
            AppEvent::FilterCompleted {
                target: FilterTarget::Branches,
                generation: 9,
                matches: vec![
                    (FilterKey::Branch("gamma".into()), 3),
                    (FilterKey::Branch("beta".into()), 2),
                    (FilterKey::Branch("alpha".into()), 1),
                ],
                selected: None,
            },
            &mut branch_state,
            &mut TickChanges::default(),
        );
        assert_eq!(branch_state.selected_branch().unwrap().name, "beta");

        let mut base_state = state_with_branch(false);
        base_state.mode = Mode::SelectBaseBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            flow: BaseBranchSelection {
                new_name: "new".into(),
                bases: vec!["alpha".into(), "beta".into(), "gamma".into()],
                list: SearchableList::new(3),
            },
        };
        if let Mode::SelectBaseBranch { flow, .. } = &mut base_state.mode {
            flow.list.selected = Some(1);
        }
        base_state.base_filter_generation = 11;
        process_app_event(
            AppEvent::FilterCompleted {
                target: FilterTarget::Bases,
                generation: 11,
                matches: vec![
                    (FilterKey::Base("gamma".into()), 3),
                    (FilterKey::Base("beta".into()), 2),
                    (FilterKey::Base("alpha".into()), 1),
                ],
                selected: None,
            },
            &mut base_state,
            &mut TickChanges::default(),
        );
        let Mode::SelectBaseBranch { flow, .. } = &base_state.mode else {
            unreachable!()
        };
        let selected = flow.list.selected.unwrap();
        let index = flow.list.filtered[selected].0;
        assert_eq!(flow.bases[index], "beta");
    }

    #[test]
    fn base_picker_text_actions_edit_only_the_base_query() {
        let mut state = state_with_branch(false);
        state.branch_view.list.input.text = "underlying".into();
        state.mode = Mode::SelectBaseBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            flow: BaseBranchSelection {
                new_name: "feat/new".into(),
                bases: vec!["main".into(), "feature".into()],
                list: SearchableList::new(2),
            },
        };
        let Mode::SelectBaseBranch { flow, .. } = &mut state.mode else {
            unreachable!()
        };
        flow.list.input.text = "one two".into();
        flow.list.input.cursor = flow.list.input.text.len();

        let git = Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>;
        let (sender, _rx) = sender();
        let worker = FilterWorker::spawn(sender.clone());
        let keys = KeysConfig::default();
        for action in [
            Action::CursorLeft,
            Action::CursorRight,
            Action::Backspace,
            Action::DeleteWord,
            Action::Insert('x'),
        ] {
            process_action(action, &mut state, &git, None, &sender, &worker, &keys);
        }
        let Mode::SelectBaseBranch { flow, .. } = &state.mode else {
            unreachable!()
        };
        assert_eq!(flow.list.input.text, "one x");
        assert_eq!(state.branch_view.list.input.text, "underlying");
    }

    #[test]
    fn unicode_actions_edit_repo_and_new_branch_queries() {
        let git = Arc::new(MockGitProvider::default()) as Arc<dyn GitProvider>;
        let (sender, _rx) = sender();
        let worker = FilterWorker::spawn(sender.clone());
        let keys = KeysConfig::default();
        let mut state = state_with_repo();
        process_action(
            Action::Insert('é'),
            &mut state,
            &git,
            None,
            &sender,
            &worker,
            &keys,
        );
        assert_eq!(state.repo_list.input.text, "é");

        state.mode = Mode::BranchSelect(BranchContext {
            repo_path: "/repo".into(),
            repo_name: "repo".into(),
        });
        process_action(
            Action::Insert('界'),
            &mut state,
            &git,
            None,
            &sender,
            &worker,
            &keys,
        );
        assert_eq!(state.branch_view.list.input.text, "界");
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
        state.branch_view.entries = vec![BranchEntry {
            name: "feature".into(),
            worktree_path: has_worktree.then(|| PathBuf::from("/repo-feature")),
            is_current: false,
            is_default: false,
            remote: None,
            open_workspace_id: None,
        }];
        state.branch_view.list = SearchableList::new(1);
        state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Loaded {
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
        };
        state
    }

    #[test]
    fn validating_new_branch_keeps_branch_view_visible_under_popup() {
        let mut state = state_with_branch(false);
        state.mode = Mode::ValidatingNewBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            name: "feat/new".into(),
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::from_config(&crate::config::ThemeConfig::default());
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &mut state,
                    &theme,
                    &KeysConfig::default(),
                    Instant::now(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("repo — select branch"));
        assert!(rendered.contains("feature"));
        assert!(rendered.contains("New branch \"feat/new\""));
        assert!(rendered.contains("Validating branch name…"));
    }

    fn worktree() -> WorktreeInfo {
        WorktreeInfo {
            path: "/repo-feature".into(),
            branch: Some("feature".into()),
            open_workspace_id: Some("w_1".into()),
        }
    }

    fn opened_worktree() -> OpenedWorktree {
        OpenedWorktree {
            root_pane_id: "p_root".into(),
            path: "/repo-feature".into(),
        }
    }

    fn create_response() -> WorktreeCreateResponse {
        WorktreeCreateResponse {
            opened: Some(opened_worktree()),
            warning: None,
        }
    }

    fn worktree_list_response(worktrees: Vec<WorktreeInfo>) -> WorktreeListResponse {
        WorktreeListResponse { worktrees }
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
    fn invalid_new_branch_name_is_validated_by_git_and_returns_to_branch_view() {
        let git_mock = Arc::new(MockGitProvider {
            invalid_branch_names: HashSet::from(["bad..name".into()]),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        state.branch_view.list.input.text = "bad..name".into();
        state.branch_view.list.input.cursor = 9;

        begin_start_new_branch(&mut state, &git, None, &sender);
        assert!(matches!(state.mode, Mode::ValidatingNewBranch { .. }));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &mut TickChanges::default());

        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(
            state
                .toasts
                .front()
                .unwrap()
                .message
                .contains("Invalid branch name")
        );
        assert_eq!(
            *git_mock.validation_calls.lock().unwrap(),
            [(PathBuf::from("/repo"), "bad..name".into())]
        );
    }

    #[test]
    fn new_branch_before_local_load_stays_in_branch_view_then_works_after_load() {
        let git_mock = Arc::new(MockGitProvider::default());
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        state.branch_view.entries.clear();
        state.branch_view.list = SearchableList::new(0);
        state.branch_view.list.input.text = "feat/new".into();
        state.branch_view.list.input.cursor = "feat/new".len();
        state.branch_view.loading = true;

        begin_start_new_branch(&mut state, &git, None, &sender);

        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(state.branch_view.loading);
        assert!(
            state
                .toasts
                .back()
                .unwrap()
                .message
                .contains("still loading")
        );
        assert!(rx.try_recv().is_err());

        state.branch_view.loading = false;
        begin_start_new_branch(&mut state, &git, None, &sender);

        assert!(matches!(state.mode, Mode::ValidatingNewBranch { .. }));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(event, AppEvent::BranchNameValidated { .. }));
        assert_eq!(
            *git_mock.validation_calls.lock().unwrap(),
            [(PathBuf::from("/repo"), "feat/new".into())]
        );
    }

    #[test]
    fn validated_new_branch_preselects_known_default_local_base() {
        let mut state = state_with_branch(false);
        state.branch_view.entries = vec![
            BranchEntry {
                name: "feature".into(),
                worktree_path: None,
                is_current: false,
                is_default: false,
                remote: None,
                open_workspace_id: None,
            },
            BranchEntry {
                name: "main".into(),
                worktree_path: Some("/repo".into()),
                is_current: true,
                is_default: true,
                remote: None,
                open_workspace_id: Some("w_1".into()),
            },
            BranchEntry {
                name: "remote".into(),
                worktree_path: None,
                is_current: false,
                is_default: false,
                remote: Some("origin".into()),
                open_workspace_id: None,
            },
        ];
        state.mode = Mode::ValidatingNewBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            name: "feat/new".into(),
        };
        process_app_event(
            AppEvent::BranchNameValidated {
                repo_path: "/repo".into(),
                branch_name: "feat/new".into(),
                valid: true,
                error: None,
            },
            &mut state,
            &mut TickChanges::default(),
        );

        let Mode::SelectBaseBranch { flow, .. } = &state.mode else {
            panic!("expected base picker")
        };
        assert_eq!(flow.bases, ["feature", "main"]);
        let selected = flow.list.filtered[flow.list.selected.unwrap()].0;
        assert_eq!(flow.bases[selected], "main");
    }

    #[test]
    fn selected_base_is_passed_to_focused_new_branch_creation() {
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock
            .worktree_create_results
            .lock()
            .unwrap()
            .push_back(Ok(create_response()));
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(false);
        state.mode = Mode::SelectBaseBranch {
            context: BranchContext {
                repo_path: "/repo".into(),
                repo_name: "repo".into(),
            },
            flow: BaseBranchSelection {
                new_name: "feat/new".into(),
                bases: vec!["main".into(), "feature".into()],
                list: SearchableList {
                    selected: Some(1),
                    ..SearchableList::new(2)
                },
            },
        };

        begin_create_new_branch(&mut state, Some(&herdr), &sender);
        assert!(matches!(
            &state.mode,
            Mode::Loading { message, .. } if message == "Creating feat/new from feature…"
        ));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut changes = TickChanges::default();
        process_app_event(event, &mut state, &mut changes);
        assert!(changes.workspace_opened);
        let calls = herdr_mock.calls.lock().unwrap();
        let HerdrCall::WorktreeCreate(request) = &calls[0] else {
            panic!("expected worktree create")
        };
        assert_eq!(request.branch, "feat/new");
        assert_eq!(request.base.as_deref(), Some("feature"));
        assert!(request.focus);
    }

    fn remove_response(_forced: bool) -> WorktreeRemoveResponse {
        WorktreeRemoveResponse { warning: None }
    }

    #[test]
    fn delete_before_open_state_load_is_refused() {
        let git_mock = Arc::new(MockGitProvider::default());
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        let mut state = state_with_branch(true);
        state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Unknown;

        begin_delete_worktree(&mut state);

        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(
            state
                .toasts
                .back()
                .is_some_and(|toast| toast.message.contains("still loading"))
        );
        assert!(git_mock.remove_calls.lock().unwrap().is_empty());
        assert!(herdr_mock.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn delete_after_open_state_failure_is_refused() {
        let mut state = state_with_branch(true);
        state.branch_view.open_worktree_load_state = OpenWorktreeLoadState::Failed {
            repo_path: "/repo".into(),
            generation: state.branch_view.generation,
        };

        begin_delete_worktree(&mut state);

        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(state.toasts.back().is_some_and(|toast| {
            toast
                .message
                .contains("open checkout state could not be loaded")
        }));
    }

    #[test]
    fn checkout_becoming_open_after_confirmation_routes_removal_through_herdr() {
        let git_mock = Arc::new(MockGitProvider::default());
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock
            .worktree_list_results
            .lock()
            .unwrap()
            .push_back(Ok(worktree_list_response(vec![worktree()])));
        herdr_mock
            .worktree_remove_results
            .lock()
            .unwrap()
            .push_back(Ok(remove_response(false)));
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(true);

        begin_delete_worktree(&mut state);
        assert!(matches!(
            &state.mode,
            Mode::ConfirmWorktreeDelete { target, .. }
                if target.open_workspace_id.is_none()
        ));

        confirm_delete_worktree(&mut state, &git, Some(&herdr), &sender);
        let _event = rx.recv_timeout(Duration::from_secs(1)).unwrap();

        assert_eq!(
            *herdr_mock.calls.lock().unwrap(),
            [
                HerdrCall::WorktreeList {
                    cwd: "/repo".into(),
                },
                HerdrCall::WorktreeRemove {
                    workspace_id: "w_1".into(),
                    force: false,
                },
            ]
        );
        assert!(git_mock.remove_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn fresh_open_state_failure_refuses_removal_without_falling_back_to_git() {
        let git_mock = Arc::new(MockGitProvider::default());
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock
            .worktree_list_results
            .lock()
            .unwrap()
            .push_back(Err(HerdrError::Invocation("herdr unavailable".into())));
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(true);

        begin_delete_worktree(&mut state);
        confirm_delete_worktree(&mut state, &git, Some(&herdr), &sender);
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(event, &mut state, &mut TickChanges::default());

        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(state.toasts.back().is_some_and(|toast| {
            toast
                .message
                .contains("could not refresh open checkout state")
        }));
        assert!(git_mock.remove_calls.lock().unwrap().is_empty());
        assert_eq!(
            *herdr_mock.calls.lock().unwrap(),
            [HerdrCall::WorktreeList {
                cwd: "/repo".into(),
            }]
        );
    }

    #[test]
    fn herdr_delete_requires_a_second_force_confirmation_then_refreshes() {
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock.worktree_list_results.lock().unwrap().extend([
            Ok(worktree_list_response(vec![worktree()])),
            Ok(worktree_list_response(vec![worktree()])),
        ]);
        herdr_mock.worktree_remove_results.lock().unwrap().extend([
            Err(HerdrError::DirtyWorktreeRequiresForce("dirty".into())),
            Ok(remove_response(true)),
        ]);
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let git = git_provider();
        let (sender, rx) = sender();
        let mut state = state_with_branch(true);
        state.branch_view.entries[0].open_workspace_id = Some("w_1".into());

        begin_delete_worktree(&mut state);
        confirm_delete_worktree(&mut state, &git, Some(&herdr), &sender);
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(first, &mut state, &mut TickChanges::default());
        assert!(matches!(
            &state.mode,
            Mode::ConfirmWorktreeDelete { target, .. }
                if target.force && !target.in_progress
        ));

        confirm_delete_worktree(&mut state, &git, Some(&herdr), &sender);
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut changes = TickChanges::default();
        process_app_event(second, &mut state, &mut changes);
        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(changes.refresh_branch.is_some());
        assert_eq!(
            *herdr_mock.calls.lock().unwrap(),
            [
                HerdrCall::WorktreeList {
                    cwd: "/repo".into(),
                },
                HerdrCall::WorktreeRemove {
                    workspace_id: "w_1".into(),
                    force: false,
                },
                HerdrCall::WorktreeList {
                    cwd: "/repo".into(),
                },
                HerdrCall::WorktreeRemove {
                    workspace_id: "w_1".into(),
                    force: true,
                },
            ]
        );
    }

    #[test]
    fn closed_git_checkout_requires_force_confirmation_and_prunes_after_success() {
        let git_mock = Arc::new(MockGitProvider::default());
        git_mock.dirty_remove_once.store(true, Ordering::Release);
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        let mut closed_worktree = worktree();
        closed_worktree.open_workspace_id = None;
        herdr_mock.worktree_list_results.lock().unwrap().extend([
            Ok(worktree_list_response(vec![closed_worktree.clone()])),
            Ok(worktree_list_response(vec![closed_worktree])),
        ]);
        let herdr: Arc<dyn HerdrProvider> = herdr_mock.clone();
        let (sender, rx) = sender();
        let mut state = state_with_branch(true);

        begin_delete_worktree(&mut state);
        confirm_delete_worktree(&mut state, &git, Some(&herdr), &sender);
        let first = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        process_app_event(first, &mut state, &mut TickChanges::default());
        assert!(matches!(
            &state.mode,
            Mode::ConfirmWorktreeDelete { target, .. }
                if target.force && !target.in_progress
        ));

        confirm_delete_worktree(&mut state, &git, Some(&herdr), &sender);
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let mut changes = TickChanges::default();
        process_app_event(second, &mut state, &mut changes);
        assert!(matches!(state.mode, Mode::BranchSelect(_)));
        assert!(changes.refresh_branch.is_some());
        assert_eq!(
            *git_mock.remove_calls.lock().unwrap(),
            [
                (
                    PathBuf::from("/repo"),
                    PathBuf::from("/repo-feature"),
                    false
                ),
                (PathBuf::from("/repo"), PathBuf::from("/repo-feature"), true),
            ]
        );
        assert_eq!(
            *git_mock.prune_calls.lock().unwrap(),
            [PathBuf::from("/repo")]
        );
        assert_eq!(
            *herdr_mock.calls.lock().unwrap(),
            [
                HerdrCall::WorktreeList {
                    cwd: "/repo".into(),
                },
                HerdrCall::WorktreeList {
                    cwd: "/repo".into(),
                },
            ]
        );
    }

    #[test]
    fn post_deletion_refresh_rejects_old_final_fetch_without_clearing_spinner() {
        let git = git_provider();
        let (sender, _rx) = sender();
        let mut state = state_with_branch(true);
        let old_generation = state.branch_view.generation;
        begin_delete_worktree(&mut state);
        if let Mode::ConfirmWorktreeDelete { target, .. } = &mut state.mode {
            target.in_progress = true;
        }
        let mut changes = TickChanges::default();

        process_app_event(
            AppEvent::WorktreeRemovalFinished {
                repo_path: "/repo".into(),
                branch_name: "feature".into(),
                worktree_path: "/repo-feature".into(),
                outcome: WorktreeRemovalOutcome::Removed { warning: None },
            },
            &mut state,
            &mut changes,
        );
        let repo = changes.refresh_branch.take().expect("branch refresh");
        crate::screens::branch::refresh(&mut state, &git, None, &sender, repo);
        assert_eq!(state.branch_view.generation, old_generation + 1);
        state.branch_view.fetching_remote_repo = Some("/repo".into());

        process_app_event(
            AppEvent::GitFetchCompleted {
                remote: Some("origin".into()),
                branches: Vec::new(),
                repo_path: "/repo".into(),
                generation: old_generation,
                error: None,
                is_final: true,
                skipped_unsupported_refs: false,
            },
            &mut state,
            &mut TickChanges::default(),
        );

        assert_eq!(
            state.branch_view.fetching_remote_repo.as_deref(),
            Some(Path::new("/repo"))
        );
    }

    #[test]
    fn late_recovered_delete_completion_does_not_change_a_newer_mode() {
        let mut state = state_with_repo();
        state.repos[0].repo.worktrees.push(Worktree {
            path: "/repo-feature".into(),
            branch: Some("feature".into()),
        });
        state
            .in_flight_worktree_removals
            .insert("/repo-feature".into());
        state.mark_pending_worktree_delete(PendingWorktreeDelete::new(
            "/repo".into(),
            "feature".into(),
            "/repo-feature".into(),
        ));
        let mut changes = TickChanges::default();

        process_app_event(
            AppEvent::WorktreeRemovalFinished {
                repo_path: "/repo".into(),
                branch_name: "feature".into(),
                worktree_path: "/repo-feature".into(),
                outcome: WorktreeRemovalOutcome::Removed { warning: None },
            },
            &mut state,
            &mut changes,
        );

        assert_eq!(state.mode, Mode::RepoSelect);
        assert!(state.pending_worktree_deletes.is_empty());
        assert!(state.repos[0].repo.worktrees.is_empty());
        assert!(changes.refresh_branch.is_none());
    }
}
