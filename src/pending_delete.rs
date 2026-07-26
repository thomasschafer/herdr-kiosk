use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    config::ConfigWarning,
    state_store::{self, StatePathResolution},
};

const FILE_NAME: &str = "pending_deletes.toml";
const STATE_VERSION: u32 = 1;
const TTL_SECS: u64 = 60 * 60 * 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingWorktreeDelete {
    pub repo_path: PathBuf,
    pub branch_name: String,
    pub worktree_path: PathBuf,
    #[serde(default)]
    pub force: bool,
    pub started_at_unix_secs: u64,
}

impl PendingWorktreeDelete {
    pub fn new(repo_path: PathBuf, branch_name: String, worktree_path: PathBuf) -> Self {
        Self {
            repo_path,
            branch_name,
            worktree_path,
            force: false,
            started_at_unix_secs: now_unix_secs(),
        }
    }

    pub fn is_expired(&self) -> bool {
        now_unix_secs().saturating_sub(self.started_at_unix_secs) > TTL_SECS
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PendingDeleteFile {
    version: u32,
    entries: Vec<PendingWorktreeDelete>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PendingDeleteLoad {
    pub entries: Vec<PendingWorktreeDelete>,
    pub warnings: Vec<ConfigWarning>,
}

pub(crate) fn resolve_state_path(get_env: impl Fn(&str) -> Option<String>) -> StatePathResolution {
    state_store::resolve_state_path(FILE_NAME, get_env)
}

pub fn load_pending_worktree_deletes() -> PendingDeleteLoad {
    let resolution = resolve_state_path(|name| std::env::var(name).ok());
    let Some(path) = resolution.path else {
        return PendingDeleteLoad {
            warnings: resolution.warnings,
            ..PendingDeleteLoad::default()
        };
    };
    let mut loaded = load_from(&path);
    loaded.warnings.splice(0..0, resolution.warnings);
    loaded
}

pub fn save_pending_worktree_deletes(entries: &[PendingWorktreeDelete]) -> Result<()> {
    let path = resolve_state_path(|name| std::env::var(name).ok())
        .path
        .ok_or_else(|| anyhow::anyhow!("no trusted pending-delete state path is available"))?;
    save_to(&path, entries)
}

fn load_from(path: &Path) -> PendingDeleteLoad {
    let contents = match state_store::read(path) {
        Ok(Some(contents)) => contents,
        Ok(None) => return PendingDeleteLoad::default(),
        Err(error) => return invalid_state(path, &format!("could not be read: {error}")),
    };
    let parsed = match std::str::from_utf8(&contents)
        .map_err(|error| error.to_string())
        .and_then(|contents| {
            toml::from_str::<PendingDeleteFile>(contents).map_err(|error| error.to_string())
        }) {
        Ok(parsed) => parsed,
        Err(error) => return invalid_state(path, &format!("is malformed: {error}")),
    };
    if parsed.version != STATE_VERSION {
        return invalid_state(
            path,
            &format!(
                "uses unsupported version {} (expected {STATE_VERSION})",
                parsed.version
            ),
        );
    }
    PendingDeleteLoad {
        entries: parsed
            .entries
            .into_iter()
            .filter(|entry| !entry.is_expired())
            .collect(),
        warnings: Vec::new(),
    }
}

fn invalid_state(path: &Path, reason: &str) -> PendingDeleteLoad {
    PendingDeleteLoad {
        entries: Vec::new(),
        warnings: vec![state_store::invalid_warning(
            path,
            "Pending-delete state",
            reason,
            "no deletions were resumed",
        )],
    }
}

fn save_to(path: &Path, entries: &[PendingWorktreeDelete]) -> Result<()> {
    if entries.is_empty() {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to remove pending-delete state"),
        }
        return Ok(());
    }
    let encoded = toml::to_string(&PendingDeleteFile {
        version: STATE_VERSION,
        entries: entries.to_vec(),
    })?;
    state_store::write_atomic(path, encoded.as_bytes())?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;

    use super::*;

    fn absolute_test_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join("herdr-kiosk-state-tests")
            .join(name)
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn pending_delete_round_trips_and_empty_save_removes_file() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("state/pending.toml");
        let entry = PendingWorktreeDelete::new("/repo".into(), "dev".into(), "/repo-dev".into());
        save_to(&path, std::slice::from_ref(&entry)).unwrap();
        assert_eq!(load_from(&path).entries, [entry]);
        save_to(&path, &[]).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn expired_entries_are_ignored() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("pending.toml");
        let entry = PendingWorktreeDelete {
            repo_path: "/repo".into(),
            branch_name: "dev".into(),
            worktree_path: "/repo-dev".into(),
            force: false,
            started_at_unix_secs: 0,
        };
        save_to(&path, &[entry]).unwrap();
        assert!(load_from(&path).entries.is_empty());
    }

    #[test]
    fn malformed_state_warns_and_is_quarantined_without_resuming_deletions() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("pending.toml");
        fs::write(&path, "not = [valid toml").unwrap();

        let loaded = load_from(&path);

        assert!(loaded.entries.is_empty());
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].message.contains("is malformed"));
        assert!(
            loaded.warnings[0]
                .message
                .contains("no deletions were resumed")
        );
        assert!(!path.exists());
        assert!(fs::read_dir(temp.path()).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("pending.toml.invalid.")
        }));
    }

    #[test]
    fn state_path_uses_plugin_directory_before_xdg_and_home() {
        let plugin = absolute_test_path("plugin-state");
        let xdg_path = absolute_test_path("xdg-state");
        let home_path = absolute_test_path("home");
        let values = HashMap::from([
            ("HERDR_PLUGIN_STATE_DIR", path_string(&plugin)),
            ("XDG_STATE_HOME", path_string(&xdg_path)),
            ("HOME", path_string(&home_path)),
        ]);
        let resolution = resolve_state_path(|name| values.get(name).cloned());
        assert_eq!(resolution.path, Some(plugin.join("pending_deletes.toml")));
        assert!(resolution.warnings.is_empty());

        let xdg = HashMap::from([
            ("XDG_STATE_HOME", path_string(&xdg_path)),
            ("HOME", path_string(&home_path)),
        ]);
        assert_eq!(
            resolve_state_path(|name| xdg.get(name).cloned()).path,
            Some(xdg_path.join("herdr-kiosk/pending_deletes.toml"))
        );

        let home = HashMap::from([("HOME", path_string(&home_path))]);
        assert_eq!(
            resolve_state_path(|name| home.get(name).cloned()).path,
            Some(home_path.join(".local/state/herdr-kiosk/pending_deletes.toml"))
        );
    }

    #[test]
    fn relative_state_paths_are_refused_and_fall_through() {
        let xdg_path = absolute_test_path("xdg-fallback");
        let home_path = absolute_test_path("home-fallback");
        let values = HashMap::from([
            ("HERDR_PLUGIN_STATE_DIR", "plugin-state".to_string()),
            ("XDG_STATE_HOME", path_string(&xdg_path)),
            ("HOME", path_string(&home_path)),
        ]);
        let resolution = resolve_state_path(|name| values.get(name).cloned());
        assert_eq!(
            resolution.path,
            Some(xdg_path.join("herdr-kiosk/pending_deletes.toml"))
        );
        assert_eq!(resolution.warnings.len(), 1);
        assert!(
            resolution.warnings[0]
                .message
                .contains("refusing relative state directory from HERDR_PLUGIN_STATE_DIR")
        );
    }
}
