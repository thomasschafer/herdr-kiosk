use std::{collections::HashMap, sync::Arc};

use crate::{
    app::{FilterItem, FilterRequest, FilterWorker, TickChanges},
    event::{AppEvent, FilterKey, FilterTarget},
    git::GitProvider,
    herdr::HerdrProvider,
    spawn::{EventSender, spawn_create_new_branch, spawn_validate_branch_name},
    state::{AppState, BranchContext, BranchEntry, Mode, SearchableList, ToastKind},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseBranchSelection {
    pub new_name: String,
    pub bases: Vec<String>,
    pub list: SearchableList,
}

#[derive(Debug, Default)]
pub(crate) struct NewBranchState {
    pub(crate) filter_generation: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum NewBranchRoute {
    Existing(BranchEntry),
    Validate {
        context: BranchContext,
        name: String,
    },
}

pub(crate) fn handle_event(
    event: AppEvent,
    state: &mut AppState,
    _changes: &mut TickChanges,
) -> Option<AppEvent> {
    match event {
        AppEvent::FilterCompleted {
            target: FilterTarget::Bases,
            generation,
            matches,
            selected,
        } if generation == state.new_branch.filter_generation => {
            apply_filter_result(state, &matches, selected.as_ref());
        }
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
            handle_validation(state, branch_name, valid, error);
        }
        event => return Some(event),
    }
    None
}

pub(crate) fn move_selection(state: &mut AppState, delta: i32) {
    if let Mode::SelectBaseBranch { flow, .. } = &mut state.mode {
        flow.list.move_selection(delta);
    }
}

pub(crate) fn edit(
    state: &mut AppState,
    worker: &FilterWorker,
    edit: impl FnOnce(&mut SearchableList),
) {
    if let Mode::SelectBaseBranch { flow, .. } = &mut state.mode {
        edit(&mut flow.list);
    }
    queue_filter(state, worker, None);
}

pub(crate) fn start(
    state: &mut AppState,
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
) {
    match route(state) {
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

pub(crate) fn create(
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

pub(crate) fn cancel(state: &mut AppState) -> bool {
    let mode = state.mode.clone();
    if let Mode::SelectBaseBranch { context, .. } = mode {
        state.mode = Mode::BranchSelect(context);
        true
    } else {
        false
    }
}

fn route(state: &AppState) -> Result<NewBranchRoute, &'static str> {
    let Mode::BranchSelect(context) = &state.mode else {
        return Err("New branches can only be created from the branch view");
    };
    if state.branch_view.loading {
        return Err("Branches are still loading");
    }
    let name = state.branch_view.list.input.text.clone();
    if name.is_empty() {
        return Err("Type a branch name first");
    }
    if let Some(branch) = state
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

fn handle_validation(
    state: &mut AppState,
    branch_name: String,
    valid: bool,
    error: Option<String>,
) {
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
            state.new_branch.filter_generation = state.new_branch.filter_generation.wrapping_add(1);
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

fn queue_filter(state: &mut AppState, worker: &FilterWorker, selected_name: Option<String>) {
    state.new_branch.filter_generation = state.new_branch.filter_generation.wrapping_add(1);
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
        generation: state.new_branch.filter_generation,
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

fn apply_filter_result(
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

#[cfg(test)]
#[path = "new_branch/tests.rs"]
mod tests;
