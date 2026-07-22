use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use rayon::ThreadPoolBuilder;

use crate::{
    event::{AppEvent, WorktreeRemovalOutcome},
    git::{
        GitProvider, Repo, ScanWarning, is_dirty_worktree_requires_force,
        is_local_branch_already_exists,
    },
    herdr::{HerdrError, HerdrProvider, WorktreeCreateRequest, WorktreeOpenTarget},
    state::BranchEntry,
};

/// Bounds concurrent `git worktree list` enrichment calls for large scans.
pub const ENRICHMENT_POOL_SIZE: usize = 8;
/// Bounds concurrent remote fetches.
pub const FETCH_POOL_SIZE: usize = 4;

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
    search_dirs: Vec<(PathBuf, u16)>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        if sender.is_cancelled() {
            return;
        }

        let enrichment_pool = ThreadPoolBuilder::new()
            .num_threads(ENRICHMENT_POOL_SIZE)
            .build()
            .map(Arc::new);
        if let Err(error) = &enrichment_pool {
            sender.send(AppEvent::ScanWarning(ScanWarning {
                path: PathBuf::new(),
                message: format!("failed to create repository enrichment pool: {error}"),
            }));
        }

        thread::scope(|scope| {
            for (dir, depth) in &search_dirs {
                let git = &git;
                let sender = &sender;
                let enrichment_pool = enrichment_pool.as_ref().ok().cloned();
                scope.spawn(move || {
                    if sender.is_cancelled() {
                        return;
                    }
                    let result = git.scan_repos_streaming(dir, *depth, &|repo| {
                        if sender.is_cancelled() {
                            return;
                        }
                        let repo_path = repo.path.clone();
                        sender.send(AppEvent::ReposFound { repo });
                        if let Some(pool) = enrichment_pool.as_ref() {
                            let git = Arc::clone(git);
                            let sender = sender.clone();
                            pool.spawn(move || match git.list_worktrees(&repo_path) {
                                Ok(worktrees) => {
                                    sender.send(AppEvent::RepoEnriched {
                                        repo_path,
                                        worktrees,
                                    });
                                }
                                Err(error) => {
                                    sender.send(AppEvent::ScanWarning(ScanWarning {
                                        path: repo_path,
                                        message: format!(
                                            "failed to enrich repository worktrees: {error:#}"
                                        ),
                                    }));
                                }
                            });
                        }
                    });
                    match result {
                        Ok(warnings) => {
                            for warning in warnings {
                                sender.send(AppEvent::ScanWarning(warning));
                            }
                        }
                        Err(error) => {
                            sender.send(AppEvent::ScanWarning(ScanWarning {
                                path: dir.clone(),
                                message: format!("repository scan failed: {error:#}"),
                            }));
                        }
                    }
                });
            }
        });

        sender.send(AppEvent::ScanComplete { search_dirs });
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

