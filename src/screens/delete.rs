use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app::TickChanges,
    components,
    config::{
        ConfigWarning,
        keys::{BindingMode, Command, KeysConfig},
    },
    event::{AppEvent, WorktreeRemovalOutcome},
    git::GitProvider,
    herdr::HerdrProvider,
    pending_delete::PendingWorktreeDelete,
    spawn::{EventSender, spawn_worktree_removal},
    state::{AppState, BranchContext, Mode, OpenWorktreeLoadState, ToastKind},
    theme::Theme,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteWorktreeTarget {
    pub(crate) branch_name: String,
    pub(crate) worktree_path: PathBuf,
    pub(crate) open_workspace_id: Option<String>,
    pub(crate) force: bool,
    pub(crate) in_progress: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteFlowState {
    context: BranchContext,
    pub(crate) target: DeleteWorktreeTarget,
}

impl DeleteFlowState {
    pub(crate) fn new(context: BranchContext, target: DeleteWorktreeTarget) -> Self {
        Self { context, target }
    }

    pub(crate) fn context(&self) -> &BranchContext {
        &self.context
    }

    pub(crate) fn in_progress(&self) -> bool {
        self.target.in_progress
    }
}

#[derive(Debug, Default)]
pub(crate) struct DeleteState {
    pending: Vec<PendingWorktreeDelete>,
    in_flight: HashSet<PathBuf>,
}

pub fn load_pending(state: &mut AppState) -> Vec<ConfigWarning> {
    let loaded = crate::pending_delete::load_pending_worktree_deletes();
    state.delete.pending = loaded.entries;
    loaded.warnings
}

pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    changes: &mut TickChanges,
) -> Option<AppEvent> {
    let AppEvent::WorktreeRemovalFinished {
        repo_path,
        branch_name,
        worktree_path,
        outcome,
    } = event
    else {
        return Some(event);
    };
    if !event_matches(state, &repo_path, &branch_name, &worktree_path)
        && !state.delete.in_flight.contains(&worktree_path)
    {
        return Some(AppEvent::WorktreeRemovalFinished {
            repo_path,
            branch_name,
            worktree_path,
            outcome,
        });
    }

    let ui_matches = event_matches(state, &repo_path, &branch_name, &worktree_path)
        || crate::screens::branch::matches_repo(state, &repo_path);
    state.delete.in_flight.remove(&worktree_path);
    match outcome {
        WorktreeRemovalOutcome::DirtyRequiresForce => {
            handle_dirty(state, ui_matches, branch_name, worktree_path);
        }
        WorktreeRemovalOutcome::Removed { warning } => {
            handle_removed(
                state,
                changes,
                ui_matches,
                &repo_path,
                &branch_name,
                &worktree_path,
                warning,
            );
        }
        WorktreeRemovalOutcome::Failed(error) => {
            handle_failed(
                state,
                ui_matches,
                repo_path,
                &branch_name,
                &worktree_path,
                &error,
            );
        }
    }
    None
}

fn handle_dirty(
    state: &mut AppState,
    ui_matches: bool,
    branch_name: String,
    worktree_path: PathBuf,
) {
    if let Mode::ConfirmWorktreeDelete(flow) = &mut state.mode {
        if flow.target.worktree_path == worktree_path {
            flow.target.force = true;
            flow.target.in_progress = false;
        }
    } else if ui_matches && let Mode::BranchSelect(context) = &state.mode {
        let context = context.clone();
        let open_workspace_id = state
            .branch_view
            .entries
            .iter()
            .find(|branch| branch.name == branch_name)
            .and_then(|branch| branch.open_workspace_id.clone());
        state.mode = Mode::ConfirmWorktreeDelete(DeleteFlowState::new(
            context,
            DeleteWorktreeTarget {
                branch_name,
                worktree_path,
                open_workspace_id,
                force: true,
                in_progress: false,
            },
        ));
    }
}

