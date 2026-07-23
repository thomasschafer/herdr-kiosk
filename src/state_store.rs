use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Serialize, de::DeserializeOwned};

pub(crate) const STATE_VERSION: u32 = 1;

pub(crate) trait VersionedState: Default + Serialize + DeserializeOwned {
    fn version(&self) -> u32;
}

pub(crate) fn state_path(file_name: &str) -> Option<PathBuf> {
    let (directory, warning) = resolve_directory_from(std::env::var_os("HERDR_PLUGIN_STATE_DIR"));
    if let Some(warning) = warning {
        warn(warning);
    }
    directory.map(|directory| directory.join(file_name))
}

fn resolve_directory_from(value: Option<OsString>) -> (Option<PathBuf>, Option<String>) {
    let Some(directory) = value
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
    else {
        return (None, None);
    };
    if !directory.is_absolute() {
        return (
            None,
            Some(format!(
                "refusing relative state directory from HERDR_PLUGIN_STATE_DIR: {}",
                directory.display()
            )),
        );
    }
    (Some(directory), None)
}

pub(crate) fn load_from<T: VersionedState>(path: &Path, state_name: &str) -> (T, Option<String>) {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) => {
            return (
                T::default(),
                Some(format!(
                    "{state_name} at {} could not be read: {error}; using an empty store",
                    path.display()
                )),
            );
        }
    };
    match serde_json::from_slice::<T>(&contents) {
        Ok(file) if file.version() == STATE_VERSION => (file, None),
        Ok(file) => (
            T::default(),
            Some(format!(
                "{state_name} at {} uses unsupported version {} (expected {STATE_VERSION}); using an empty store",
                path.display(),
                file.version()
            )),
        ),
        Err(error) => (
            T::default(),
            Some(format!(
                "{state_name} at {} is corrupt: {error}; using an empty store",
                path.display()
            )),
        ),
    }
}

pub(crate) fn save_to<T: VersionedState>(path: &Path, file: &T) -> io::Result<()> {
    let contents = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_atomic(path, &contents)
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let (temporary_path, mut temporary_file) = create_temp_file(path)?;
    if let Err(error) = temporary_file
        .write_all(contents)
        .and_then(|()| temporary_file.sync_all())
    {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(temporary_file);
    if let Err(error) = replace_file_atomic(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    Ok(())
}

fn create_temp_file(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state.json");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..16_u8 {
        let temporary_path = path.with_file_name(format!(
            ".{file_name}.{}.{nonce}.{attempt}.tmp",
            std::process::id()
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary state file",
    ))
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
    // SAFETY: Both paths are valid, NUL-terminated UTF-16 buffers for the duration of the call.
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

pub(crate) fn warn(message: impl AsRef<str>) {
    eprintln!("herdr-kiosk: warning: {}", message.as_ref());
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use tempfile::tempdir;

    use super::*;

    #[derive(Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    struct TestFile {
        version: u32,
        entries: Vec<String>,
    }

    impl VersionedState for TestFile {
        fn version(&self) -> u32 {
            self.version
        }
    }

    #[test]
    fn directory_resolution_accepts_only_absolute_paths() {
        let absolute = std::env::temp_dir().join("herdr-kiosk-state");
        assert_eq!(
            resolve_directory_from(Some(absolute.clone().into_os_string())),
            (Some(absolute), None)
        );

        let (relative, warning) = resolve_directory_from(Some(OsString::from("relative-state")));
        assert!(relative.is_none());
        assert!(
            warning
                .unwrap()
                .contains("refusing relative state directory")
        );
        assert_eq!(resolve_directory_from(None), (None, None));
    }

    #[test]
    fn missing_and_corrupt_files_load_empty_without_panicking() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");

        let (missing, missing_warning) = load_from::<TestFile>(&path, "test state");
        assert_eq!(missing, TestFile::default());
        assert!(missing_warning.unwrap().contains("could not be read"));

        fs::write(&path, b"{not json").unwrap();
        let (corrupt, corrupt_warning) = load_from::<TestFile>(&path, "test state");
        assert_eq!(corrupt, TestFile::default());
        assert!(corrupt_warning.unwrap().contains("is corrupt"));
    }

    #[test]
    fn atomic_save_round_trips_and_replaces_existing_state() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("state.json");
        let first = TestFile {
            version: STATE_VERSION,
            entries: vec!["first".into()],
        };
        save_to(&path, &first).unwrap();

        let second = TestFile {
            version: STATE_VERSION,
            entries: vec!["second".into()],
        };
        save_to(&path, &second).unwrap();

        let (loaded, warning) = load_from::<TestFile>(&path, "test state");
        assert!(warning.is_none());
        assert_eq!(loaded, second);
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
