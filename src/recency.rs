use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    config::ConfigWarning,
    path::{canonical_or_original, normalized_key},
    state::BranchId,
    state_store,
};

const FILE_NAME: &str = "recency.json";
const STATE_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecencyKey {
    Repo {
        path: PathBuf,
    },
    Branch {
        repo_path: PathBuf,
        branch: BranchId,
    },
}

impl RecencyKey {
    pub fn repo(path: &Path) -> Self {
        Self::repo_canonical(&canonical_or_original(path))
    }

    pub fn branch(repo_path: &Path, branch: BranchId) -> Self {
        Self::branch_canonical(&canonical_or_original(repo_path), branch)
    }

    pub(crate) fn repo_canonical(path: &Path) -> Self {
        Self::Repo {
            path: normalized_key(path),
        }
    }

    pub(crate) fn branch_canonical(repo_path: &Path, branch: BranchId) -> Self {
        Self::Branch {
            repo_path: normalized_key(repo_path),
            branch,
        }
    }

    pub(crate) fn normalized(self) -> Self {
        match self {
            Self::Repo { path } => Self::repo(&path),
            Self::Branch { repo_path, branch } => Self::branch(&repo_path, branch),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct RecencyFile {
    version: u32,
    entries: Vec<RecencyKey>,
}

#[derive(Debug, Clone, Default)]
pub struct RecencyStore {
    entries: Vec<RecencyKey>,
    ranks: HashMap<RecencyKey, usize>,
}

#[derive(Debug, Default)]
pub struct RecencyLoad {
    pub store: RecencyStore,
    pub warnings: Vec<ConfigWarning>,
}

impl RecencyStore {
    pub fn load() -> RecencyLoad {
        Self::load_with(|name| std::env::var(name).ok())
    }

    pub fn repo_rank(&self, path: &Path) -> Option<usize> {
        self.repo_rank_canonical(&canonical_or_original(path))
    }

    pub(crate) fn repo_rank_canonical(&self, path: &Path) -> Option<usize> {
        self.rank(&RecencyKey::repo_canonical(path))
    }

    pub fn branch_rank(&self, repo_path: &Path, branch: &BranchId) -> Option<usize> {
        self.branch_rank_canonical(&canonical_or_original(repo_path), branch)
    }

    pub(crate) fn branch_rank_canonical(
        &self,
        repo_path: &Path,
        branch: &BranchId,
    ) -> Option<usize> {
        self.rank(&RecencyKey::branch_canonical(repo_path, branch.clone()))
    }

    fn rank(&self, key: &RecencyKey) -> Option<usize> {
        self.ranks.get(key).copied()
    }

    pub(crate) fn record(&mut self, key: RecencyKey) {
        self.entries.retain(|entry| entry != &key);
        self.entries.insert(0, key);
        self.entries.truncate(MAX_ENTRIES);
        self.rebuild_ranks();
    }

    fn rebuild_ranks(&mut self) {
        self.ranks = self
            .entries
            .iter()
            .cloned()
            .enumerate()
            .map(|(rank, key)| (key, rank))
            .collect();
    }

    fn load_with(get_env: impl Fn(&str) -> Option<String>) -> RecencyLoad {
        let resolution = state_store::resolve_state_path(FILE_NAME, get_env);
        let Some(path) = resolution.path else {
            let mut warnings = resolution.warnings;
            warnings.push(ConfigWarning {
                message: "Recency state is unavailable because no trusted state directory could be resolved"
                    .into(),
            });
            return RecencyLoad {
                warnings,
                ..RecencyLoad::default()
            };
        };
        let (store, mut load_warnings) = Self::load_from(&path);
        let mut warnings = resolution.warnings;
        warnings.append(&mut load_warnings);
        RecencyLoad { store, warnings }
    }

    fn load_from(path: &Path) -> (Self, Vec<ConfigWarning>) {
        let contents = match state_store::read(path) {
            Ok(Some(contents)) => contents,
            Ok(None) => return (Self::default(), Vec::new()),
            Err(error) => {
                return (
                    Self::default(),
                    vec![invalid_state(path, &format!("could not be read: {error}"))],
                );
            }
        };
        let file = match serde_json::from_slice::<RecencyFile>(&contents) {
            Ok(file) if file.version == STATE_VERSION => file,
            Ok(file) => {
                return (
                    Self::default(),
                    vec![invalid_state(
                        path,
                        &format!(
                            "uses unsupported version {} (expected {STATE_VERSION})",
                            file.version
                        ),
                    )],
                );
            }
            Err(error) => {
                return (
                    Self::default(),
                    vec![invalid_state(path, &format!("is corrupt: {error}"))],
                );
            }
        };
        let mut store = Self::default();
        for entry in file.entries.into_iter().rev() {
            store.record(entry.normalized());
        }
        (store, Vec::new())
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        let contents = serde_json::to_vec_pretty(&RecencyFile {
            version: STATE_VERSION,
            entries: self.entries.clone(),
        })
        .map_err(io::Error::other)?;
        state_store::write_atomic(path, &contents)
    }
}

pub fn record_success(key: RecencyKey) -> Option<String> {
    let warnings = record_success_with(key, |name| std::env::var(name).ok());
    (!warnings.is_empty()).then(|| {
        warnings
            .into_iter()
            .map(|warning| warning.message)
            .collect::<Vec<_>>()
            .join("; ")
    })
}

fn record_success_with(
    key: RecencyKey,
    get_env: impl Fn(&str) -> Option<String>,
) -> Vec<ConfigWarning> {
    let resolution = state_store::resolve_state_path(FILE_NAME, get_env);
    let Some(path) = resolution.path else {
        let mut warnings = resolution.warnings;
        warnings.push(ConfigWarning {
            message: "Could not persist recency state because no trusted state directory could be resolved"
                .into(),
        });
        return warnings;
    };
    let mut warnings = resolution.warnings;
    match state_store::with_lock(&path, || {
        let (mut store, load_warnings) = RecencyStore::load_from(&path);
        if let RecencyKey::Branch { repo_path, .. } = &key {
            store.record(RecencyKey::repo_canonical(repo_path));
        }
        store.record(key);
        store.save_to(&path)?;
        Ok(load_warnings)
    }) {
        Ok(mut persist_warnings) => warnings.append(&mut persist_warnings),
        Err(error) => warnings.push(ConfigWarning {
            message: format!(
                "Could not persist recency state at {}: {error}",
                path.display()
            ),
        }),
    }
    warnings
}

fn invalid_state(path: &Path, reason: &str) -> ConfigWarning {
    state_store::invalid_warning(
        path,
        "Recency state",
        reason,
        "an empty recency store was used",
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;

    use super::*;

    fn local(name: &str) -> BranchId {
        BranchId::Local(name.into())
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn recording_deduplicates_and_moves_entries_to_the_front() {
        let mut store = RecencyStore::default();
        let alpha = RecencyKey::repo(Path::new("/repos/alpha"));
        let beta = RecencyKey::repo(Path::new("/repos/beta"));

        store.record(alpha.clone());
        store.record(beta.clone());
        store.record(alpha.clone());

        assert_eq!(store.rank(&alpha), Some(0));
        assert_eq!(store.rank(&beta), Some(1));
        assert_eq!(store.entries.len(), 2);
    }

    #[test]
    fn recording_evicts_the_oldest_entry_at_the_bound() {
        let mut store = RecencyStore::default();
        for index in 0..=MAX_ENTRIES {
            store.record(RecencyKey::repo(Path::new(&format!("/repos/{index}"))));
        }

        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert_eq!(
            store.repo_rank(Path::new(&format!("/repos/{MAX_ENTRIES}"))),
            Some(0)
        );
        assert_eq!(store.repo_rank(Path::new("/repos/0")), None);
    }

    #[test]
    fn repo_and_branch_ranks_use_distinct_typed_keys() {
        let mut store = RecencyStore::default();
        let repo = Path::new("/repos/alpha");
        store.record(RecencyKey::repo(repo));
        store.record(RecencyKey::branch(repo, local("main")));

        assert_eq!(store.branch_rank(repo, &local("main")), Some(0));
        assert_eq!(store.repo_rank(repo), Some(1));
        assert_eq!(store.branch_rank(repo, &local("other")), None);
    }

    #[test]
    fn missing_file_loads_empty_without_a_warning_and_corrupt_file_is_quarantined() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let (missing, missing_warnings) = RecencyStore::load_from(&path);
        assert!(missing.entries.is_empty());
        assert!(missing_warnings.is_empty());

        std::fs::write(&path, b"{not json").unwrap();
        let (corrupt, corrupt_warnings) = RecencyStore::load_from(&path);
        assert!(corrupt.entries.is_empty());
        assert_eq!(corrupt_warnings.len(), 1);
        assert!(corrupt_warnings[0].message.contains("is corrupt"));
        assert!(!path.exists());
    }

    #[test]
    fn saved_entries_round_trip_in_rank_order() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut store = RecencyStore::default();
        store.record(RecencyKey::repo(Path::new("/repos/alpha")));
        store.record(RecencyKey::branch(Path::new("/repos/alpha"), local("main")));
        store.save_to(&path).unwrap();

        let (loaded, warnings) = RecencyStore::load_from(&path);

        assert!(warnings.is_empty());
        assert_eq!(
            loaded.branch_rank(Path::new("/repos/alpha"), &local("main")),
            Some(0)
        );
        assert_eq!(loaded.repo_rank(Path::new("/repos/alpha")), Some(1));
    }

    #[test]
    fn record_success_resolves_fallback_and_round_trips_companion_repo() {
        let directory = tempdir().unwrap();
        let values = HashMap::from([("XDG_STATE_HOME", path_string(directory.path()))]);
        let repo = directory.path().join("repo");
        let warnings = record_success_with(RecencyKey::branch(&repo, local("main")), |name| {
            values.get(name).cloned()
        });
        assert!(warnings.is_empty());

        let path = directory.path().join("herdr-kiosk").join(FILE_NAME);
        let (loaded, warnings) = RecencyStore::load_from(&path);
        assert!(warnings.is_empty());
        assert_eq!(loaded.branch_rank(&repo, &local("main")), Some(0));
        assert_eq!(loaded.repo_rank(&repo), Some(1));
    }

    #[test]
    fn record_success_reports_an_unwritable_state_path() {
        let directory = tempdir().unwrap();
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, "file").unwrap();
        let values = HashMap::from([("HERDR_PLUGIN_STATE_DIR", path_string(&blocked))]);

        let warnings = record_success_with(RecencyKey::repo(Path::new("/repo")), |name| {
            values.get(name).cloned()
        });

        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0]
                .message
                .contains("Could not persist recency state")
        );
    }

    #[test]
    fn concurrent_record_success_calls_preserve_both_updates() {
        let directory = tempdir().unwrap();
        let state_dir = path_string(directory.path());
        let first_dir = state_dir.clone();
        let first = std::thread::spawn(move || {
            record_success_with(RecencyKey::repo(Path::new("/repo/first")), |name| {
                (name == "HERDR_PLUGIN_STATE_DIR").then(|| first_dir.clone())
            })
        });
        let second = std::thread::spawn(move || {
            record_success_with(RecencyKey::repo(Path::new("/repo/second")), |name| {
                (name == "HERDR_PLUGIN_STATE_DIR").then(|| state_dir.clone())
            })
        });
        assert!(first.join().unwrap().is_empty());
        assert!(second.join().unwrap().is_empty());

        let (loaded, warnings) = RecencyStore::load_from(&directory.path().join(FILE_NAME));
        assert!(warnings.is_empty());
        assert!(loaded.repo_rank(Path::new("/repo/first")).is_some());
        assert!(loaded.repo_rank(Path::new("/repo/second")).is_some());
    }
}
