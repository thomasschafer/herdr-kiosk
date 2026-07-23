use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::recency::RecencyKey;

const FILE_NAME: &str = "pins.json";
const STATE_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Serialize, Deserialize)]
struct PinFile {
    version: u32,
    entries: Vec<RecencyKey>,
}

#[derive(Debug, Clone, Default)]
pub struct PinStore {
    entries: Vec<RecencyKey>,
    path: Option<PathBuf>,
}

impl PinStore {
    pub fn load() -> Self {
        let Some(directory) = std::env::var_os("HERDR_PLUGIN_STATE_DIR")
            .filter(|directory| !directory.is_empty())
            .map(PathBuf::from)
        else {
            return Self::default();
        };
        if !directory.is_absolute() {
            warn(format!(
                "refusing relative state directory from HERDR_PLUGIN_STATE_DIR: {}",
                directory.display()
            ));
            return Self::default();
        }
        let path = directory.join(FILE_NAME);
        let (mut store, warning) = Self::load_from(&path);
        store.path = Some(path);
        if let Some(warning) = warning {
            warn(warning);
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
            warn(format!(
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
        let contents = match fs::read(path) {
            Ok(contents) => contents,
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!(
                        "pin state at {} could not be read: {error}; using an empty store",
                        path.display()
                    )),
                );
            }
        };
        let file = match serde_json::from_slice::<PinFile>(&contents) {
            Ok(file) if file.version == STATE_VERSION => file,
            Ok(file) => {
                return (
                    Self::default(),
                    Some(format!(
                        "pin state at {} uses unsupported version {} (expected {STATE_VERSION}); using an empty store",
                        path.display(),
                        file.version
                    )),
                );
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!(
                        "pin state at {} is corrupt: {error}; using an empty store",
                        path.display()
                    )),
                );
            }
        };
        let mut store = Self::default();
        for entry in file.entries {
            store.insert(entry);
        }
        (store, None)
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        let contents = serde_json::to_vec_pretty(&PinFile {
            version: STATE_VERSION,
            entries: self.entries.clone(),
        })
        .map_err(io::Error::other)?;
        fs::write(path, contents)
    }
}

fn warn(message: impl AsRef<str>) {
    eprintln!("herdr-kiosk: warning: {}", message.as_ref());
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
