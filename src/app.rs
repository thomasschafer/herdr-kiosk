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
    event::AppEvent,
    git::{GitProvider, Repo},
    herdr::HerdrProvider,
    keyboard::Action,
    spawn::{EventSender, spawn_open_repo, spawn_repo_discovery, spawn_workspace_list},
    state::{AppState, Mode, RepoEntry, ToastKind, collision_disambiguators},
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
    path: PathBuf,
    text: String,
}

struct FilterRequest {
    generation: u64,
    query: String,
    items: Vec<FilterItem>,
    selected_path: Option<PathBuf>,
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
                    generation: request.generation,
                    matches: filtered,
                    selected_path: request.selected_path,
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

fn fuzzy_filter(query: &str, items: &[FilterItem], matcher: &SkimMatcherV2) -> Vec<(PathBuf, i64)> {
    if query.is_empty() {
        return items.iter().map(|item| (item.path.clone(), 0)).collect();
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
        .map(|(item, score)| (item.path.clone(), score))
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

        if changes.repo_opened {
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
        if changes.repos_changed {
            queue_filter(state, &filter_worker, true);
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
            if let Some(outcome) = process_action(action, state, herdr, &sender, &filter_worker) {
                break outcome;
            }
        }
    };

    cancel.store(true, Ordering::Relaxed);
    drop(filter_worker);
    Ok(outcome)
}

#[derive(Default)]
struct TickChanges {
    repos_changed: bool,
    collision_pass: bool,
    repo_opened: bool,
}

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
            generation,
            matches,
            selected_path,
        } if generation == state.filter_generation => {
            apply_filter_result(state, &matches, selected_path.as_deref());
        }
        AppEvent::RepoOpened => changes.repo_opened = true,
        AppEvent::HerdrError(message) => {
            state.mode = Mode::RepoSelect;
            state.push_toast(ToastKind::Error, message);
        }
        AppEvent::GitError(message) => state.push_toast(ToastKind::Warning, message),
        _ => {}
    }
}

fn process_action(
    action: Action,
    state: &mut AppState,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
    filter_worker: &FilterWorker,
) -> Option<RunOutcome> {
    match action {
        Action::Quit => return Some(RunOutcome::Quit),
        Action::MoveSelection(delta) => {
            state.selection_touched = true;
            state.repo_list.move_selection(delta);
        }
        Action::Insert(character) => {
            state.selection_touched = true;
            state.repo_list.input.insert_char(character);
            queue_filter(state, filter_worker, false);
        }
        Action::Backspace => {
            state.selection_touched = true;
            state.repo_list.input.backspace();
            queue_filter(state, filter_worker, false);
        }
        Action::DeleteWord => {
            state.selection_touched = true;
            state.repo_list.input.delete_word();
            queue_filter(state, filter_worker, false);
        }
        Action::CursorLeft => state.repo_list.input.cursor_left(),
        Action::CursorRight => state.repo_list.input.cursor_right(),
        Action::ClearQuery => {
            state.repo_list.input.clear();
            queue_filter(state, filter_worker, false);
        }
        Action::OpenRepo => begin_open_selected(state, herdr, sender),
        Action::DismissToast => {
            state.toasts.pop_front();
        }
        Action::Noop => {}
    }
    None
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

fn queue_filter(state: &mut AppState, worker: &FilterWorker, preserve_selection: bool) {
    state.filter_generation = state.filter_generation.wrapping_add(1);
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
    let selected_path = preserve_selection
        .then(|| state.selected_repo().map(|entry| entry.repo.path.clone()))
        .flatten();
    worker.request(FilterRequest {
        generation: state.filter_generation,
        query: state.repo_list.input.text.clone(),
        items: state
            .repos
            .iter()
            .map(|entry| FilterItem {
                path: entry.repo.path.clone(),
                text: entry.display_name(),
            })
            .collect(),
        selected_path,
    });
}

fn apply_filter_result(
    state: &mut AppState,
    matches: &[(PathBuf, i64)],
    selected_path: Option<&Path>,
) {
    let indices: HashMap<_, _> = state
        .repos
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.repo.path.as_path(), index))
        .collect();
    state.repo_list.filtered = matches
        .iter()
        .filter_map(|(path, score)| indices.get(path.as_path()).map(|index| (*index, *score)))
        .collect();
    state.repo_list.selected = selected_path
        .and_then(|path| {
            state
                .repo_list
                .filtered
                .iter()
                .position(|(index, _)| state.repos[*index].repo.path == path)
        })
        .or_else(|| (!state.repo_list.filtered.is_empty()).then_some(0));
    state.repo_list.scroll_offset = 0;
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
    state.mode = Mode::Loading(format!("Opening {repo_name}…"));
    spawn_open_repo(provider, sender, repo_path);
}

fn draw(frame: &mut Frame, state: &mut AppState, theme: &Theme, spinner_start: Instant) {
    if let Mode::Loading(message) = &state.mode {
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
    components::repo_list::draw(frame, main_area, state, theme, spinner_start);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("↑/↓", Style::default().fg(theme.hint)),
            Span::raw(" move  "),
            Span::styled("Enter", Style::default().fg(theme.hint)),
            Span::raw(" open  "),
            Span::styled("Esc", Style::default().fg(theme.hint)),
            Span::raw(" clear/quit  "),
            Span::styled("Ctrl+C", Style::default().fg(theme.hint)),
            Span::raw(" quit"),
        ]))
        .alignment(Alignment::Center),
        footer_area,
    );
    components::error_toast::draw(frame, frame.area(), state, theme);
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicBool, time::Duration};

    use crate::{
        git::Repo,
        herdr::{HerdrError, HerdrProvider, mock::MockHerdrProvider},
    };

    use super::*;

    fn items(names: &[&str]) -> Vec<FilterItem> {
        names
            .iter()
            .map(|name| FilterItem {
                path: PathBuf::from(format!("/{name}")),
                text: (*name).into(),
            })
            .collect()
    }

    fn names(matches: &[(PathBuf, i64)]) -> Vec<String> {
        matches
            .iter()
            .map(|(path, _)| path.file_name().unwrap().to_string_lossy().into_owned())
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
            path: "/repo".into(),
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

    #[test]
    fn opening_transitions_to_loading_and_dispatches_through_mock_provider() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_open_results
            .lock()
            .unwrap()
            .push_back(Err(HerdrError::WorktreeOpenFailed("boom".into())));
        let provider: Arc<dyn HerdrProvider> = mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        let mut state = state_with_repo();

        begin_open_selected(&mut state, Some(&provider), &sender);

        assert_eq!(state.mode, Mode::Loading("Opening repo…".into()));
        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            &event,
            AppEvent::HerdrError(message) if message.contains("boom")
        ));
        process_app_event(event, &mut state, &mut TickChanges::default());
        assert_eq!(state.mode, Mode::RepoSelect);
        assert!(state.toasts.front().unwrap().message.contains("boom"));
        assert_eq!(mock.calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn opening_without_herdr_keeps_picker_usable_and_shows_error() {
        let (tx, _rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        let mut state = state_with_repo();
        begin_open_selected(&mut state, None, &sender);
        assert_eq!(state.mode, Mode::RepoSelect);
        assert_eq!(
            state.toasts.front().unwrap().message,
            "not running inside herdr"
        );
    }
}
