use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{ConfigWarning, resolve_trusted_file_path};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatePathResolution {
    pub path: Option<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

pub(crate) fn resolve_state_path(get_env: impl Fn(&str) -> Option<String>) -> StatePathResolution {
    let candidates = [
        get_env("HERDR_PLUGIN_STATE_DIR")
            .filter(|value| !value.is_empty())
            .map(|value| ("HERDR_PLUGIN_STATE_DIR", PathBuf::from(value), false)),
        get_env("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(|value| ("XDG_STATE_HOME", PathBuf::from(value), true)),
        get_env("HOME")
            .filter(|value| !value.is_empty())
            .map(|value| ("HOME", PathBuf::from(value).join(".local/state"), true)),
    ];
    let (path, warnings) =
        resolve_trusted_file_path(candidates.into_iter().flatten(), FILE_NAME, "state");
    StatePathResolution { path, warnings }
}

pub fn load_pending_worktree_deletes() -> Vec<PendingWorktreeDelete> {
    let resolution = resolve_state_path(|name| std::env::var(name).ok());
    let Some(path) = resolution.path else {
        return Vec::new();
    };
    load_from(&path).unwrap_or_default()
}

pub fn save_pending_worktree_deletes(entries: &[PendingWorktreeDelete]) -> Result<()> {
    let path = resolve_state_path(|name| std::env::var(name).ok())
        .path
        .ok_or_else(|| anyhow::anyhow!("no trusted pending-delete state path is available"))?;
    save_to(&path, entries)
}

fn load_from(path: &Path) -> io::Result<Vec<PendingWorktreeDelete>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let Ok(parsed) = toml::from_str::<PendingDeleteFile>(&contents) else {
        return Ok(Vec::new());
    };
    if parsed.version != STATE_VERSION {
        return Ok(Vec::new());
    }
    Ok(parsed
        .entries
        .into_iter()
        .filter(|entry| !entry.is_expired())
        .collect())
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let encoded = toml::to_string(&PendingDeleteFile {
        version: STATE_VERSION,
        entries: entries.to_vec(),
    })?;
    fs::write(path, encoded)?;
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
        assert_eq!(load_from(&path).unwrap(), [entry]);
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
        assert!(load_from(&path).unwrap().is_empty());
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
