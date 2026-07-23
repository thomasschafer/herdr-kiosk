use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use rayon::ThreadPoolBuilder;

use crate::{
    config::{OnOpenConfig, ResolvedSearchDir},
    event::{AppEvent, BranchOperationFailure, WorktreeRemovalOutcome},
    git::{
        GitProvider, Repo, ScanWarning, is_dirty_worktree_requires_force,
        is_local_branch_already_exists,
    },
    herdr::{
        HerdrError, HerdrProvider, OpenedWorktree, PaneSplitRequest, WorktreeCreateRequest,
        WorktreeOpenTarget,
    },
    state::BranchEntry,
};

/// Bounds concurrent remote fetches.
pub const FETCH_POOL_SIZE: usize = 4;

type FetchKey = (PathBuf, String);

#[derive(Clone, Default)]
pub struct FetchDeduplicator {
    in_flight: Arc<Mutex<HashSet<FetchKey>>>,
}

impl FetchDeduplicator {
    fn claim(&self, repo_path: &Path, remote: &str) -> Option<FetchClaim> {
        let key = (repo_path.to_path_buf(), remote.to_string());
        self.in_flight
            .lock()
            .unwrap()
            .insert(key.clone())
            .then(|| FetchClaim {
                key,
                in_flight: Arc::clone(&self.in_flight),
            })
    }
}

struct FetchClaim {
    key: FetchKey,
    in_flight: Arc<Mutex<HashSet<FetchKey>>>,
}

impl Drop for FetchClaim {
    fn drop(&mut self) {
        self.in_flight.lock().unwrap().remove(&self.key);
    }
}

#[derive(Clone)]
pub struct EventSender {
    tx: mpsc::Sender<AppEvent>,
    cancel: Arc<AtomicBool>,
}

impl EventSender {
    pub fn new(tx: mpsc::Sender<AppEvent>, cancel: Arc<AtomicBool>) -> Self {
        Self { tx, cancel }
    }

    pub fn send(&self, event: AppEvent) -> bool {
        !self.is_cancelled() && self.tx.send(event).is_ok()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Run built work items on a bounded Rayon pool, with a bounded OS-thread
/// fallback if Rayon cannot initialize.
pub fn spawn_work_parallel<T, F, W>(
    pool_size: usize,
    items: impl IntoIterator<Item = T>,
    build_work: F,
) where
    F: Fn(T) -> W,
    W: FnOnce() + Send + 'static,
{
    assert!(pool_size > 0, "worker pool size must be non-zero");
    let pool = ThreadPoolBuilder::new().num_threads(pool_size).build().ok();
    if let Some(pool) = &pool {
        for item in items {
            pool.spawn(build_work(item));
        }
    } else {
        let work = items.into_iter().map(build_work).collect::<VecDeque<_>>();
        let worker_count = pool_size.min(work.len());
        let work = Arc::new(Mutex::new(work));
        for _ in 0..worker_count {
            let work = Arc::clone(&work);
            thread::spawn(move || {
                loop {
                    let Some(job) = work.lock().unwrap().pop_front() else {
                        break;
                    };
                    job();
                }
            });
        }
    }
}

pub fn spawn_repo_discovery(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    search_dirs: Vec<ResolvedSearchDir>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.is_cancelled() {
            return;
        }

        thread::scope(|scope| {
            for search_dir in &search_dirs {
                let git = &git;
                let sender = &sender;
                scope.spawn(move || {
                    if sender.is_cancelled() {
                        return;
                    }
                    let result = git.scan_repos_streaming(
                        &search_dir.path,
                        search_dir.depth,
                        search_dir.include_non_git,
                        &|| sender.is_cancelled(),
                        &|repo| {
                            if sender.is_cancelled() {
                                return;
                            }
                            sender.send(AppEvent::ReposFound { repo });
                        },
                    );
                    match result {
                        Ok(warnings) => {
                            for warning in warnings {
                                sender.send(AppEvent::ScanWarning(warning));
                            }
                        }
                        Err(error) => {
                            sender.send(AppEvent::ScanWarning(ScanWarning {
                                path: search_dir.path.clone(),
                                message: format!("repository scan failed: {error:#}"),
                            }));
                        }
                    }
                });
            }
        });

        sender.send(AppEvent::ScanComplete);
    });
}

pub fn spawn_workspace_list(provider: &Arc<dyn HerdrProvider>, sender: &EventSender) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || match provider.workspace_list() {
        Ok(workspaces) => {
            sender.send(AppEvent::OpenWorkspacesLoaded { workspaces });
        }
        Err(error) => {
            sender.send(AppEvent::OpenWorkspacesFailed(format!(
                "could not load open workspace indicators: {error}"
            )));
        }
    });
}

