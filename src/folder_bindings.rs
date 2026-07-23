use std::path::{Path, PathBuf};

#[cfg(test)]
use std::fs;

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use crate::{
    path::{canonical_or_original, equivalent},
    state_store::{self, STATE_VERSION, VersionedState},
};

const FILE_NAME: &str = "folder_bindings.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct CanonicalFolder(PathBuf);

impl CanonicalFolder {
    fn new(path: &Path) -> Option<Self> {
        let path = canonical_or_original(path);
        path.is_absolute().then_some(Self(path))
    }
}

impl<'de> Deserialize<'de> for CanonicalFolder {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let path = PathBuf::deserialize(deserializer)?;
        Self::new(&path).ok_or_else(|| D::Error::custom("folder path must be absolute"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
struct WorkspaceId(String);

impl WorkspaceId {
    fn new(value: String) -> Option<Self> {
        (!value.trim().is_empty()).then_some(Self(value))
    }
}

impl<'de> Deserialize<'de> for WorkspaceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).ok_or_else(|| D::Error::custom("workspace id must not be empty"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FolderBinding {
    folder: CanonicalFolder,
    workspace_id: WorkspaceId,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FolderBindingFile {
    version: u32,
    bindings: Vec<FolderBinding>,
}

impl VersionedState for FolderBindingFile {
    fn version(&self) -> u32 {
        self.version
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct FolderBindings {
    bindings: Vec<FolderBinding>,
    path: Option<PathBuf>,
}

impl FolderBindings {
    pub(crate) fn load() -> Self {
        let Some(path) = state_store::state_path(FILE_NAME) else {
            return Self::default();
        };
        let (mut bindings, warning) = Self::load_from(&path);
        bindings.path = Some(path);
        if let Some(warning) = warning {
            state_store::warn(warning);
        }
        bindings
    }

    pub(crate) fn lookup(&self, folder: &Path) -> Option<&str> {
        let folder = CanonicalFolder::new(folder)?;
        self.bindings
            .iter()
            .find(|binding| equivalent(&binding.folder.0, &folder.0))
            .map(|binding| binding.workspace_id.0.as_str())
    }

    pub(crate) fn record(&mut self, folder: &Path, workspace_id: String) {
        let Some(folder) = CanonicalFolder::new(folder) else {
            return;
        };
        let Some(workspace_id) = WorkspaceId::new(workspace_id) else {
            return;
        };
        self.bindings
            .retain(|binding| !equivalent(&binding.folder.0, &folder.0));
        self.bindings.push(FolderBinding {
            folder,
            workspace_id,
        });
        self.persist();
    }

    pub(crate) fn remove(&mut self, folder: &Path) {
        let Some(folder) = CanonicalFolder::new(folder) else {
            return;
        };
        let previous_len = self.bindings.len();
        self.bindings
            .retain(|binding| !equivalent(&binding.folder.0, &folder.0));
        if self.bindings.len() != previous_len {
            self.persist();
        }
    }

    fn load_from(path: &Path) -> (Self, Option<String>) {
        let (file, warning) =
            state_store::load_from::<FolderBindingFile>(path, "folder-binding state");
        let mut store = Self::default();
        for binding in file.bindings {
            store.record_in_memory(binding);
        }
        (store, warning)
    }

    fn record_in_memory(&mut self, binding: FolderBinding) {
        self.bindings
            .retain(|entry| !equivalent(&entry.folder.0, &binding.folder.0));
        self.bindings.push(binding);
    }

    fn persist(&self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Err(error) = state_store::save_to(
            path,
            &FolderBindingFile {
                version: STATE_VERSION,
                bindings: self.bindings.clone(),
            },
        ) {
            state_store::warn(format!(
                "could not persist folder-binding state at {}: {error}",
                path.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn bindings_record_lookup_and_overwrite() {
        let directory = tempdir().unwrap();
        let folder = directory.path().join("folder");
        fs::create_dir(&folder).unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut bindings = FolderBindings {
            path: Some(path.clone()),
            ..FolderBindings::default()
        };

        bindings.record(&folder, "w_first".into());
        assert_eq!(bindings.lookup(&folder), Some("w_first"));
        bindings.record(&folder, "w_second".into());
        assert_eq!(bindings.lookup(&folder), Some("w_second"));

        let (loaded, warning) = FolderBindings::load_from(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.lookup(&folder), Some("w_second"));
        assert_eq!(loaded.bindings.len(), 1);
    }

    #[test]
    fn missing_and_corrupt_files_load_empty_without_panicking() {
        let directory = tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);

        let (missing, missing_warning) = FolderBindings::load_from(&path);
        assert!(missing.bindings.is_empty());
        assert!(missing_warning.unwrap().contains("could not be read"));

        fs::write(&path, b"{not json").unwrap();
        let (corrupt, corrupt_warning) = FolderBindings::load_from(&path);
        assert!(corrupt.bindings.is_empty());
        assert!(corrupt_warning.unwrap().contains("is corrupt"));
    }
}
