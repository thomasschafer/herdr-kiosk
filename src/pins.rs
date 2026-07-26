use std::{
    collections::HashSet,
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{config::ConfigWarning, recency::RecencyKey, state_store};

const FILE_NAME: &str = "pins.json";
const STATE_VERSION: u32 = 1;
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Serialize, Deserialize)]
struct PinFile {
    version: u32,
    entries: Vec<RecencyKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinOutcome {
    Pinned,
    Unpinned,
    AtCapacity,
}

#[derive(Debug)]
pub enum PinToggle {
    Applied {
        outcome: PinOutcome,
        warnings: Vec<ConfigWarning>,
    },
    Failed {
        warnings: Vec<ConfigWarning>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct PinStore {
    entries: HashSet<RecencyKey>,
    path: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct PinLoad {
    pub store: PinStore,
    pub warnings: Vec<ConfigWarning>,
}

impl PinStore {
    pub fn load() -> PinLoad {
        Self::load_with(|name| std::env::var(name).ok())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn repo_is_pinned_canonical(&self, path: &Path) -> bool {
        self.entries.contains(&RecencyKey::repo_canonical(path))
    }

    pub(crate) fn branch_is_pinned_canonical(
        &self,
        repo_path: &Path,
        branch: &crate::state::BranchId,
    ) -> bool {
        self.entries
            .contains(&RecencyKey::branch_canonical(repo_path, branch.clone()))
    }

    pub fn toggle(&mut self, key: RecencyKey) -> PinToggle {
        let Some(path) = self.path.clone() else {
            return PinToggle::Applied {
                outcome: self.apply_toggle(key),
                warnings: Vec::new(),
            };
        };
        match state_store::with_lock(&path, || {
            let (mut store, warnings) = Self::load_from(&path);
            let outcome = store.apply_toggle(key);
            if outcome != PinOutcome::AtCapacity {
                store.save_to(&path)?;
            }
            Ok((store, outcome, warnings))
        }) {
            Ok((mut store, outcome, warnings)) => {
                store.path = Some(path);
                *self = store;
                PinToggle::Applied { outcome, warnings }
            }
            Err(error) => PinToggle::Failed {
                warnings: vec![ConfigWarning {
                    message: format!("Could not persist pin state at {}: {error}", path.display()),
                }],
            },
        }
    }

    fn apply_toggle(&mut self, key: RecencyKey) -> PinOutcome {
        if self.entries.remove(&key) {
            PinOutcome::Unpinned
        } else if self.entries.len() >= MAX_ENTRIES {
            PinOutcome::AtCapacity
        } else {
            self.entries.insert(key);
            PinOutcome::Pinned
        }
    }

    fn load_with(get_env: impl Fn(&str) -> Option<String>) -> PinLoad {
        let resolution = state_store::resolve_state_path(FILE_NAME, get_env);
        let Some(path) = resolution.path else {
            let mut warnings = resolution.warnings;
            warnings.push(ConfigWarning {
                message:
                    "Pin state is unavailable because no trusted state directory could be resolved"
                        .into(),
            });
            return PinLoad {
                warnings,
                ..PinLoad::default()
            };
        };
        let (mut store, mut load_warnings) = Self::load_from(&path);
        store.path = Some(path);
        let mut warnings = resolution.warnings;
        warnings.append(&mut load_warnings);
        PinLoad { store, warnings }
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
        let file = match serde_json::from_slice::<PinFile>(&contents) {
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
        let mut entries = HashSet::new();
        for entry in file.entries {
            if entries.len() >= MAX_ENTRIES {
                break;
            }
            entries.insert(entry.normalized());
        }
        (
            Self {
                entries,
                path: None,
            },
            Vec::new(),
        )
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        let contents = serde_json::to_vec_pretty(&PinFile {
            version: STATE_VERSION,
            entries: self.entries.iter().cloned().collect(),
        })
        .map_err(io::Error::other)?;
        state_store::write_atomic(path, &contents)
    }
}

fn invalid_state(path: &Path, reason: &str) -> ConfigWarning {
    state_store::invalid_warning(path, "Pin state", reason, "an empty pin store was used")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        sync::{Arc, Barrier},
        thread,
    };

    use tempfile::tempdir;

    use super::*;

    fn store_at(path: &Path) -> PinStore {
        let (mut store, warnings) = PinStore::load_from(path);
        assert!(warnings.is_empty());
        store.path = Some(path.to_path_buf());
        store
    }

    fn outcome(toggle: PinToggle) -> PinOutcome {
        match toggle {
            PinToggle::Applied { outcome, warnings } => {
                assert!(warnings.is_empty());
                outcome
            }
            PinToggle::Failed { warnings } => panic!("toggle failed: {warnings:?}"),
        }
    }

    #[test]
    fn missing_and_corrupt_files_load_defensively() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let (missing, missing_warnings) = PinStore::load_from(&path);
        assert!(missing.is_empty());
        assert!(missing_warnings.is_empty());

        fs::write(&path, b"{not json").unwrap();
        let (corrupt, corrupt_warnings) = PinStore::load_from(&path);
        assert!(corrupt.is_empty());
        assert_eq!(corrupt_warnings.len(), 1);
        assert!(corrupt_warnings[0].message.contains("is corrupt"));
        assert!(!path.exists());
    }

    #[test]
    fn unwritable_state_path_is_non_fatal_and_reports_warnings() {
        let directory = tempdir().unwrap();
        let blocked = directory.path().join("not-a-directory");
        fs::write(&blocked, "file").unwrap();
        let values = HashMap::from([(
            "HERDR_PLUGIN_STATE_DIR",
            blocked.to_string_lossy().into_owned(),
        )]);

        let mut load = PinStore::load_with(|name| values.get(name).cloned());

        assert!(load.store.is_empty());
        assert_eq!(load.warnings.len(), 1);
        assert!(load.warnings[0].message.contains("could not be read"));
        match load
            .store
            .toggle(RecencyKey::repo(Path::new("/repos/alpha")))
        {
            PinToggle::Failed { warnings } => {
                assert_eq!(warnings.len(), 1);
                assert!(warnings[0].message.contains("Could not persist pin state"));
            }
            PinToggle::Applied { .. } => panic!("unwritable toggle unexpectedly succeeded"),
        }
    }

    #[test]
    fn loaded_entries_are_normalized_deduplicated_and_bounded() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let alpha = RecencyKey::repo(Path::new("/repos/alpha"));
        let entries = std::iter::once(alpha.clone())
            .chain(std::iter::once(alpha.clone()))
            .chain(
                (0..MAX_ENTRIES)
                    .map(|index| RecencyKey::repo(Path::new(&format!("/repos/{index}")))),
            )
            .collect();
        fs::write(
            &path,
            serde_json::to_vec(&PinFile {
                version: STATE_VERSION,
                entries,
            })
            .unwrap(),
        )
        .unwrap();

        let (store, warnings) = PinStore::load_from(&path);

        assert!(warnings.is_empty());
        assert_eq!(store.entries.len(), MAX_ENTRIES);
        assert!(store.entries.contains(&alpha));
    }

    #[test]
    fn toggle_outcomes_include_capacity_and_use_greater_than_or_equal_defensively() {
        let mut store = PinStore::default();
        for index in 0..MAX_ENTRIES {
            assert_eq!(
                outcome(store.toggle(RecencyKey::repo(Path::new(&format!("/repos/{index}"))))),
                PinOutcome::Pinned
            );
        }
        assert_eq!(
            outcome(store.toggle(RecencyKey::repo(Path::new("/repos/overflow")))),
            PinOutcome::AtCapacity
        );
        store
            .entries
            .insert(RecencyKey::repo(Path::new("/repos/excess")));
        assert_eq!(
            outcome(store.toggle(RecencyKey::repo(Path::new("/repos/another")))),
            PinOutcome::AtCapacity
        );
        assert_eq!(
            outcome(store.toggle(RecencyKey::repo(Path::new("/repos/0")))),
            PinOutcome::Unpinned
        );
    }

    #[test]
    fn locked_toggles_from_two_stores_do_not_lose_updates() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let barrier = Arc::new(Barrier::new(3));
        let handles = ["/repos/alpha", "/repos/beta"].map(|repo| {
            let mut store = store_at(&path);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                assert_eq!(
                    outcome(store.toggle(RecencyKey::repo(Path::new(repo)))),
                    PinOutcome::Pinned
                );
            })
        });
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        let (loaded, warnings) = PinStore::load_from(&path);
        assert!(warnings.is_empty());
        assert!(
            loaded
                .entries
                .contains(&RecencyKey::repo(Path::new("/repos/alpha")))
        );
        assert!(
            loaded
                .entries
                .contains(&RecencyKey::repo(Path::new("/repos/beta")))
        );
    }

    #[test]
    fn saves_replace_existing_state_with_a_complete_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut first = PinStore::default();
        first.apply_toggle(RecencyKey::repo(Path::new("/repos/old")));
        first.save_to(&path).unwrap();

        let mut replacement = PinStore::default();
        replacement.apply_toggle(RecencyKey::repo(Path::new("/repos/new")));
        replacement.save_to(&path).unwrap();

        let (loaded, warnings) = PinStore::load_from(&path);
        assert!(warnings.is_empty());
        assert!(
            !loaded
                .entries
                .contains(&RecencyKey::repo(Path::new("/repos/old")))
        );
        assert!(
            loaded
                .entries
                .contains(&RecencyKey::repo(Path::new("/repos/new")))
        );
        assert_eq!(
            fs::read_dir(directory.path())
                .unwrap()
                .filter_map(Result::ok)
                .count(),
            1
        );
    }
}
