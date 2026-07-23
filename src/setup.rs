use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::{config::SearchDirEntry, state::TextInput};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupDir {
    pub path: String,
    pub depth: u16,
    pub include_non_git: Option<bool>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FolderMode {
    #[default]
    GitRepositoriesOnly,
    AllFolders,
}

impl FolderMode {
    const fn previous(self) -> Self {
        match self {
            Self::GitRepositoriesOnly => Self::AllFolders,
            Self::AllFolders => Self::GitRepositoriesOnly,
        }
    }

    const fn next(self) -> Self {
        self.previous()
    }

    const fn include_non_git(self) -> Option<bool> {
        match self {
            Self::GitRepositoriesOnly => None,
            Self::AllFolders => Some(true),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupStep {
    Welcome,
    Directories,
    Depth { path: String },
    FolderMode { path: String, depth: u16 },
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupState {
    pub step: SetupStep,
    pub input: TextInput,
    pub completions: Vec<String>,
    pub selected_completion: Option<usize>,
    pub dirs: Vec<SetupDir>,
    pub message: Option<String>,
    pub depth_default_pristine: bool,
    pub folder_mode: FolderMode,
}

impl Default for SetupState {
    fn default() -> Self {
        Self {
            step: SetupStep::Welcome,
            input: TextInput::default(),
            completions: Vec::new(),
            selected_completion: None,
            dirs: Vec::new(),
            message: None,
            depth_default_pristine: false,
            folder_mode: FolderMode::default(),
        }
    }
}

impl SetupState {
    pub fn continue_from_welcome(&mut self) {
        if self.step == SetupStep::Welcome {
            self.step = SetupStep::Directories;
        }
    }

    pub fn begin_depth(&mut self) -> Result<(), &'static str> {
        let path = normalize_search_dir(self.input.text.trim());
        if path.is_empty() {
            if self.dirs.is_empty() {
                return Err("Add at least one search directory");
            }
            self.step = SetupStep::Confirm;
            return Ok(());
        }
        self.step = SetupStep::Depth { path };
        self.folder_mode = FolderMode::default();
        self.input.text = "1".into();
        self.input.cursor = 1;
        self.depth_default_pristine = true;
        self.completions.clear();
        self.selected_completion = None;
        Ok(())
    }

    pub fn commit_depth(&mut self) -> Result<(), String> {
        let SetupStep::Depth { path } = &self.step else {
            return Err("not choosing a search depth".into());
        };
        let depth = self
            .input
            .text
            .parse::<u16>()
            .map_err(|_| "Depth must be a whole number".to_string())?;
        if depth == 0 {
            return Err("Depth must be at least 1".into());
        }
        self.step = SetupStep::FolderMode {
            path: path.clone(),
            depth,
        };
        self.message = None;
        self.depth_default_pristine = false;
        Ok(())
    }

    pub fn select_previous_folder_mode(&mut self) {
        self.folder_mode = self.folder_mode.previous();
    }

    pub fn select_next_folder_mode(&mut self) {
        self.folder_mode = self.folder_mode.next();
    }

    pub fn commit_folder_mode(&mut self) -> Result<(), &'static str> {
        let SetupStep::FolderMode { path, depth } = &self.step else {
            return Err("not choosing a folder inclusion mode");
        };
        let entry = SetupDir {
            path: path.clone(),
            depth: *depth,
            include_non_git: self.folder_mode.include_non_git(),
        };
        if let Some(existing) = self
            .dirs
            .iter_mut()
            .find(|entry| entry.path == path.as_str())
        {
            *existing = entry;
        } else {
            self.dirs.push(entry);
        }
        self.step = SetupStep::Directories;
        self.input.clear();
        self.message = None;
        Ok(())
    }

    pub fn cancel_folder_mode(&mut self) {
        let SetupStep::FolderMode { path, depth } = &self.step else {
            return;
        };
        let path = path.clone();
        let depth = depth.to_string();
        self.step = SetupStep::Depth { path };
        self.input.text = depth;
        self.input.cursor = self.input.text.len();
        self.message = None;
        self.depth_default_pristine = false;
    }

    pub fn cancel_depth(&mut self) {
        let SetupStep::Depth { path } = &self.step else {
            return;
        };
        let path = path.clone();
        self.step = SetupStep::Directories;
        self.input.text = path;
        self.input.cursor = self.input.text.len();
        self.message = None;
        self.depth_default_pristine = false;
    }

    pub fn remove_last(&mut self) -> Option<SetupDir> {
        self.dirs.pop()
    }

    pub fn update_completions(&mut self, home: Option<&Path>) {
        self.completions = complete_paths(&self.input.text, home);
        self.selected_completion = None;
    }

    pub fn tab_complete(&mut self, home: Option<&Path>) {
        if self.completions.is_empty() {
            self.update_completions(home);
        }
        if self.completions.is_empty() {
            return;
        }
        let common = common_prefix(&self.completions);
        if common.len() > self.input.text.len() {
            self.input.text = common;
            self.input.cursor = self.input.text.len();
            self.update_completions(home);
            return;
        }
        let selected = self
            .selected_completion
            .map_or(0, |index| (index + 1) % self.completions.len());
        self.selected_completion = Some(selected);
        self.input.text = self.completions[selected].clone();
        self.input.cursor = self.input.text.len();
    }

    pub fn search_dirs(&self) -> Vec<SearchDirEntry> {
        self.dirs
            .iter()
            .map(|entry| SearchDirEntry::Rich {
                path: entry.path.clone(),
                depth: Some(entry.depth),
                include_non_git: entry.include_non_git,
            })
            .collect()
    }
}

fn normalize_search_dir(path: &str) -> String {
    if is_root_path(path) {
        return path.to_string();
    }
    path.trim_end_matches(['/', '\\']).to_string()
}

fn is_root_path(path: &str) -> bool {
    if path.is_empty() || !path.ends_with(['/', '\\']) {
        return false;
    }
    let without_separators = path.trim_end_matches(['/', '\\']);
    if without_separators.is_empty() {
        return true;
    }
    let bytes = without_separators.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }
    if path.starts_with("//") || path.starts_with("\\\\") {
        return without_separators[2..]
            .split(['/', '\\'])
            .filter(|part| !part.is_empty())
            .count()
            == 2;
    }
    false
}

pub fn split_input(input: &str) -> (String, String) {
    input.rfind(['/', '\\']).map_or_else(
        || ("./".into(), input.into()),
        |slash| (input[..=slash].into(), input[slash + 1..].into()),
    )
}

fn expand_for_fs(path: &str, home: Option<&Path>) -> PathBuf {
    if path == "~" {
        return home.unwrap_or_else(|| Path::new(".")).to_path_buf();
    }
    path.strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\"))
        .map_or_else(
            || PathBuf::from(path),
            |rest| home.unwrap_or_else(|| Path::new(".")).join(rest),
        )
}

pub fn complete_paths(input: &str, home: Option<&Path>) -> Vec<String> {
    complete_paths_with(input, home, |parent| {
        fs::read_dir(parent).map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
    })
}

pub fn complete_paths_with(
    input: &str,
    home: Option<&Path>,
    read_directory_names: impl FnOnce(&Path) -> io::Result<Vec<String>>,
) -> Vec<String> {
    if input.is_empty() {
        return Vec::new();
    }
    let (parent, prefix) = split_input(input);
    let Ok(entries) = read_directory_names(&expand_for_fs(&parent, home)) else {
        return Vec::new();
    };
    let prefix_lower = prefix.to_lowercase();
    let mut matches = entries
        .into_iter()
        .filter_map(|name| {
            ((!name.starts_with('.') || prefix.starts_with('.'))
                && name.to_lowercase().starts_with(&prefix_lower))
            .then(|| format!("{parent}{name}"))
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches
}

pub fn common_prefix(values: &[String]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    values.iter().skip(1).fold(first.clone(), |prefix, value| {
        prefix
            .chars()
            .zip(value.chars())
            .take_while(|(left, right)| left == right)
            .map(|(character, _)| character)
            .collect()
    })
}

#[derive(Serialize)]
struct WrittenConfig<'a> {
    search_dirs: &'a [SearchDirEntry],
}

pub fn config_contents(search_dirs: &[SearchDirEntry]) -> Result<String> {
    toml::to_string_pretty(&WrittenConfig { search_dirs }).context("failed to encode config")
}

pub trait AtomicWriteFs {
    fn exists(&self, path: &Path) -> bool;
    fn create_parent_dirs(&self, path: &Path) -> io::Result<()>;
    fn write_new(&self, path: &Path, contents: &[u8]) -> io::Result<()>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
}

pub struct RealFs;

impl AtomicWriteFs for RealFs {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn create_parent_dirs(&self, path: &Path) -> io::Result<()> {
        path.parent().map_or(Ok(()), fs::create_dir_all)
    }

    fn write_new(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(contents)?;
        file.sync_all()
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        rename_no_replace(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both C strings live for the call and contain no interior NUL bytes.
    let result = unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both C strings live for the syscall and contain no interior NUL bytes.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    // A same-directory hard link is atomic and fails if the destination exists.
    // This preserves the setup writer's no-clobber guarantee without relying on
    // Windows rename replacement semantics.
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

pub fn write_config_atomic_with(
    fs: &impl AtomicWriteFs,
    path: &Path,
    contents: &str,
) -> Result<()> {
    if fs.exists(path) {
        bail!("refusing to overwrite existing config {}", path.display());
    }
    fs.create_parent_dirs(path)
        .with_context(|| format!("failed to create parent directory for {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = path.with_file_name(format!(".{file_name}.{}.{nonce}.tmp", std::process::id()));
    fs.write_new(&temp, contents.as_bytes())
        .with_context(|| format!("failed to write temporary config {}", temp.display()))?;
    if fs.exists(path) {
        let _ = fs.remove_file(&temp);
        bail!(
            "refusing to overwrite config created during setup: {}",
            path.display()
        );
    }
    if let Err(error) = fs.rename(&temp, path) {
        let _ = fs.remove_file(&temp);
        return Err(error).with_context(|| format!("failed to install config {}", path.display()));
    }
    Ok(())
}

pub fn write_config_atomic(path: &Path, search_dirs: &[SearchDirEntry]) -> Result<()> {
    write_config_atomic_with(&RealFs, path, &config_contents(search_dirs)?)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::HashMap};

    use super::*;

    #[test]
    fn state_machine_adds_updates_removes_and_confirms_directories() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "~/Code".into();
        state.begin_depth().unwrap();
        state.input.text = "3".into();
        state.commit_depth().unwrap();
        state.commit_folder_mode().unwrap();
        assert_eq!(state.dirs[0].depth, 3);
        state.input.text = "~/Code".into();
        state.begin_depth().unwrap();
        state.input.text = "2".into();
        state.commit_depth().unwrap();
        state.commit_folder_mode().unwrap();
        assert_eq!(
            state.dirs,
            [SetupDir {
                path: "~/Code".into(),
                depth: 2,
                include_non_git: None,
            }]
        );
        assert!(state.remove_last().is_some());
        assert!(state.dirs.is_empty());
        assert!(state.begin_depth().is_err());
    }

    #[test]
    fn cancelling_depth_restores_the_pending_path_for_editing() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "~/Code".into();
        state.begin_depth().unwrap();

        state.cancel_depth();

        assert_eq!(state.step, SetupStep::Directories);
        assert_eq!(state.input.text, "~/Code");
        assert_eq!(state.input.cursor, "~/Code".len());
        assert!(state.dirs.is_empty());
    }

    #[test]
    fn filesystem_root_is_preserved_when_beginning_depth() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/".into();

        state.begin_depth().unwrap();

        assert_eq!(state.step, SetupStep::Depth { path: "/".into() });
    }

    #[test]
    fn trailing_separator_is_trimmed_from_a_normal_search_path() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "/work/projects/".into();

        state.begin_depth().unwrap();

        assert_eq!(
            state.step,
            SetupStep::Depth {
                path: "/work/projects".into()
            }
        );
    }

    #[test]
    fn windows_and_unc_roots_are_preserved() {
        for root in [r"C:\", r"\\server\share\"] {
            assert_eq!(normalize_search_dir(root), root);
        }
    }

    #[test]
    fn completion_and_tilde_handling_use_the_filesystem() {
        let values = complete_paths_with("~/De", Some(Path::new("/home/tester")), |parent| {
            assert_eq!(parent, Path::new("/home/tester"));
            Ok(vec![
                "Development".into(),
                "Desktop".into(),
                ".hidden".into(),
            ])
        });
        assert_eq!(values, ["~/Desktop", "~/Development"]);
        assert_eq!(common_prefix(&values), "~/De");
    }

    #[test]
    fn path_input_accepts_windows_separators_and_preserves_the_typed_style() {
        assert_eq!(
            split_input(r"C:\\Users\\Tom\\Dev"),
            (r"C:\\Users\\Tom\\".into(), "Dev".into())
        );
        assert_eq!(
            split_input("C:/Users/Tom/Dev"),
            ("C:/Users/Tom/".into(), "Dev".into())
        );
        let values = complete_paths_with(r"~\De", Some(Path::new("/home/tester")), |parent| {
            assert_eq!(parent, Path::new("/home/tester"));
            Ok(vec!["Development".into()])
        });
        assert_eq!(values, [r"~\Development"]);
    }

    #[test]
    fn written_content_preserves_paths_and_depths() {
        let contents = config_contents(&[
            SearchDirEntry::Rich {
                path: "~/Code".into(),
                depth: Some(2),
                include_non_git: None,
            },
            SearchDirEntry::Rich {
                path: "/work".into(),
                depth: Some(4),
                include_non_git: None,
            },
        ])
        .unwrap();
        let (parsed, _) = crate::config::parse_config(&contents).unwrap();
        assert_eq!(parsed.search_dirs.len(), 2);
        assert!(contents.contains("depth = 4"));
    }

    #[test]
    fn default_folder_mode_omits_the_include_non_git_setting() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "~/Code".into();
        state.begin_depth().unwrap();
        state.input.text = "2".into();
        state.commit_depth().unwrap();
        state.commit_folder_mode().unwrap();

        assert_eq!(state.dirs[0].include_non_git, None);
        let search_dirs = state.search_dirs();
        assert_eq!(
            search_dirs,
            [SearchDirEntry::Rich {
                path: "~/Code".into(),
                depth: Some(2),
                include_non_git: None,
            }]
        );
        assert!(
            !config_contents(&search_dirs)
                .unwrap()
                .contains("include_non_git")
        );
    }

    #[test]
    fn all_folders_mode_writes_the_include_non_git_setting() {
        let mut state = SetupState::default();
        state.continue_from_welcome();
        state.input.text = "~/Code".into();
        state.begin_depth().unwrap();
        state.input.text = "2".into();
        state.commit_depth().unwrap();
        state.select_next_folder_mode();
        state.commit_folder_mode().unwrap();

        assert_eq!(state.dirs[0].include_non_git, Some(true));
        assert!(matches!(
            state.search_dirs().as_slice(),
            [SearchDirEntry::Rich {
                include_non_git: Some(true),
                ..
            }]
        ));
        assert!(
            config_contents(&state.search_dirs())
                .unwrap()
                .contains("include_non_git = true")
        );
    }

    #[derive(Default)]
    struct FakeFs {
        files: RefCell<HashMap<PathBuf, Vec<u8>>>,
        renames: RefCell<Vec<(PathBuf, PathBuf)>>,
    }

    impl AtomicWriteFs for FakeFs {
        fn exists(&self, path: &Path) -> bool {
            self.files.borrow().contains_key(path)
        }
        fn create_parent_dirs(&self, _path: &Path) -> io::Result<()> {
            Ok(())
        }
        fn write_new(&self, path: &Path, contents: &[u8]) -> io::Result<()> {
            if self.exists(path) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            self.files.borrow_mut().insert(path.into(), contents.into());
            Ok(())
        }
        fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            if self.exists(to) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            let contents = self
                .files
                .borrow_mut()
                .remove(from)
                .ok_or(io::ErrorKind::NotFound)?;
            self.files.borrow_mut().insert(to.into(), contents);
            self.renames.borrow_mut().push((from.into(), to.into()));
            Ok(())
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.files.borrow_mut().remove(path);
            Ok(())
        }
    }

    #[test]
    fn atomic_writer_uses_temp_then_rename_and_refuses_existing_target() {
        let fs = FakeFs::default();
        let target = Path::new("/config/config.toml");
        write_config_atomic_with(&fs, target, "search_dirs = []\n").unwrap();
        assert_eq!(fs.renames.borrow().len(), 1);
        assert_eq!(fs.files.borrow()[target], b"search_dirs = []\n");
        let error = write_config_atomic_with(&fs, target, "changed").unwrap_err();
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs.files.borrow()[target], b"search_dirs = []\n");
    }

    #[test]
    fn real_atomic_rename_never_replaces_an_existing_target() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.tmp");
        let target = directory.path().join("config.toml");
        fs::write(&source, "new").unwrap();
        fs::write(&target, "existing").unwrap();

        let error = RealFs.rename(&source, &target).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(target).unwrap(), "existing");
        assert_eq!(fs::read_to_string(source).unwrap(), "new");
    }
}
