use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    config::{OnOpenConfig, SortOrder},
    git::Repo,
    herdr::WorktreeInfo,
    pins::PinStore,
    recency::RecencyStore,
    screens::{
        branch::BranchViewState, delete::DeleteState, new_branch::NewBranchState,
        repo::RepoViewState,
    },
};

pub use crate::screens::branch::{BranchContext, BranchId, OpenWorktreeLoadState};
pub use crate::screens::delete::DeleteFlowState;
pub use crate::screens::new_branch::BaseBranchSelection;
pub use crate::screens::repo::RepoEntry;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpenFilter {
    #[default]
    All,
    OpenOnly,
}

impl OpenFilter {
    pub const fn includes(self, is_open: bool) -> bool {
        matches!(self, Self::All) || is_open
    }

    pub const fn is_active(self) -> bool {
        matches!(self, Self::OpenOnly)
    }

    pub fn toggle(&mut self) {
        *self = match self {
            Self::All => Self::OpenOnly,
            Self::OpenOnly => Self::All,
        };
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
    ConfirmWorktreeDelete(DeleteFlowState),
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
    pub repo_view: RepoViewState,
    pub branch_view: BranchViewState,
    pub mode: Mode,
    pub toasts: VecDeque<Toast>,
    toast_messages: HashSet<String>,
    front_toast_since: Option<Instant>,
    scan_warning_count: usize,
    pub help_overlay: Option<HelpOverlayState>,
    pub active_list_rows: usize,
    pub help_filter_generation: u64,
    pub(crate) delete: DeleteState,
    pub(crate) new_branch: NewBranchState,
    pub on_open: OnOpenConfig,
    pub sort_order: SortOrder,
    pub recency: RecencyStore,
    pub pins: PinStore,
}

impl AppState {
    pub fn new(current_cwd: Option<PathBuf>) -> Self {
        Self {
            repo_view: RepoViewState::new(current_cwd),
            branch_view: BranchViewState::default(),
            mode: Mode::RepoSelect,
            toasts: VecDeque::new(),
            toast_messages: HashSet::new(),
            front_toast_since: None,
            scan_warning_count: 0,
            help_overlay: None,
            active_list_rows: 1,
            help_filter_generation: 0,
            delete: DeleteState::default(),
            new_branch: NewBranchState::default(),
            on_open: OnOpenConfig::default(),
            sort_order: SortOrder::Alphabetical,
            recency: RecencyStore::default(),
            pins: PinStore::default(),
        }
    }

    pub fn configure_sort(&mut self, sort_order: SortOrder, recency: RecencyStore, pins: PinStore) {
        self.sort_order = sort_order;
        self.recency = recency;
        self.pins = pins;
    }

    pub fn selected_repo(&self) -> Option<&RepoEntry> {
        self.repo_view.selected()
    }

    pub fn selected_branch(&self) -> Option<&BranchEntry> {
        self.branch_view.selected()
    }

    pub fn branch_context(&self) -> Option<&BranchContext> {
        match &self.mode {
            Mode::BranchSelect(context)
            | Mode::ValidatingNewBranch { context, .. }
            | Mode::SelectBaseBranch { context, .. }
            | Mode::Loading {
                branch: Some(context),
                ..
            } => Some(context),
            Mode::ConfirmWorktreeDelete(flow) => Some(flow.context()),
            Mode::RepoSelect | Mode::Loading { branch: None, .. } => None,
        }
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
        self.front_toast_since = None;
    }

    pub fn tick_toasts(&mut self, now: Instant, ttl: Duration) -> bool {
        if self.toasts.is_empty() {
            self.front_toast_since = None;
            return false;
        }
        let Some(since) = self.front_toast_since else {
            self.front_toast_since = Some(now);
            return false;
        };
        if now.saturating_duration_since(since) < ttl {
            return false;
        }
        self.dismiss_toast();
        true
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
        let queue_was_empty = self.toasts.is_empty();
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
            if remove == 0 {
                self.front_toast_since = None;
            }
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
        if queue_was_empty {
            self.front_toast_since = None;
        }
    }
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
            is_git: true,
            worktrees: Vec::new(),
        }
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
    fn toast_timer_arms_and_dismisses_at_the_ttl() {
        let mut state = AppState::new(None);
        let start = Instant::now();
        let ttl = Duration::from_secs(5);
        let before_ttl = ttl.saturating_sub(Duration::from_millis(1));
        state.push_toast(ToastKind::Warning, "first");

        assert!(!state.tick_toasts(start, ttl));
        assert!(!state.tick_toasts(start + before_ttl, ttl));
        assert_eq!(state.toasts.len(), 1);
        assert!(state.tick_toasts(start + ttl, ttl));
        assert!(state.toasts.is_empty());
        assert!(!state.tick_toasts(start + ttl, ttl));
    }

    #[test]
    fn queued_toast_does_not_restart_the_front_timer_and_gets_a_fresh_ttl() {
        let mut state = AppState::new(None);
        let start = Instant::now();
        let ttl = Duration::from_secs(5);
        let before_ttl = ttl.saturating_sub(Duration::from_millis(1));
        state.push_toast(ToastKind::Warning, "first");
        assert!(!state.tick_toasts(start, ttl));

        state.push_toast(ToastKind::Error, "second");
        assert!(!state.tick_toasts(start + Duration::from_secs(4), ttl));
        assert!(state.tick_toasts(start + ttl, ttl));
        assert_eq!(
            state.toasts.front().map(|toast| toast.message.as_str()),
            Some("second")
        );

        let second_start = start + ttl + Duration::from_millis(40);
        assert!(!state.tick_toasts(second_start, ttl));
        assert!(!state.tick_toasts(second_start + before_ttl, ttl));
        assert!(state.tick_toasts(second_start + ttl, ttl));
        assert!(state.toasts.is_empty());
    }

    #[test]
    fn manual_dismiss_gives_the_next_toast_a_fresh_ttl() {
        let mut state = AppState::new(None);
        let start = Instant::now();
        let ttl = Duration::from_secs(5);
        let before_ttl = ttl.saturating_sub(Duration::from_millis(1));
        state.push_toast(ToastKind::Warning, "first");
        state.push_toast(ToastKind::Error, "second");
        assert!(!state.tick_toasts(start, ttl));

        let second_start = start + Duration::from_secs(4);
        state.dismiss_toast();
        assert!(!state.tick_toasts(second_start, ttl));
        assert!(!state.tick_toasts(second_start + before_ttl, ttl));
        assert!(state.tick_toasts(second_start + ttl, ttl));
        assert!(state.toasts.is_empty());
    }

    #[test]
    fn branch_entries_derive_current_default_worktree_and_open_markers() {
        let repo = Repo {
            name: "repo".into(),
            path: "/repo".into(),
            is_git: true,
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
}
