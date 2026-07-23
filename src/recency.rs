use std::path::{Path, PathBuf};

#[cfg(test)]
use std::fs;

use serde::{Deserialize, Serialize};

use crate::{
    path::canonical_or_original,
    state::BranchId,
    state_store::{self, STATE_VERSION, VersionedState},
};

const FILE_NAME: &str = "recency.json";
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
        Self::Repo {
            path: canonical_or_original(path),
        }
    }

    pub fn branch(repo_path: &Path, branch: BranchId) -> Self {
        Self::Branch {
            repo_path: canonical_or_original(repo_path),
            branch,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecencyFile {
    version: u32,
    entries: Vec<RecencyKey>,
}

impl VersionedState for RecencyFile {
    fn version(&self) -> u32 {
        self.version
    }
}

#[derive(Debug, Clone, Default)]
pub struct RecencyStore {
    entries: Vec<RecencyKey>,
}

impl RecencyStore {
    pub fn load() -> Self {
        let Some(path) = state_store::state_path(FILE_NAME) else {
            return Self::default();
        };
        let (store, warning) = Self::load_from(&path);
        if let Some(warning) = warning {
            state_store::warn(warning);
        }
        store
    }

    pub fn repo_rank(&self, path: &Path) -> Option<usize> {
        self.rank(&RecencyKey::repo(path))
    }

    pub fn branch_rank(&self, repo_path: &Path, branch: &BranchId) -> Option<usize> {
        self.rank(&RecencyKey::branch(repo_path, branch.clone()))
    }

    fn rank(&self, key: &RecencyKey) -> Option<usize> {
        self.entries.iter().position(|entry| entry == key)
    }

    pub(crate) fn record(&mut self, key: RecencyKey) {
        self.entries.retain(|entry| entry != &key);
        self.entries.insert(0, key);
        self.entries.truncate(MAX_ENTRIES);
    }

    fn load_from(path: &Path) -> (Self, Option<String>) {
        let (file, warning) = state_store::load_from::<RecencyFile>(path, "recency state");
        let mut store = Self::default();
        for entry in file.entries.into_iter().rev() {
            store.record(entry);
        }
        (store, warning)
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        state_store::save_to(
            path,
            &RecencyFile {
                version: STATE_VERSION,
                entries: self.entries.clone(),
            },
        )
    }
}

pub fn record_success(key: RecencyKey) {
    let Some(path) = state_store::state_path(FILE_NAME) else {
        return;
    };
    let (mut store, warning) = RecencyStore::load_from(&path);
    if let Some(warning) = warning {
        state_store::warn(warning);
    }
    if let RecencyKey::Branch { repo_path, .. } = &key {
        store.record(RecencyKey::repo(repo_path));
    }
    store.record(key);
    if let Err(error) = store.save_to(&path) {
        state_store::warn(format!(
            "could not persist recency state at {}: {error}",
            path.display()
        ));
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn local(name: &str) -> BranchId {
        BranchId::Local(name.into())
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
    fn missing_and_corrupt_files_load_empty_without_panicking() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let (missing, missing_warning) = RecencyStore::load_from(&path);
        assert!(missing.entries.is_empty());
        assert!(missing_warning.unwrap().contains("could not be read"));

        fs::write(&path, b"{not json").unwrap();
        let (corrupt, corrupt_warning) = RecencyStore::load_from(&path);
        assert!(corrupt.entries.is_empty());
        assert!(corrupt_warning.unwrap().contains("is corrupt"));
    }

    #[test]
    fn saved_entries_round_trip_in_rank_order() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut store = RecencyStore::default();
        store.record(RecencyKey::repo(Path::new("/repos/alpha")));
        store.record(RecencyKey::branch(Path::new("/repos/alpha"), local("main")));
        store.save_to(&path).unwrap();

        let (loaded, warning) = RecencyStore::load_from(&path);

        assert!(warning.is_none());
        assert_eq!(
            loaded.branch_rank(Path::new("/repos/alpha"), &local("main")),
            Some(0)
        );
        assert_eq!(loaded.repo_rank(Path::new("/repos/alpha")), Some(1));
    }
}