pub fn spawn_open_folder_panes(provider: &Arc<dyn HerdrProvider>, sender: &EventSender) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || match provider.pane_list() {
        Ok(panes) => {
            sender.send(AppEvent::OpenFolderPanesLoaded { panes });
        }
        Err(error) => {
            sender.send(AppEvent::OpenFolderPanesFailed(format!(
                "could not load open folder indicators: {error}"
            )));
        }
    });
}

pub fn spawn_open_repo(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    on_open: OnOpenConfig,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        match provider.worktree_open(
            &repo_path,
            &WorktreeOpenTarget::Path(repo_path.clone()),
            true,
        ) {
            Ok(response) => {
                let on_open_warning = response
                    .opened
                    .as_ref()
                    .filter(|_| response.already_open == Some(false))
                    .and_then(|opened| {
                        apply_on_open(
                            provider.as_ref(),
                            &on_open,
                            &opened.root_pane_id,
                            &opened.path,
                        )
                    });
                let warning = combine_warnings(response.warning, on_open_warning);
                sender.send(AppEvent::RepoOpened { warning });
            }
            Err(error) => {
                sender.send(AppEvent::RepoOpenFailed(error.to_string()));
            }
        }
    });
}

pub fn spawn_open_folder(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    folder_path: PathBuf,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        let target = crate::path::canonical_or_original(&folder_path);
        let result = (|| {
            let panes = provider.pane_list()?;
            if let Some(workspace_id) = panes.iter().find_map(|pane| {
                pane.cwd.as_deref().and_then(|cwd| {
                    let cwd = crate::path::canonical_or_original(Path::new(cwd));
                    crate::path::equivalent(&cwd, &target).then(|| pane.workspace_id.clone())
                })
            }) {
                provider.workspace_focus(&workspace_id)?;
                Ok(None)
            } else {
                provider
                    .workspace_create(&target, true)
                    .map(|response| response.warning)
            }
        })();
        match result {
            Ok(warning) => {
                sender.send(AppEvent::RepoOpened { warning });
            }
            Err(error) => {
                sender.send(AppEvent::RepoOpenFailed(error.to_string()));
            }
        }
    });
}

pub fn spawn_branch_loading(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    mut repo: Repo,
    cwd: Option<PathBuf>,
    generation: u64,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let repo_path = repo.path.clone();
        let result = (|| {
            let local_names = git.list_branches(&repo.path)?;
            let default_branch = git.default_branch(&repo.path, &local_names.items)?;
            let worktrees = git.list_worktrees(&repo.path)?;
            repo.worktrees = worktrees.items;
            let branches = BranchEntry::build_local(
                &repo,
                &local_names.items,
                default_branch.as_deref(),
                cwd.as_deref(),
            );
            Ok::<_, anyhow::Error>((
                branches,
                std::mem::take(&mut repo.worktrees),
                local_names.skipped_unsupported_refs || worktrees.skipped_unsupported_refs,
            ))
        })();

        match result {
            Ok((branches, worktrees, skipped_unsupported_refs)) => {
                sender.send(AppEvent::BranchesLoaded {
                    repo_path,
                    generation,
                    branches,
                    worktrees,
                    skipped_unsupported_refs,
                });
            }
            Err(error) => {
                sender.send(AppEvent::BranchLoadFailed {
                    repo_path,
                    generation,
                    message: format!("could not load branches: {error:#}"),
                });
            }
        }
    });
}

pub fn spawn_remote_branch_loading(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    local_names: Vec<String>,
    generation: u64,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let remotes = match git.list_remotes(&repo_path) {
            Ok(remotes) => remotes,
            Err(error) => {
                sender.send(AppEvent::RemoteBranchLoadFailed {
                    repo_path,
                    generation,
                    message: format!("could not list remote branches: {error:#}"),
                });
                return;
            }
        };
        for remote in remotes {
            if sender.is_cancelled() {
                return;
            }
            match git.list_remote_branches_for_remote(&repo_path, &remote) {
                Ok(remote_names) => {
                    sender.send(AppEvent::RemoteBranchesLoaded {
                        repo_path: repo_path.clone(),
                        generation,
                        branches: BranchEntry::build_remote(
                            &remote,
                            &remote_names.items,
                            &local_names,
                        ),
                        skipped_unsupported_refs: remote_names.skipped_unsupported_refs,
                        remote,
                    });
                }
                Err(error) => {
                    sender.send(AppEvent::RemoteBranchLoadFailed {
                        repo_path: repo_path.clone(),
                        generation,
                        message: format!("could not list branches for remote {remote}: {error:#}"),
                    });
                }
            }
        }
    });
}

