use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::config::{ConfigWarning, resolve_trusted_file_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatePathResolution {
    pub path: Option<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

pub(crate) fn resolve_state_path(
    file_name: &str,
    get_env: impl Fn(&str) -> Option<String>,
) -> StatePathResolution {
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
        get_env("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(|value| ("LOCALAPPDATA", PathBuf::from(value), true)),
        get_env("USERPROFILE")
            .filter(|value| !value.is_empty())
            .map(|value| {
                (
                    "USERPROFILE",
                    PathBuf::from(value).join(".local/state"),
                    true,
                )
            }),
    ];
    let (path, warnings) =
        resolve_trusted_file_path(candidates.into_iter().flatten(), file_name, "state");
    StatePathResolution { path, warnings }
}

pub(crate) fn read(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn invalid_warning(
    path: &Path,
    subject: &str,
    reason: &str,
    consequence: &str,
) -> ConfigWarning {
    let disposition = match quarantine(path) {
        Ok(quarantined) => format!("quarantined as {}", quarantined.display()),
        Err(error) => format!("left in place because quarantine failed: {error}"),
    };
    ConfigWarning {
        message: format!(
            "{subject} at {} {reason}; it was {disposition}, and {consequence}",
            path.display()
        ),
    }
}

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    write_atomic_with(path, contents, replace_file_atomic)
}

pub(crate) fn with_lock<T>(path: &Path, action: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let lock_path = sibling_path(path, "lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    lock.lock()?;
    action()
}

fn quarantine(path: &Path) -> io::Result<PathBuf> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    let nonce = nonce();
    for attempt in 0..16_u8 {
        let quarantined = path.with_file_name(format!(
            "{file_name}.invalid.{}.{nonce}.{attempt}",
            std::process::id()
        ));
        if quarantined.exists() {
            continue;
        }
        fs::rename(path, &quarantined)?;
        return Ok(quarantined);
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate quarantine path",
    ))
}

fn write_atomic_with(
    path: &Path,
    contents: &[u8],
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let (temp_path, mut temp_file) = create_temp_file(path)?;
    if let Err(error) = temp_file
        .write_all(contents)
        .and_then(|()| temp_file.sync_all())
    {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    drop(temp_file);
    if let Err(error) = replace(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(())
}

fn create_temp_file(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    let nonce = nonce();
    for attempt in 0..16_u8 {
        let temp_path = sibling_path(
            path,
            &format!("{}.{nonce}.{attempt}.tmp", std::process::id()),
        );
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary state file",
    ))
}

fn sibling_path(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    path.with_file_name(format!(".{file_name}.{suffix}"))
}

fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(not(windows))]
fn replace_file_atomic(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

#[cfg(windows)]
fn replace_file_atomic(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, replacement: *const u16, flags: u32) -> i32;
    }

    let from = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both paths are valid, NUL-terminated UTF-16 buffers for the duration of the call.
    if unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn state_path_resolution_uses_each_supported_fallback_in_order() {
        let root = std::env::current_dir()
            .unwrap()
            .join("state-resolution-test");
        let plugin = root.join("plugin");
        let xdg = root.join("xdg");
        let home = root.join("home");
        let local_app_data = root.join("local-app-data");
        let user_profile = root.join("user-profile");
        let mut values = HashMap::from([
            ("HERDR_PLUGIN_STATE_DIR", path_string(&plugin)),
            ("XDG_STATE_HOME", path_string(&xdg)),
            ("HOME", path_string(&home)),
            ("LOCALAPPDATA", path_string(&local_app_data)),
            ("USERPROFILE", path_string(&user_profile)),
        ]);

        let resolve = |values: &HashMap<&str, String>| {
            resolve_state_path("state.json", |name| values.get(name).cloned()).path
        };
        assert_eq!(resolve(&values), Some(plugin.join("state.json")));

        values.remove("HERDR_PLUGIN_STATE_DIR");
        assert_eq!(resolve(&values), Some(xdg.join("herdr-kiosk/state.json")));

        values.remove("XDG_STATE_HOME");
        assert_eq!(
            resolve(&values),
            Some(home.join(".local/state/herdr-kiosk/state.json"))
        );

        values.remove("HOME");
        assert_eq!(
            resolve(&values),
            Some(local_app_data.join("herdr-kiosk/state.json"))
        );

        values.remove("LOCALAPPDATA");
        assert_eq!(
            resolve(&values),
            Some(user_profile.join(".local/state/herdr-kiosk/state.json"))
        );

        values.remove("USERPROFILE");
        assert_eq!(resolve(&values), None);
    }

    #[test]
    fn atomic_write_keeps_the_old_file_visible_until_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.json");
        fs::write(&path, "old complete contents").unwrap();

        write_atomic_with(&path, b"new complete contents", |temporary, target| {
            assert_eq!(fs::read_to_string(target).unwrap(), "old complete contents");
            assert_eq!(
                fs::read_to_string(temporary).unwrap(),
                "new complete contents"
            );
            replace_file_atomic(temporary, target)
        })
        .unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "new complete contents");
    }
}
