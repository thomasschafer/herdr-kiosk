use std::path::{Path, PathBuf};

#[cfg(test)]
use std::fs;

use serde::{Deserialize, Serialize};

use crate::{
    recency::RecencyKey,
    state_store::{self, STATE_VERSION, VersionedState},
};

const FILE_NAME: &str = "pins.json";
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Default, Serialize, Deserialize)]
struct PinFile {
    version: u32,
    entries: Vec<RecencyKey>,
}

impl VersionedState for PinFile {
    fn version(&self) -> u32 {
        self.version
    }
}

#[derive(Debug, Clone, Default)]
pub struct PinStore {
    entries: Vec<RecencyKey>,
    path: Option<PathBuf>,
}

impl PinStore {
    pub fn load() -> Self {
        let Some(path) = state_store::state_path(FILE_NAME) else {
            return Self::default();
        };
        let (mut store, warning) = Self::load_from(&path);
        store.path = Some(path);
        if let Some(warning) = warning {
            state_store::warn(warning);
        }
        store
    }

    pub fn contains(&self, key: &RecencyKey) -> bool {
        self.entries.contains(key)
    }

    pub fn repo_is_pinned(&self, path: &Path) -> bool {
        self.contains(&RecencyKey::repo(path))
    }

    pub fn branch_is_pinned(&self, repo_path: &Path, branch: &crate::state::BranchId) -> bool {
        self.contains(&RecencyKey::branch(repo_path, branch.clone()))
    }

    pub fn toggle(&mut self, key: RecencyKey) -> bool {
        let pinned = if self.contains(&key) {
            self.entries.retain(|entry| entry != &key);
            false
        } else {
            self.insert(key)
        };
        if let Some(path) = self.path.as_deref()
            && let Err(error) = self.save_to(path)
        {
            state_store::warn(format!(
                "could not persist pin state at {}: {error}",
                path.display()
            ));
        }
        pinned
    }

    fn insert(&mut self, key: RecencyKey) -> bool {
        if self.entries.contains(&key) || self.entries.len() == MAX_ENTRIES {
            return false;
        }
        self.entries.push(key);
        true
    }

    pub(crate) fn load_from(path: &Path) -> (Self, Option<String>) {
        let (file, warning) = state_store::load_from::<PinFile>(path, "pin state");
        let mut store = Self::default();
        for entry in file.entries {
            store.insert(entry);
        }
        (store, warning)
    }

    fn save_to(&self, path: &Path) -> std::io::Result<()> {
        state_store::save_to(
            path,
            &PinFile {
                version: STATE_VERSION,
                entries: self.entries.clone(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn missing_and_corrupt_files_load_empty_without_panicking() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let (missing, missing_warning) = PinStore::load_from(&path);
        assert!(missing.entries.is_empty());
        assert!(missing_warning.unwrap().contains("could not be read"));

        fs::write(&path, b"{not json").unwrap();
        let (corrupt, corrupt_warning) = PinStore::load_from(&path);
        assert!(corrupt.entries.is_empty());
        assert!(corrupt_warning.unwrap().contains("is corrupt"));
    }

    #[test]
    fn toggles_persist_and_loaded_entries_are_deduplicated() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let alpha = RecencyKey::repo(Path::new("/repos/alpha"));
        let duplicate_file = PinFile {
            version: STATE_VERSION,
            entries: vec![alpha.clone(), alpha.clone()],
        };
        fs::write(&path, serde_json::to_vec(&duplicate_file).unwrap()).unwrap();

        let (mut store, warning) = PinStore::load_from(&path);
        store.path = Some(path.clone());
        assert!(warning.is_none());
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0], alpha);

        assert!(!store.toggle(alpha.clone()));
        let (loaded, warning) = PinStore::load_from(&path);
        assert!(warning.is_none());
        assert!(!loaded.contains(&alpha));

        assert!(store.toggle(alpha.clone()));
        let (loaded, warning) = PinStore::load_from(&path);
        assert!(warning.is_none());
        assert!(loaded.contains(&alpha));
    }

    #[test]
    fn loading_caps_unique_entries() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let file = PinFile {
            version: STATE_VERSION,
            entries: (0..=MAX_ENTRIES)
                .map(|index| RecencyKey::repo(Path::new(&format!("/repos/{index}"))))
                .collect(),
        };
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let (store, warning) = PinStore::load_from(&path);

        assert!(warning.is_none());
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert!(store.repo_is_pinned(Path::new("/repos/0")));
        assert!(!store.repo_is_pinned(Path::new(&format!("/repos/{MAX_ENTRIES}"))));
    }
}
