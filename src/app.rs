use std::{
    collections::HashMap,
    path::PathBuf,
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
    event::{AppEvent, FilterKey, FilterTarget},
    git::{GitProvider, Repo},
    herdr::HerdrProvider,
    keyboard::Action,
    spawn::{
        EventSender, FetchDeduplicator, spawn_git_fetch, spawn_remote_branch_loading,
        spawn_repo_discovery, spawn_workspace_list,
    },
    state::{AppState, BranchId, Mode, SearchableList},
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

        crate::screens::repo::apply_changes(state, &mut changes, &filter_worker);
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
            crate::screens::delete::resume(state, git, herdr, &sender);
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
    let Some(event) = crate::screens::delete::handle_event(event, state, changes) else {
        return;
    };
    let Some(event) = crate::screens::repo::handle_event(event, state, changes) else {
        return;
    };
    let Some(event) = crate::screens::new_branch::handle_event(event, state, changes) else {
        return;
    };
    match event {
        AppEvent::FilterCompleted {
            target,
            generation,
            matches,
            selected,
        } => match target {
            FilterTarget::Help if generation == state.help_filter_generation => {
                apply_help_filter_result(state, &matches, selected.as_ref());
            }
            FilterTarget::Repos
            | FilterTarget::Branches
            | FilterTarget::Bases
            | FilterTarget::Help => {}
        },
        AppEvent::RepoOpened { warning } => {
            changes.open_warning = warning;
            changes.workspace_opened = true;
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
                crate::screens::repo::move_selection(state, delta);
            } else if matches!(state.mode, Mode::BranchSelect(_)) {
                crate::screens::branch::move_selection(state, delta);
            } else if matches!(state.mode, Mode::SelectBaseBranch { .. }) {
                crate::screens::new_branch::move_selection(state, delta);
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
        Action::OpenRepo => crate::screens::repo::open_selected(state, herdr, sender),
        Action::OpenBranches => crate::screens::branch::enter(state, git, herdr, sender),
        Action::OpenBranch => crate::screens::branch::open_selected(state, git, herdr, sender),
        Action::StartNewBranch => crate::screens::new_branch::start(state, git, herdr, sender),
        Action::CreateNewBranch => crate::screens::new_branch::create(state, herdr, sender),
        Action::DeleteWorktree => crate::screens::delete::begin(state),
        Action::ConfirmDeleteWorktree => {
            crate::screens::delete::confirm(state, git, herdr, sender);
        }
        Action::CancelOverlay => cancel_overlay(state),
        Action::BackToRepos => {
            crate::screens::branch::leave(state);
            crate::screens::repo::queue_filter(state, filter_worker, true);
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
        | Mode::ConfirmWorktreeDelete(_) => &mut state.repo_view.list,
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
            crate::screens::repo::edit(state, worker, edit);
        }
        Mode::BranchSelect(_) => {
            crate::screens::branch::edit(state, worker, edit);
        }
        Mode::SelectBaseBranch { .. } => {
            crate::screens::new_branch::edit(state, worker, edit);
        }
        Mode::Loading { .. }
        | Mode::ValidatingNewBranch { .. }
        | Mode::ConfirmWorktreeDelete(_) => {}
    }
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

fn cancel_overlay(state: &mut AppState) {
    if crate::screens::delete::cancel(state) {
        return;
    }
    crate::screens::new_branch::cancel(state);
}

pub(crate) fn draw(
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
        Mode::ConfirmWorktreeDelete(flow) => {
            components::branch_picker::draw(frame, main_area, state, theme, spinner_start);
            crate::screens::delete::draw_dialog(frame, main_area, flow, theme, keys, spinner_start);
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
        Mode::RepoSelect => state.repo_view.loading,
        Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. } => branch_spinner,
        Mode::ValidatingNewBranch { .. } | Mode::Loading { .. } => true,
        Mode::ConfirmWorktreeDelete(flow) => branch_spinner || flow.in_progress(),
    }
}

fn loading_hint(keys: &KeysConfig) -> Option<String> {
    keys.first_key(BindingMode::Modal, Command::Quit)
        .map(|key| format!("{key} to close (operation continues)"))
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
            Mode::SelectBaseBranch { .. } | Mode::ConfirmWorktreeDelete(_)
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
        Mode::BranchSelect(_) | Mode::SelectBaseBranch { .. } | Mode::ConfirmWorktreeDelete(_)
    ) {
        add(Command::Back, "back");
    } else {
        add(Command::Clear, "clear/quit");
    }
    add(Command::Help, "help");
    add(Command::Quit, "quit");
    hints
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use crate::{
        git::{Repo, mock::MockGitProvider},
        state::{BranchContext, BranchEntry, BranchId, OpenWorktreeLoadState, RepoEntry},
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
            crate::screens::delete::dialog_hints(&keys),
            (Some("ctrl+y".into()), Some("ctrl+g".into()))
        );
    }

    #[test]
    fn branch_logical_selection_survives_current_generation_filter_results() {
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
        assert_eq!(state.repo_view.list.input.text, "é");

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
        state.repo_view.entries.push(RepoEntry::new(Repo {
            name: "repo".into(),
            path: "/repo".into(),
            worktrees: Vec::new(),
        }));
        state.repo_view.list = crate::state::SearchableList::new(1);
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

    fn sender() -> (EventSender, mpsc::Receiver<AppEvent>) {
        let (tx, rx) = mpsc::channel();
        (EventSender::new(tx, Arc::new(AtomicBool::new(false))), rx)
    }
}