fn handle_removed(
    state: &mut AppState,
    changes: &mut TickChanges,
    ui_matches: bool,
    repo_path: &Path,
    branch_name: &str,
    worktree_path: &Path,
    warning: Option<String>,
) {
    clear_pending(state, worktree_path);
    persist(state);
    if let Some(entry) = state
        .repo_view
        .entries
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
        repo_path: repo_path.to_path_buf(),
        repo_name: state
            .repo_view
            .entries
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
        .repo_view
        .entries
        .iter()
        .find(|entry| entry.repo.path == repo_path)
        .map(|entry| entry.repo.clone());
    if let Some(warning) = warning {
        state.push_toast(ToastKind::Warning, warning);
    }
}

fn handle_failed(
    state: &mut AppState,
    ui_matches: bool,
    repo_path: PathBuf,
    branch_name: &str,
    worktree_path: &Path,
    error: &str,
) {
    clear_pending(state, worktree_path);
    persist(state);
    if !ui_matches {
        state.push_toast(
            ToastKind::Error,
            format!("Failed to remove checkout for {branch_name}: {error}"),
        );
        return;
    }
    let context = BranchContext {
        repo_path,
        repo_name: state
            .branch_context()
            .map_or_else(|| "repository".into(), |context| context.repo_name.clone()),
    };
    state.mode = Mode::BranchSelect(context);
    state.push_toast(
        ToastKind::Error,
        format!("Failed to remove checkout for {branch_name}: {error}"),
    );
}

pub(crate) fn begin(state: &mut AppState) {
    let context = match &state.mode {
        Mode::BranchSelect(context) => context.clone(),
        _ => return,
    };
    match selected_target(state) {
        Ok(target) => {
            state.mode = Mode::ConfirmWorktreeDelete(DeleteFlowState::new(context, target));
        }
        Err(message) => state.push_toast(ToastKind::Error, message),
    }
}