pub fn spawn_open_repo(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        match provider.worktree_open(
            &repo_path,
            &WorktreeOpenTarget::Path(repo_path.clone()),
            true,
        ) {
            Ok(_) => {
                sender.send(AppEvent::RepoOpened);
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
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let repo_path = repo.path.clone();
        let result = (|| {
            let local_names = git.list_branches(&repo.path)?;
            let default_branch = git.default_branch(&repo.path, &local_names)?;
            // This listing is deliberately fresh. Repo discovery enrichment may be
            // stale, and Enter relies on it for the open-vs-create decision.
            repo.worktrees = git.list_worktrees(&repo.path)?;
            let branches = BranchEntry::build_local(
                &repo,
                &local_names,
                default_branch.as_deref(),
                cwd.as_deref(),
            );
            Ok::<_, anyhow::Error>((branches, std::mem::take(&mut repo.worktrees)))
        })();

        match result {
            Ok((branches, worktrees)) => {
                sender.send(AppEvent::BranchesLoaded {
                    repo_path,
                    branches,
                    worktrees,
                });
            }
            Err(error) => {
                sender.send(AppEvent::BranchLoadFailed {
                    repo_path,
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
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let remotes = match git.list_remotes(&repo_path) {
            Ok(remotes) => remotes,
            Err(error) => {
                sender.send(AppEvent::RemoteBranchLoadFailed {
                    repo_path,
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
                        branches: BranchEntry::build_remote(&remote, &remote_names, &local_names),
                        remote,
                    });
                }
                Err(error) => {
                    sender.send(AppEvent::RemoteBranchLoadFailed {
                        repo_path: repo_path.clone(),
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
    repo_path: PathBuf,
    local_names: Vec<String>,
) {
    let git = Arc::clone(git);
    let sender = sender.clone();
    thread::spawn(move || {
        let remotes = match git.list_remotes(&repo_path) {
            Ok(remotes) => remotes,
            Err(error) => {
                sender.send(AppEvent::GitFetchCompleted {
                    remote: None,
                    branches: Vec::new(),
                    repo_path,
                    error: Some(format!("could not list remotes for fetch: {error:#}")),
                    is_final: true,
                });
                return;
            }
        };
        if remotes.is_empty() {
            sender.send(AppEvent::GitFetchCompleted {
                remote: None,
                branches: Vec::new(),
                repo_path,
                error: None,
                is_final: true,
            });
            return;
        }

        let remaining = Arc::new(AtomicUsize::new(remotes.len()));
        let local_names = Arc::new(local_names);
        spawn_work_parallel(FETCH_POOL_SIZE, remotes, |remote| {
            let git = Arc::clone(&git);
            let sender = sender.clone();
            let repo_path = repo_path.clone();
            let local_names = Arc::clone(&local_names);
            let remaining = Arc::clone(&remaining);
            move || {
                let (branches, error) = if sender.is_cancelled() {
                    (Vec::new(), None)
                } else {
                    match git.fetch_remote(&repo_path, &remote) {
                        Ok(()) => match git.list_remote_branches_for_remote(&repo_path, &remote) {
                            Ok(remote_names) => (
                                BranchEntry::build_remote(&remote, &remote_names, &local_names),
                                None,
                            ),
                            Err(error) => (
                                Vec::new(),
                                Some(format!(
                                    "fetch succeeded but branches could not be refreshed: {error:#}"
                                )),
                            ),
                        },
                        Err(error) => (Vec::new(), Some(error.to_string())),
                    }
                };
                let is_final = remaining.fetch_sub(1, Ordering::AcqRel) == 1;
                sender.send(AppEvent::GitFetchCompleted {
                    remote: Some(remote),
                    branches,
                    repo_path,
                    error,
                    is_final,
                });
            }
        });
    });
}

pub fn spawn_open_worktrees(
    provider: &Arc<dyn HerdrProvider>,
    sender: &EventSender,
    repo_path: PathBuf,
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || match provider.worktree_list(&repo_path) {
        Ok(response) => {
            sender.send(AppEvent::OpenWorktreesLoaded {
                repo_path,
                worktrees: response.worktrees,
            });
        }
        Err(error) => {
            sender.send(AppEvent::OpenWorktreesFailed {
                repo_path,
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
) {
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        let result = if has_worktree {
            provider
                .worktree_open(
                    &repo_path,
                    &WorktreeOpenTarget::Branch(branch_name.clone()),
                    true,
                )
                .map(|_| ())
        } else {
            provider
                .worktree_create(&WorktreeCreateRequest {
                    cwd: repo_path.clone(),
                    branch: branch_name,
                    base: None,
                    path: None,
                    focus: true,
                })
                .map(|_| ())
        };

        match result {
            Ok(()) => {
                sender.send(AppEvent::RepoOpened);
            }
            Err(error) => {
                sender.send(AppEvent::BranchOperationFailed {
                    repo_path,
                    message: friendly_branch_error(&error),
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
            Ok(_) => {
                sender.send(AppEvent::RepoOpened);
            }
            Err(error) => {
                sender.send(AppEvent::BranchOperationFailed {
                    repo_path,
                    message: friendly_branch_error(&error),
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
    open_workspace_id: Option<String>,
    force: bool,
) {
    let git = Arc::clone(git);
    let herdr = herdr.cloned();
    let sender = sender.clone();
    thread::spawn(move || {
        let outcome = if let Some(workspace_id) = open_workspace_id {
            match herdr {
                Some(provider) => provider
                    .worktree_remove(&workspace_id, force)
                    .map(|_| ())
                    .map_or_else(
                        |error| removal_error(RemoveError::Herdr(error), force),
                        |()| WorktreeRemovalOutcome::Removed { warning: None },
                    ),
                None => WorktreeRemovalOutcome::Failed("not running inside herdr".into()),
            }
        } else {
            match git.remove_worktree(&repo_path, &worktree_path, force) {
                Ok(()) => WorktreeRemovalOutcome::Removed {
                    warning: git.prune_worktrees(&repo_path).err().map(|error| {
                        format!("checkout was removed, but git worktree prune failed: {error:#}")
                    }),
                },
                Err(error) => removal_error(RemoveError::Git(error), force),
            }
        };
        sender.send(AppEvent::WorktreeRemovalFinished {
            repo_path,
            branch_name,
            worktree_path,
            outcome,
        });
    });
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
) {
    let git = Arc::clone(git);
    let provider = Arc::clone(provider);
    let sender = sender.clone();
    thread::spawn(move || {
        if let Err(error) = git.create_tracking_branch(&repo_path, &branch_name, &remote)
            && !is_local_branch_already_exists(&error)
        {
            sender.send(AppEvent::BranchOperationFailed {
                repo_path,
                message: error.to_string(),
            });
            return;
        }

        let result = provider
            .worktree_create(&WorktreeCreateRequest {
                cwd: repo_path.clone(),
                branch: branch_name,
                base: None,
                path: None,
                focus: true,
            })
            .map(|_| ());
        match result {
            Ok(()) => {
                sender.send(AppEvent::RepoOpened);
            }
            Err(error) => {
                sender.send(AppEvent::BranchOperationFailed {
                    repo_path,
                    message: friendly_branch_error(&error),
                });
            }
        }
    });
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
        time::Duration,
    };

    use crate::git::{Repo, mock::MockGitProvider};

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
                worktrees: Vec::new(),
            },
        }));
        sender.cancel();
        assert!(!sender.send(AppEvent::GitError("cancelled".into())));
        assert!(rx.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(rx.try_recv().is_err());
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

        spawn_remote_branch_loading(&git, &sender, "/repo".into(), vec!["main".into()]);

        let events = [
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        ];
        assert!(matches!(
            &events[0],
            AppEvent::RemoteBranchesLoaded { repo_path, remote, branches }
                if repo_path == Path::new("/repo")
                    && remote == "origin"
                    && branches.iter().map(|branch| branch.name.as_str()).eq(["one"])
        ));
        assert!(matches!(
            &events[1],
            AppEvent::RemoteBranchesLoaded { repo_path, remote, branches }
                if repo_path == Path::new("/repo")
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

        spawn_git_fetch(&git, &sender, "/repo".into(), Vec::new());

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
    fn prune_failure_after_removal_reports_success_with_warning() {
        let git_mock = Arc::new(MockGitProvider::default());
        *git_mock.prune_failure.lock().unwrap() = Some("prune broke".into());
        let git: Arc<dyn GitProvider> = git_mock;
        let (tx, rx) = mpsc::channel();
        let sender = EventSender::new(tx, Arc::new(AtomicBool::new(false)));

        spawn_worktree_removal(
            &git,
            None,
            &sender,
            "/repo".into(),
            "feature".into(),
            "/repo-feature".into(),
            None,
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