pub fn spawn_git_fetch(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    deduplicator: &FetchDeduplicator,
    repo_path: PathBuf,
    local_names: Vec<String>,
    generation: u64,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    let deduplicator = deduplicator.clone();
    thread::spawn(move || {
        let remotes = match git.list_remotes(&repo_path) {
            Ok(remotes) => remotes,
            Err(error) => {
                sender.send(AppEvent::GitFetchCompleted {
                    remote: None,
                    branches: Vec::new(),
                    repo_path,
                    generation,
                    error: Some(format!("could not list remotes for fetch: {error:#}")),
                    is_final: true,
                    skipped_unsupported_refs: false,
                });
                return;
            }
        };
        let claimed = remotes
            .into_iter()
            .filter_map(|remote| {
                deduplicator
                    .claim(&repo_path, &remote)
                    .map(|claim| (remote, claim))
            })
            .collect::<Vec<_>>();
        if claimed.is_empty() {
            sender.send(AppEvent::GitFetchCompleted {
                remote: None,
                branches: Vec::new(),
                repo_path,
                generation,
                error: None,
                is_final: true,
                skipped_unsupported_refs: false,
            });
            return;
        }

        let remaining = Arc::new(AtomicUsize::new(claimed.len()));
        let local_names = Arc::new(local_names);
        spawn_work_parallel(FETCH_POOL_SIZE, claimed, |(remote, claim)| {
            let git = Arc::clone(&git);
            let sender = sender.clone();
            let repo_path = repo_path.clone();
            let local_names = Arc::clone(&local_names);
            let remaining = Arc::clone(&remaining);
            move || {
                let _claim = claim;
                let (branches, error, skipped_unsupported_refs) = if sender.is_cancelled() {
                    (Vec::new(), None, false)
                } else {
                    match git.fetch_remote(&repo_path, &remote) {
                        Ok(()) => match git.list_remote_branches_for_remote(&repo_path, &remote) {
                            Ok(remote_names) => (
                                BranchEntry::build_remote(
                                    &remote,
                                    &remote_names.items,
                                    &local_names,
                                ),
                                None,
                                remote_names.skipped_unsupported_refs,
                            ),
                            Err(error) => (
                                Vec::new(),
                                Some(format!(
                                    "fetch succeeded but branches could not be refreshed: {error:#}"
                                )),
                                false,
                            ),
                        },
                        Err(error) => (Vec::new(), Some(error.to_string()), false),
                    }
                };
                let is_final = remaining.fetch_sub(1, Ordering::AcqRel) == 1;
                sender.send(AppEvent::GitFetchCompleted {
                    remote: Some(remote),
                    branches,
                    repo_path,
                    generation,
                    error,
                    is_final,
                    skipped_unsupported_refs,
                });
            }
        });
    });
}

pub fn spawn_open_worktrees(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    generation: u64,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || match provider.worktree_list(&repo_path) {
        Ok(response) => {
            sender.send(AppEvent::OpenWorktreesLoaded {
                repo_path,
                generation,
                worktrees: response.worktrees,
            });
        }
        Err(error) => {
            sender.send(AppEvent::OpenWorktreesFailed {
                repo_path,
                generation,
                message: format!("could not load open branch indicators: {error}"),
            });
        }
    });
}

pub fn spawn_open_branch(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    has_worktree: bool,
    on_open: OnOpenConfig,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        let route = if has_worktree {
            BranchOpenRoute::Open
        } else {
            BranchOpenRoute::Create
        };
        let result = open_branch_with_retry(provider.as_ref(), &repo_path, &branch_name, route);

        match result {
            Ok((opened, run_on_open, response_warning)) => {
                let on_open_warning = if run_on_open {
                    opened.as_ref().and_then(|opened| {
                        apply_on_open(
                            provider.as_ref(),
                            &on_open,
                            &opened.root_pane_id,
                            &opened.path,
                        )
                    })
                } else {
                    None
                };
                let warning = combine_warnings(response_warning, on_open_warning);
                sender.send(AppEvent::RepoOpened { warning });
            }
            Err(error) => {
                sender.send(AppEvent::BranchOperationFailed {
                    repo_path,
                    failure: BranchOperationFailure::Failed(friendly_branch_error(&error)),
                });
            }
        }
    });
}

pub fn spawn_validate_branch_name(
    git: &Arc<dyn GitProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(
        move || match git.is_valid_branch_name(&repo_path, &branch_name) {
            Ok(valid) => {
                sender.send(AppEvent::BranchNameValidated {
                    repo_path,
                    branch_name,
                    valid,
                    error: None,
                });
            }
            Err(error) => {
                sender.send(AppEvent::BranchNameValidated {
                    repo_path,
                    branch_name,
                    valid: false,
                    error: Some(format!("could not validate branch name: {error:#}")),
                });
            }
        },
    );
}