pub(crate) fn confirm(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    let Mode::ConfirmWorktreeDelete(flow) = &state.mode else {
        return;
    };
    if flow.target.in_progress {
        return;
    }
    let context = flow.context.clone();
    let mut target_snapshot = flow.target.clone();
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
    if let Mode::ConfirmWorktreeDelete(flow) = &mut state.mode {
        flow.target
            .open_workspace_id
            .clone_from(&target_snapshot.open_workspace_id);
    }
    let mut pending = PendingWorktreeDelete::new(
        context.repo_path.clone(),
        target_snapshot.branch_name.clone(),
        target_snapshot.worktree_path.clone(),
    );
    pending.force = target_snapshot.force;
    mark_pending(state, pending);
    if let Err(error) = save_pending(&state.delete.pending) {
        clear_pending(state, &target_snapshot.worktree_path);
        state.push_toast(
            ToastKind::Error,
            format!("Could not persist pending deletion: {error:#}"),
        );
        return;
    }
    if let Mode::ConfirmWorktreeDelete(flow) = &mut state.mode {
        flow.target.in_progress = true;
    }
    state
        .delete
        .in_flight
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

pub(crate) fn cancel(state: &mut AppState) -> bool {
    let Mode::ConfirmWorktreeDelete(flow) = state.mode.clone() else {
        return false;
    };
    if !flow.target.in_progress {
        if flow.target.force {
            clear_pending(state, &flow.target.worktree_path);
            persist(state);
        }
        state.mode = Mode::BranchSelect(flow.context);
    }
    true
}

pub(crate) fn resume(
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
        .delete
        .pending
        .iter()
        .filter(|pending| pending.repo_path == repo_path)
        .cloned()
        .collect::<Vec<_>>();
    for pending in pending {
        if !state.delete.in_flight.insert(pending.worktree_path.clone()) {
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

pub(crate) fn reconcile_pending(state: &mut AppState, repo_path: &Path) {
    let active = state
        .repo_view
        .entries
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
    let before = state.delete.pending.len();
    state.delete.pending.retain(|pending| {
        pending.repo_path != repo_path
            || !pending.is_expired() && active.contains(pending.worktree_path.as_path())
    });
    if before != state.delete.pending.len() {
        persist(state);
    }
}

pub(crate) fn refresh_open_state(state: &mut AppState) {
    let Mode::ConfirmWorktreeDelete(flow) = &state.mode else {
        return;
    };
    let workspace_id = state
        .branch_view
        .entries
        .iter()
        .find(|branch| {
            branch.name == flow.target.branch_name
                && branch.worktree_path.as_ref() == Some(&flow.target.worktree_path)
        })
        .and_then(|branch| branch.open_workspace_id.clone());
    if let Mode::ConfirmWorktreeDelete(flow) = &mut state.mode {
        flow.target.open_workspace_id = workspace_id;
    }
}

pub(crate) fn draw_dialog(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    flow: &DeleteFlowState,
    theme: &Theme,
    keys: &KeysConfig,
    spinner_start: Instant,
) {
    let target = &flow.target;
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
        let (confirm, cancel) = dialog_hints(keys);
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

pub(crate) fn dialog_hints(keys: &KeysConfig) -> (Option<String>, Option<String>) {
    (
        keys.first_key(BindingMode::Modal, Command::Open)
            .map(|key| key.to_string()),
        keys.first_key(BindingMode::Modal, Command::Back)
            .map(|key| key.to_string()),
    )
}

fn selected_target(state: &AppState) -> Result<DeleteWorktreeTarget, &'static str> {
    let Mode::BranchSelect(context) = &state.mode else {
        return Err("Worktrees can only be deleted from the branch view");
    };
    let branch = state.selected_branch().ok_or("No branch selected")?;
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
    if state.delete.in_flight.contains(&worktree_path)
        || state.delete.pending.iter().any(|pending| {
            pending.repo_path == context.repo_path && pending.branch_name == branch.name
        })
    {
        return Err("Worktree deletion already in progress");
    }
    match &state.branch_view.open_worktree_load_state {
        OpenWorktreeLoadState::Loaded {
            repo_path,
            generation,
        } if repo_path == &context.repo_path && *generation == state.branch_view.generation => {}
        OpenWorktreeLoadState::Failed {
            repo_path,
            generation,
        } if repo_path == &context.repo_path && *generation == state.branch_view.generation => {
            return Err("Cannot delete checkout because open checkout state could not be loaded");
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

fn event_matches(
    state: &AppState,
    repo_path: &Path,
    branch_name: &str,
    worktree_path: &Path,
) -> bool {
    matches!(
        &state.mode,
        Mode::ConfirmWorktreeDelete(flow)
            if flow.context.repo_path == repo_path
                && flow.target.branch_name == branch_name
                && flow.target.worktree_path == worktree_path
                && flow.target.in_progress
    )
}

fn mark_pending(state: &mut AppState, pending: PendingWorktreeDelete) {
    state.delete.pending.retain(|entry| {
        !(entry.repo_path == pending.repo_path && entry.branch_name == pending.branch_name)
    });
    state.delete.pending.push(pending);
}

fn clear_pending(state: &mut AppState, worktree_path: &Path) {
    state
        .delete
        .pending
        .retain(|pending| pending.worktree_path != worktree_path);
}

pub(crate) fn persist(state: &mut AppState) {
    if let Err(error) = save_pending(&state.delete.pending) {
        state.push_toast(
            ToastKind::Error,
            format!("Could not persist pending deletions: {error:#}"),
        );
    }
}

#[cfg(not(test))]
fn save_pending(entries: &[PendingWorktreeDelete]) -> anyhow::Result<()> {
    crate::pending_delete::save_pending_worktree_deletes(entries)
}

#[cfg(test)]
#[allow(clippy::unnecessary_wraps)]
fn save_pending(_entries: &[PendingWorktreeDelete]) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests;
