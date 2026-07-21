use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use rayon::ThreadPoolBuilder;

use crate::{
    event::AppEvent,
    git::{GitProvider, ScanWarning},
    herdr::{HerdrProvider, WorktreeOpenTarget},
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
            sender.send(AppEvent::HerdrError(format!(
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
                sender.send(AppEvent::HerdrError(error.to_string()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::git::Repo;

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
}