pub fn spawn_create_new_branch(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    base: String,
    on_open: OnOpenConfig,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        match provider.worktree_create(&WorktreeCreateRequest {
            cwd: repo_path.clone(),
            branch: branch_name,
            base: Some(base),
            path: None,
            focus: true,
        }) {
            Ok(response) => {
                let on_open_warning = response.opened.as_ref().and_then(|opened| {
                    apply_on_open(
                        provider.as_ref(),
                        &on_open,
                        &opened.root_pane_id,
                        &opened.path,
                    )
                });
                let warning = combine_warnings(response.warning, on_open_warning);
                sender.send(AppEvent::RepoOpened { warning });
            }
            Err(error) => {
                sender.send(AppEvent::BranchOperationFailed {
                    repo_path,
                    failure: BranchOperationFailure::Failed(friendly_branch_error(&error)),
                });
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_worktree_removal(
    git: &Arc<dyn GitProvider>,
    herdr: Option<&Arc<dyn HerdrProvider>>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    worktree_path: PathBuf,
    force: bool,
) {
    let git = Arc::clone(git);
    let herdr = herdr.cloned();
    let sender = sender.clone();
    thread::spawn(move || {
        let fresh_workspace_id =
            fresh_open_workspace_id(herdr.as_deref(), &repo_path, &worktree_path);
        let outcome = match (fresh_workspace_id, herdr.as_ref()) {
            (Ok(Some(workspace_id)), Some(provider)) => {
                provider.worktree_remove(&workspace_id, force).map_or_else(
                    |error| removal_error(RemoveError::Herdr(error), force),
                    |response| WorktreeRemovalOutcome::Removed {
                        warning: response.warning,
                    },
                )
            }
            (Ok(Some(_)), None) => WorktreeRemovalOutcome::Failed(
                "could not determine fresh open checkout state because herdr is unavailable".into(),
            ),
            (Ok(None), _) => match git.remove_worktree(&repo_path, &worktree_path, force) {
                Ok(()) => WorktreeRemovalOutcome::Removed {
                    warning: git.prune_worktrees(&repo_path).err().map(|error| {
                        format!("checkout was removed, but git worktree prune failed: {error:#}")
                    }),
                },
                Err(error) => removal_error(RemoveError::Git(error), force),
            },
            (Err(error), _) => WorktreeRemovalOutcome::Failed(error),
        };
        sender.send(AppEvent::WorktreeRemovalFinished {
            repo_path,
            branch_name,
            worktree_path,
            outcome,
        });
    });
}

fn fresh_open_workspace_id(
    herdr: Option<&dyn HerdrProvider>,
    repo_path: &Path,
    worktree_path: &Path,
) -> Result<Option<String>, String> {
    let provider = herdr.ok_or_else(|| {
        "could not determine fresh open checkout state because herdr is unavailable".to_string()
    })?;
    let response = provider.worktree_list(repo_path).map_err(|error| {
        format!("could not refresh open checkout state before removal: {error}")
    })?;
    response
        .worktrees
        .into_iter()
        .find(|worktree| crate::path::equivalent(Path::new(&worktree.path), worktree_path))
        .map(|worktree| worktree.open_workspace_id)
        .ok_or_else(|| {
            format!(
                "could not confirm checkout state before removal because {} was not returned by herdr",
                crate::path::display(worktree_path)
            )
        })
}

fn removal_error(error: RemoveError, force: bool) -> WorktreeRemovalOutcome {
    match error {
        RemoveError::Herdr(HerdrError::DirtyWorktreeRequiresForce(_)) if !force => {
            WorktreeRemovalOutcome::DirtyRequiresForce
        }
        RemoveError::Git(error) if !force && is_dirty_worktree_requires_force(&error) => {
            WorktreeRemovalOutcome::DirtyRequiresForce
        }
        RemoveError::Herdr(error) => WorktreeRemovalOutcome::Failed(friendly_branch_error(&error)),
        RemoveError::Git(error) => WorktreeRemovalOutcome::Failed(format!("{error:#}")),
    }
}

enum RemoveError {
    Herdr(HerdrError),
    Git(anyhow::Error),
}

pub fn spawn_open_remote_branch(
    git: &Arc<dyn GitProvider>,
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
    branch_name: String,
    remote: String,
    on_open: OnOpenConfig,
) {
    let git = Arc::clone(git);
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        let tracking_created = match git.create_tracking_branch(&repo_path, &branch_name, &remote) {
            Ok(()) => true,
            Err(error) if is_local_branch_already_exists(&error) => false,
            Err(error) => {
                sender.send(AppEvent::BranchOperationFailed {
                    repo_path,
                    failure: BranchOperationFailure::Failed(error.to_string()),
                });
                return;
            }
        };

        let result = open_branch_with_retry(
            provider.as_ref(),
            &repo_path,
            &branch_name,
            BranchOpenRoute::Create,
        );
        match result {
            Ok((opened, run_on_open, response_warning)) => {
                let on_open_warning = run_on_open
                    .then(|| {
                        opened.as_ref().and_then(|opened| {
                            apply_on_open(
                                provider.as_ref(),
                                &on_open,
                                &opened.root_pane_id,
                                &opened.path,
                            )
                        })
                    })
                    .flatten();
                let warning = combine_warnings(response_warning, on_open_warning);
                sender.send(AppEvent::RepoOpened { warning });
            }
            Err(error) => {
                let message = friendly_branch_error(&error);
                let failure = BranchOperationFailure::LocalBranchAvailable {
                    branch_name,
                    tracking_created,
                    message,
                };
                sender.send(AppEvent::BranchOperationFailed { repo_path, failure });
            }
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchOpenRoute {
    Open,
    Create,
}

fn open_branch_with_retry(
    provider: &dyn HerdrProvider,
    repo_path: &Path,
    branch_name: &str,
    route: BranchOpenRoute,
) -> Result<(Option<OpenedWorktree>, bool, Option<String>), HerdrError> {
    match run_branch_route(provider, repo_path, branch_name, route) {
        Ok(opened) => Ok(opened),
        Err(error) if is_wrong_route_failure(route, &error) => {
            let refreshed_route = provider.worktree_list(repo_path).ok().map(|response| {
                if response
                    .worktrees
                    .iter()
                    .any(|worktree| worktree.branch.as_deref() == Some(branch_name))
                {
                    BranchOpenRoute::Open
                } else {
                    BranchOpenRoute::Create
                }
            });
            match refreshed_route.filter(|refreshed| *refreshed != route) {
                Some(refreshed) => run_branch_route(provider, repo_path, branch_name, refreshed),
                None => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn run_branch_route(
    provider: &dyn HerdrProvider,
    repo_path: &Path,
    branch_name: &str,
    route: BranchOpenRoute,
) -> Result<(Option<OpenedWorktree>, bool, Option<String>), HerdrError> {
    match route {
        BranchOpenRoute::Open => provider
            .worktree_open(
                repo_path,
                &WorktreeOpenTarget::Branch(branch_name.to_string()),
                true,
            )
            .map(|response| {
                let run_on_open = response.already_open == Some(false);
                (response.opened, run_on_open, response.warning)
            }),
        BranchOpenRoute::Create => provider
            .worktree_create(&WorktreeCreateRequest {
                cwd: repo_path.to_path_buf(),
                branch: branch_name.to_string(),
                base: None,
                path: None,
                focus: true,
            })
            .map(|response| (response.opened, true, response.warning)),
    }
}

fn is_wrong_route_failure(route: BranchOpenRoute, error: &HerdrError) -> bool {
    match (route, error) {
        (BranchOpenRoute::Create, HerdrError::WorktreeCreateFailed(message)) => {
            message.to_ascii_lowercase().contains("already checked out")
        }
        (BranchOpenRoute::Open, HerdrError::WorktreeNotFound(_)) => true,
        (BranchOpenRoute::Open, HerdrError::WorktreeOpenFailed(message)) => {
            let message = message.to_ascii_lowercase();
            message.contains("not found") || message.contains("no worktree")
        }
        _ => false,
    }
}

fn apply_on_open(
    provider: &dyn HerdrProvider,
    on_open: &OnOpenConfig,
    root_pane_id: &str,
    checkout_path: &str,
) -> Option<String> {
    let mut errors = Vec::new();
    for (index, pane) in on_open.panes.iter().enumerate() {
        let split = provider.pane_split(&PaneSplitRequest {
            pane_id: root_pane_id.into(),
            direction: pane.direction,
            ratio: pane.ratio.map(|ratio| 1.0 - ratio),
            cwd: PathBuf::from(checkout_path),
            focus: false,
        });
        match split {
            Ok(response) => {
                if let Err(error) = provider.pane_run(&response.pane_id, &pane.command) {
                    errors.push(format!("pane {} run failed: {error}", index + 1));
                }
            }
            Err(error) => errors.push(format!("pane {} split failed: {error}", index + 1)),
        }
    }
    (!errors.is_empty()).then(|| format!("on_open: {}", errors.join("; ")))
}

fn combine_warnings(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(warning), None) | (None, Some(warning)) => Some(warning),
        (None, None) => None,
    }
}

fn friendly_branch_error(error: &HerdrError) -> String {
    match error {
        HerdrError::WorktreeOperationInProgress(_) => {
            "Another worktree operation is already in progress; wait for it to finish and try again."
                .into()
        }
        _ => error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use crate::{
        config::{OnOpenPaneConfig, OnOpenPaneDirection},
        git::{Repo, Worktree, mock::MockGitProvider},
        herdr::{
            OpenedWorktree, PaneRunResponse, PaneSplitResponse, WorktreeInfo, WorktreeListResponse,
            WorktreeOpenResponse,
            mock::{HerdrCall, MockHerdrProvider},
        },
    };

    use super::*;

    #[test]
    fn parallel_work_runs_every_item() {
        let (tx, rx) = mpsc::channel();
        spawn_work_parallel(2, 0..6, |item| {
            let tx = tx.clone();
            move || tx.send(item).unwrap()
        });
        drop(tx);
        let mut received: Vec<_> = rx.iter().collect();
        received.sort_unstable();
        assert_eq!(received, (0..6).collect::<Vec<_>>());
    }

    #[test]
    fn event_sender_stops_after_cancellation() {
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        assert!(sender.send(AppEvent::ReposFound {
            repo: Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: Vec::new(),
            },
        }));
        sender.cancel();
        assert!(!sender.send(AppEvent::ScanWarning(ScanWarning {
            path: PathBuf::new(),
            message: "cancelled".into(),
        })));
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn repo_discovery_streams_stubs_without_listing_worktrees() {
        let git_mock = Arc::new(MockGitProvider {
            repos: vec![Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: vec![Worktree {
                    path: "/repo".into(),
                    branch: Some("main".into()),
                }],
            }],
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_repo_discovery(
            &git,
            &sender,
            vec![ResolvedSearchDir {
                path: "/search".into(),
                depth: 1,
                include_non_git: false,
            }],
        );

        let AppEvent::ReposFound { repo } = rx.recv_timeout(Duration::from_secs(1)).unwrap() else {
            panic!("repository was not streamed")
        };
        assert_eq!(repo.path, Path::new("/repo"));
        assert!(repo.worktrees.is_empty());
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::ScanComplete
        ));
        assert!(git_mock.list_worktree_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn branch_loading_uses_a_fresh_worktree_listing() {
        let git_mock = Arc::new(MockGitProvider {
            branches: vec!["main".into(), "feature".into()],
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
            default_branch: Some("main".into()),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_branch_loading(
            &git,
            &sender,
            Repo {
                name: "repo".into(),
                path: "/repo".into(),
                is_git: true,
                worktrees: Vec::new(),
            },
            Some("/repo-feature/src".into()),
            7,
        );

        let AppEvent::BranchesLoaded {
            repo_path,
            generation,
            branches,
            worktrees,
            ..
        } = rx.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("branches were not loaded")
        };
        assert_eq!(repo_path, Path::new("/repo"));
        assert_eq!(generation, 7);
        assert_eq!(worktrees.len(), 2);
        let feature = branches
            .iter()
            .find(|branch| branch.name == "feature")
            .unwrap();
        assert_eq!(
            feature.worktree_path.as_deref(),
            Some(Path::new("/repo-feature"))
        );
        assert!(feature.is_current);
        assert_eq!(
            *git_mock.list_worktree_calls.lock().unwrap(),
            [PathBuf::from("/repo")]
        );
    }

    fn on_open_config() -> OnOpenConfig {
        OnOpenConfig {
            panes: vec![
                OnOpenPaneConfig {
                    command: "hx".into(),
                    direction: OnOpenPaneDirection::Right,
                    ratio: Some(0.4),
                },
                OnOpenPaneConfig {
                    command: "cargo test".into(),
                    direction: OnOpenPaneDirection::Down,
                    ratio: None,
                },
            ],
        }
    }

    #[test]
    fn on_open_splits_then_runs_each_configured_pane_in_order() {
        let mock = MockHerdrProvider::default();
        mock.pane_split_results
            .lock()
            .unwrap()
            .extend(["p_2", "p_3"].map(|pane_id| {
                Ok(PaneSplitResponse {
                    pane_id: pane_id.into(),
                })
            }));
        mock.pane_run_results
            .lock()
            .unwrap()
            .extend([Ok(PaneRunResponse), Ok(PaneRunResponse)]);

        assert!(apply_on_open(&mock, &on_open_config(), "p_root", "/repo").is_none());
        assert_eq!(
            *mock.calls.lock().unwrap(),
            [
                HerdrCall::PaneSplit(PaneSplitRequest {
                    pane_id: "p_root".into(),
                    direction: OnOpenPaneDirection::Right,
                    ratio: Some(0.6),
                    cwd: "/repo".into(),
                    focus: false,
                }),
                HerdrCall::PaneRun {
                    pane_id: "p_2".into(),
                    command: "hx".into(),
                },
                HerdrCall::PaneSplit(PaneSplitRequest {
                    pane_id: "p_root".into(),
                    direction: OnOpenPaneDirection::Down,
                    ratio: None,
                    cwd: "/repo".into(),
                    focus: false,
                }),
                HerdrCall::PaneRun {
                    pane_id: "p_3".into(),
                    command: "cargo test".into(),
                },
            ]
        );
    }

    #[test]
    fn on_open_ratio_is_the_new_pane_fraction() {
        let mock = MockHerdrProvider::default();
        mock.pane_split_results
            .lock()
            .unwrap()
            .push_back(Ok(PaneSplitResponse {
                pane_id: "p_2".into(),
            }));
        mock.pane_run_results
            .lock()
            .unwrap()
            .push_back(Ok(PaneRunResponse));
        let config = OnOpenConfig {
            panes: vec![OnOpenPaneConfig {
                command: "hx".into(),
                direction: OnOpenPaneDirection::Right,
                ratio: Some(0.35),
            }],
        };

        assert!(apply_on_open(&mock, &config, "p_root", "/repo").is_none());
        let calls = mock.calls.lock().unwrap();
        let HerdrCall::PaneSplit(request) = &calls[0] else {
            panic!("expected pane split call");
        };
        let ratio = request.ratio.expect("expected configured ratio");
        assert!((ratio - 0.65).abs() < f32::EPSILON);
    }

    fn worktree_open_response(already_open: bool) -> WorktreeOpenResponse {
        WorktreeOpenResponse {
            opened: Some(OpenedWorktree {
                root_pane_id: "p_root".into(),
                path: "/repo".into(),
            }),
            already_open: Some(already_open),
            warning: None,
        }
    }

    fn queue_successful_on_open(mock: &MockHerdrProvider) {
        mock.pane_split_results
            .lock()
            .unwrap()
            .extend(["p_2", "p_3"].map(|pane_id| {
                Ok(PaneSplitResponse {
                    pane_id: pane_id.into(),
                })
            }));
        mock.pane_run_results
            .lock()
            .unwrap()
            .extend([Ok(PaneRunResponse), Ok(PaneRunResponse)]);
    }

    fn pane_call_count(mock: &MockHerdrProvider) -> usize {
        mock.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| matches!(call, HerdrCall::PaneSplit(_) | HerdrCall::PaneRun { .. }))
            .count()
    }

    #[test]
    fn repo_open_applies_on_open_only_for_a_new_workspace() {
        for already_open in [false, true] {
            let mock = Arc::new(MockHerdrProvider::default());
            mock.worktree_open_results
                .lock()
                .unwrap()
                .push_back(Ok(worktree_open_response(already_open)));
            if !already_open {
                queue_successful_on_open(&mock);
            }
            let provider: Arc<dyn HerdrProvider> = mock.clone();
            let (tx, rx) = mpsc::channel();
            let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

            spawn_open_repo(&provider, &sender, "/repo".into(), on_open_config());

            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                AppEvent::RepoOpened { warning: None }
            ));
            assert_eq!(
                pane_call_count(&mock),
                if already_open { 0 } else { 4 },
                "already_open={already_open}"
            );
        }
    }

    #[test]
    fn branch_open_applies_on_open_only_for_a_new_workspace() {
        for already_open in [false, true] {
            let mock = Arc::new(MockHerdrProvider::default());
            mock.worktree_open_results
                .lock()
                .unwrap()
                .push_back(Ok(worktree_open_response(already_open)));
            if !already_open {
                queue_successful_on_open(&mock);
            }
            let provider: Arc<dyn HerdrProvider> = mock.clone();
            let (tx, rx) = mpsc::channel();
            let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

            spawn_open_branch(
                &provider,
                &sender,
                "/repo".into(),
                "main".into(),
                true,
                on_open_config(),
            );

            assert!(matches!(
                rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                AppEvent::RepoOpened { warning: None }
            ));
            assert_eq!(
                pane_call_count(&mock),
                if already_open { 0 } else { 4 },
                "already_open={already_open}"
            );
        }
    }

    #[test]
    fn on_open_failures_warn_without_replacing_open_success() {
        let mock = Arc::new(MockHerdrProvider::default());
        mock.worktree_open_results
            .lock()
            .unwrap()
            .push_back(Ok(worktree_open_response(false)));
        mock.pane_split_results.lock().unwrap().extend([
            Err(HerdrError::Other {
                code: "pane_split_failed".into(),
                message: "too small".into(),
            }),
            Ok(PaneSplitResponse {
                pane_id: "p_3".into(),
            }),
        ]);
        mock.pane_run_results
            .lock()
            .unwrap()
            .push_back(Err(HerdrError::Other {
                code: "pane_run_failed".into(),
                message: "shell closed".into(),
            }));
        let provider: Arc<dyn HerdrProvider> = mock;
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_open_repo(&provider, &sender, "/repo".into(), on_open_config());

        let AppEvent::RepoOpened {
            warning: Some(warning),
        } = rx.recv_timeout(Duration::from_secs(1)).unwrap()
        else {
            panic!("workspace open did not report success with a warning")
        };
        assert!(warning.starts_with("on_open: "));
        assert!(warning.contains("pane 1 split failed"));
        assert!(warning.contains("pane 2 run failed"));
    }

    #[test]
    fn remote_branch_loading_streams_each_remote_with_repo_scope() {
        let git = Arc::new(MockGitProvider {
            remotes: vec!["origin".into(), "upstream".into()],
            remote_branches_by_remote: HashMap::from([
                ("origin".into(), vec!["main".into(), "one".into()]),
                ("upstream".into(), vec!["two".into()]),
            ]),
            ..MockGitProvider::default()
        }) as Arc<dyn GitProvider>;
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_remote_branch_loading(&git, &sender, "/repo".into(), vec!["main".into()], 7);

        let events = [
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert!(matches!(
            &events[0],
            AppEvent::RemoteBranchesLoaded { repo_path, generation, remote, branches, .. }
                if repo_path == Path::new("/repo")
                    && *generation == 7
                    && remote == "origin"
                    && branches.iter().map(|branch| branch.name.as_str()).eq(["one"])
        ));
        assert!(matches!(
            &events[1],
            AppEvent::RemoteBranchesLoaded { repo_path, generation, remote, branches, .. }
                if repo_path == Path::new("/repo")
                    && *generation == 7
                    && remote == "upstream"
                    && branches.iter().map(|branch| branch.name.as_str()).eq(["two"])
        ));
    }

    #[test]
    fn git_fetch_streams_completions_and_marks_exactly_one_final() {
        let git_mock = Arc::new(MockGitProvider {
            remotes: vec!["origin".into(), "upstream".into()],
            remote_branches_by_remote: HashMap::from([
                ("origin".into(), vec!["one".into()]),
                ("upstream".into(), vec!["two".into()]),
            ]),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_git_fetch(
            &git,
            &sender,
            &FetchDeduplicator::default(),
            "/repo".into(),
            Vec::new(),
            7,
        );

        let events = [
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AppEvent::GitFetchCompleted { is_final: true, .. }))
                .count(),
            1
        );
        let mut calls = git_mock.fetch_calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls,
            [
                (PathBuf::from("/repo"), "origin".into()),
                (PathBuf::from("/repo"), "upstream".into()),
            ]
        );
    }

    #[test]
    fn rapid_reentry_does_not_start_a_duplicate_in_flight_fetch() {
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let git_mock = Arc::new(MockGitProvider {
            remotes: vec!["origin".into()],
            fetch_gate: Some(Arc::clone(&gate)),
            ..MockGitProvider::default()
        });
        let git: Arc<dyn GitProvider> = git_mock.clone();
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));
        let deduplicator = FetchDeduplicator::default();

        spawn_git_fetch(&git, &sender, &deduplicator, "/repo".into(), Vec::new(), 7);
        let deadline = Instant::now() + Duration::from_secs(1);
        while git_mock.fetch_calls.lock().unwrap().is_empty() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!(git_mock.fetch_calls.lock().unwrap().len(), 1);

        spawn_git_fetch(&git, &sender, &deduplicator, "/repo".into(), Vec::new(), 8);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::GitFetchCompleted {
                generation: 8,
                remote: None,
                is_final: true,
                ..
            }
        ));
        assert_eq!(git_mock.fetch_calls.lock().unwrap().len(), 1);

        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            AppEvent::GitFetchCompleted { generation: 7, .. }
        ));
    }

    #[test]
    fn prune_failure_after_removal_reports_success_with_warning() {
        let git_mock = Arc::new(MockGitProvider::default());
        *git_mock.prune_failure.lock().unwrap() = Some("prune broke".into());
        let git: Arc<dyn GitProvider> = git_mock;
        let herdr_mock = Arc::new(MockHerdrProvider::default());
        herdr_mock
            .worktree_list_results
            .lock()
            .unwrap()
            .push_back(Ok(WorktreeListResponse {
                worktrees: vec![WorktreeInfo {
                    path: "/repo-feature".into(),
                    branch: Some("feature".into()),
                    open_workspace_id: None,
                }],
            }));
        let herdr: Arc<dyn HerdrProvider> = herdr_mock;
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_worktree_removal(
            &git,
            Some(&herdr),
            &sender,
            "/repo".into(),
            "feature".into(),
            "/repo-feature".into(),
            false,
        );

        let event = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            event,
            AppEvent::WorktreeRemovalFinished {
                outcome: WorktreeRemovalOutcome::Removed { warning: Some(message) },
                ..
            } if message.contains("prune broke")
        ));
    }
}
