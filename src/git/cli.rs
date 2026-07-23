use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};

use super::{
    DirtyWorktreeRequiresForce, GitProvider, Listed, LocalBranchAlreadyExists, Repo, ScanWarning,
    Worktree, parse_worktree_porcelain,
};

const GIT_DIR_ENTRY: &str = ".git";
const GITDIR_FILE_PREFIX: &str = "gitdir:";
// Long enough for normal network latency, but bounded so a credential prompt cannot hang the UI.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Default)]
pub struct CliGitProvider;

impl GitProvider for CliGitProvider {
    fn scan_repos_streaming(
        &self,
        dir: &Path,
        depth: u16,
        is_cancelled: &dyn Fn() -> bool,
        on_found: &dyn Fn(Repo),
    ) -> Result<Vec<ScanWarning>> {
        let mut seen_paths = HashSet::new();
        walk_repos_with_cancel(dir, depth, is_cancelled, &mut |path| {
            if seen_paths.insert(path.to_path_buf())
                && let Some(repo) = Self::build_repo_stub(path)
            {
                on_found(repo);
            }
        })
    }

    fn list_branches(&self, repo_path: &Path) -> Result<Listed<String>> {
        let output = run_git(repo_path, ["branch", "--format=%(refname:short)"])?;
        Ok(utf8_lines(&output.stdout))
    }

    fn list_remote_branches_for_remote(
        &self,
        repo_path: &Path,
        remote: &str,
    ) -> Result<Listed<String>> {
        let namespace = format!("refs/remotes/{remote}/");
        let output = run_git(
            repo_path,
            [
                "for-each-ref",
                "--format=%(refname:lstrip=2)%00%(symref)",
                &namespace,
            ],
        )?;
        Ok(parse_remote_branches_for_remote(&output.stdout, remote))
    }

    fn list_worktrees(&self, repo_path: &Path) -> Result<Listed<Worktree>> {
        let output = run_git(repo_path, ["worktree", "list", "--porcelain", "-z"])?;
        let worktrees = parse_worktree_porcelain(&output.stdout)?;
        if worktrees.items.is_empty() {
            bail!(
                "git worktree list returned no worktrees for {}",
                repo_path.display()
            );
        }
        Ok(worktrees)
    }

    fn list_remotes(&self, repo_path: &Path) -> Result<Vec<String>> {
        let output = run_git(repo_path, ["remote"])?;
        Ok(lines(&output.stdout))
    }

    fn fetch_remote(&self, repo_path: &Path, remote: &str) -> Result<()> {
        let output = run_fetch_with_timeout(fetch_command(repo_path, remote), FETCH_TIMEOUT)
            .with_context(|| format!("failed to run git in {}", repo_path.display()))?;
        if !output.status.success() {
            bail!(
                "git fetch {remote} failed in {}: {}",
                repo_path.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    fn create_tracking_branch(&self, repo_path: &Path, branch: &str, remote: &str) -> Result<()> {
        let upstream = format!("{remote}/{branch}");
        let output = Command::new("git")
            .env("LC_ALL", "C")
            .args(["branch", "--track", branch, &upstream])
            .current_dir(repo_path)
            .output()
            .with_context(|| format!("failed to run git in {}", repo_path.display()))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if tracking_branch_already_exists(&stderr, branch) {
                return Err(LocalBranchAlreadyExists::new(branch).into());
            }
            bail!(
                "git branch --track {branch} {upstream} failed in {}: {}",
                repo_path.display(),
                stderr.trim()
            );
        }
        Ok(())
    }

    fn is_valid_branch_name(&self, repo_path: &Path, branch: &str) -> Result<bool> {
        let output = Command::new("git")
            .env("LC_ALL", "C")
            .args(["check-ref-format", "--branch", branch])
            .current_dir(repo_path)
            .output()
            .with_context(|| format!("failed to run git in {}", repo_path.display()))?;
        Ok(output.status.success())
    }

    fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
        let canonical =
            fs::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.to_path_buf());
        let mut command = Command::new("git");
        command.env("LC_ALL", "C").args(["worktree", "remove"]);
        if force {
            command.arg("--force");
        }
        let output = command
            .arg(&canonical)
            .current_dir(repo_path)
            .output()
            .with_context(|| format!("failed to run git in {}", repo_path.display()))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("is not a working tree") {
            if !canonical.exists() {
                return Ok(());
            }
            bail!(
                "git no longer tracks this checkout; not deleting {}",
                canonical.display()
            );
        }

        if !force && dirty_worktree_refusal(&stderr) {
            return Err(DirtyWorktreeRequiresForce.into());
        }

        bail!("git worktree remove failed: {}", stderr.trim())
    }

    fn prune_worktrees(&self, repo_path: &Path) -> Result<()> {
        run_git(repo_path, ["worktree", "prune", "--expire", "now"])?;
        Ok(())
    }

    fn default_branch(
        &self,
        repo_path: &Path,
        local_branches: &[String],
    ) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .current_dir(repo_path)
            .output()
            .with_context(|| format!("failed to run git in {}", repo_path.display()))?;
        if output.status.success() {
            let refname = String::from_utf8_lossy(&output.stdout);
            if let Some(branch) = refname.trim().strip_prefix("refs/remotes/origin/") {
                return Ok(Some(branch.to_string()));
            }
        }

        Ok(["main", "master"]
            .into_iter()
            .find(|candidate| local_branches.iter().any(|branch| branch == candidate))
            .map(str::to_string))
    }
}

impl CliGitProvider {
    fn build_repo_stub(path: &Path) -> Option<Repo> {
        Some(Repo {
            name: path.file_name()?.to_string_lossy().into_owned(),
            path: path.to_path_buf(),
            worktrees: Vec::new(),
        })
    }
}

fn walk_repos_with_cancel(
    dir: &Path,
    depth: u16,
    is_cancelled: &dyn Fn() -> bool,
    on_repo: &mut dyn FnMut(&Path),
) -> Result<Vec<ScanWarning>> {
    if depth == 0 {
        bail!("repository scan depth must be at least 1");
    }

    let mut warnings = Vec::new();
    let mut visited = HashSet::new();
    visited.insert(fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()));
    let mut pending = vec![(dir.to_path_buf(), depth, true)];

    while let Some((directory, remaining_depth, is_search_root)) = pending.pop() {
        if is_cancelled() {
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if is_search_root => {
                return Err(error).with_context(|| {
                    format!("failed to read search directory {}", directory.display())
                });
            }
            Err(error) => {
                warnings.push(ScanWarning {
                    path: directory,
                    message: format!("failed to read nested directory: {error}"),
                });
                continue;
            }
        };

        for entry in entries {
            if is_cancelled() {
                return Ok(warnings);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warnings.push(ScanWarning {
                        path: directory.clone(),
                        message: format!("failed to read directory entry: {error}"),
                    });
                    continue;
                }
            };
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    warnings.push(ScanWarning {
                        path: path.clone(),
                        message: format!("failed to inspect directory entry: {error}"),
                    });
                    continue;
                }
            };
            if file_type.is_symlink() || !file_type.is_dir() {
                continue;
            }

            let git_entry = path.join(GIT_DIR_ENTRY);
            let is_repo = match git_entry_exists(&git_entry) {
                Ok(is_repo) => is_repo,
                Err(error) => {
                    warnings.push(ScanWarning {
                        path: git_entry,
                        message: format!("failed to inspect .git entry: {error}"),
                    });
                    continue;
                }
            };
            if is_repo {
                let canonical = match fs::canonicalize(&path) {
                    Ok(canonical) => canonical,
                    Err(error) => {
                        warnings.push(ScanWarning {
                            path,
                            message: format!("failed to resolve repository path: {error}"),
                        });
                        continue;
                    }
                };
                let repo_root = resolve_main_repo_from_linked_worktree(&canonical)
                    .map(|root| fs::canonicalize(&root).unwrap_or(root))
                    .unwrap_or(canonical);
                if visited.insert(repo_root.clone()) {
                    on_repo(&repo_root);
                }
            } else if remaining_depth > 1 {
                pending.push((path, remaining_depth - 1, false));
            }
        }
    }

    Ok(warnings)
}

fn git_entry_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::metadata(path).map(|_| true),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn resolve_main_repo_from_linked_worktree(path: &Path) -> Option<PathBuf> {
    let git_entry = path.join(GIT_DIR_ENTRY);
    if !git_entry.is_file() {
        return None;
    }

    let content = fs::read_to_string(&git_entry).ok()?;
    let gitdir = content
        .lines()
        .find_map(|line| line.strip_prefix(GITDIR_FILE_PREFIX))?
        .trim();
    let gitdir = Path::new(gitdir);
    let gitdir = if gitdir.is_relative() {
        path.join(gitdir)
    } else {
        gitdir.to_path_buf()
    };

    // Only treat `.git` indirections ending in `.git/worktrees/<name>` as
    // linked worktrees. Other `.git` files may belong to unrelated git setups.
    let worktree_metadata = gitdir.parent()?;
    if worktree_metadata.file_name()? != "worktrees" {
        return None;
    }
    let main_git_dir = worktree_metadata.parent()?;
    if main_git_dir.file_name()? != GIT_DIR_ENTRY {
        return None;
    }
    Some(main_git_dir.parent()?.to_path_buf())
}

fn run_git<'a>(repo_path: &Path, args: impl IntoIterator<Item = &'a str>) -> Result<Output> {
    let args: Vec<_> = args.into_iter().collect();
    let output = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .with_context(|| format!("failed to run git in {}", repo_path.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output)
}

fn fetch_command(repo_path: &Path, remote: &str) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["fetch", remote])
        .current_dir(repo_path);
    command
}

#[derive(Debug)]
struct FetchOutput {
    status: ExitStatus,
    stderr: Vec<u8>,
}

fn run_fetch_with_timeout(mut command: Command, timeout: Duration) -> Result<FetchOutput> {
    let (stderr_path, stderr_file) = temporary_stderr_file()?;
    command
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));
    configure_fetch_process(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_file(&stderr_path);
            return Err(error.into());
        }
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                terminate_fetch_child(&mut child);
                let _ = child.wait();
                let _ = fs::remove_file(&stderr_path);
                return Err(error.into());
            }
        }
        if started.elapsed() >= timeout {
            terminate_fetch_child(&mut child);
            let _ = child.wait();
            let _ = fs::remove_file(&stderr_path);
            bail!(
                "git fetch timed out after {} seconds",
                timeout.as_secs_f64()
            );
        }
        thread::sleep(CHILD_POLL_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
    };
    let stderr = fs::read(&stderr_path).unwrap_or_default();
    let _ = fs::remove_file(stderr_path);
    Ok(FetchOutput { status, stderr })
}

fn temporary_stderr_file() -> io::Result<(PathBuf, fs::File)> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..16_u8 {
        let path = std::env::temp_dir().join(format!(
            "herdr-kiosk-fetch-{}-{nonce}-{attempt}.stderr",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary fetch stderr file",
    ))
}

#[cfg(unix)]
fn configure_fetch_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_fetch_process(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_fetch_child(child: &mut std::process::Child) {
    if let Ok(process_group) = i32::try_from(child.id()) {
        // SAFETY: the negative PID targets only the process group created for this fetch child.
        if unsafe { libc::kill(-process_group, libc::SIGKILL) } == 0 {
            return;
        }
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn terminate_fetch_child(child: &mut std::process::Child) {
    let _ = child.kill();
}

fn tracking_branch_already_exists(stderr: &str, branch: &str) -> bool {
    stderr.contains(&format!("a branch named '{branch}' already exists"))
}

fn dirty_worktree_refusal(stderr: &str) -> bool {
    stderr.contains("contains modified or untracked files")
        || stderr.contains("contains modified files")
        || stderr.contains("contains untracked files")
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn utf8_lines(bytes: &[u8]) -> Listed<String> {
    let mut items = Vec::new();
    let mut skipped_unsupported_refs = false;
    for line in bytes.split(|byte| *byte == b'\n') {
        match std::str::from_utf8(line) {
            Ok(line) => {
                let line = line.trim();
                if !line.is_empty() {
                    items.push(line.to_string());
                }
            }
            Err(_) => skipped_unsupported_refs = true,
        }
    }
    Listed::new(items, skipped_unsupported_refs)
}

fn parse_remote_branches_for_remote(bytes: &[u8], remote: &str) -> Listed<String> {
    let prefix = format!("{remote}/");
    let lines = utf8_lines(bytes);
    let items = lines
        .items
        .into_iter()
        .filter_map(|line| {
            let (refname, symref) = line.split_once('\0').unwrap_or((&line, ""));
            symref
                .is_empty()
                .then(|| refname.strip_prefix(&prefix))
                .flatten()
                .map(str::to_string)
        })
        .collect();
    Listed::new(items, lines.skipped_unsupported_refs)
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        fs,
    };

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    fn init_test_repo(dir: &Path) {
        run_test_git(dir, &["init"]);
        run_test_git(dir, &["config", "user.email", "test@example.com"]);
        run_test_git(dir, &["config", "user.name", "Test"]);
        fs::write(dir.join("README.md"), "# test").unwrap();
        run_test_git(dir, &["add", "."]);
        run_test_git(dir, &["commit", "-m", "init"]);
    }

    fn run_test_git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repo_fixture(parent: &Path, name: &str) -> PathBuf {
        let repo = parent.join(name);
        fs::create_dir_all(&repo).unwrap();
        init_test_repo(&repo);
        repo
    }

    fn scan_streaming(
        provider: &CliGitProvider,
        dir: &Path,
        depth: u16,
    ) -> Result<(Vec<Repo>, Vec<ScanWarning>)> {
        let repos = RefCell::new(Vec::new());
        let warnings = provider.scan_repos_streaming(dir, depth, &|| false, &|repo| {
            repos.borrow_mut().push(repo);
        })?;
        Ok((repos.into_inner(), warnings))
    }

    #[test]
    fn streaming_discovery_emits_repo_stubs_without_worktrees() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "my-repo");
        fs::create_dir(temp.path().join("not-a-repo")).unwrap();
        let provider = CliGitProvider;

        let (repos, warnings) = scan_streaming(&provider, temp.path(), 1).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "my-repo");
        assert_eq!(repos[0].path, fs::canonicalize(repo).unwrap());
        assert!(repos[0].worktrees.is_empty());
    }

    #[test]
    fn scan_depth_stops_at_repositories() {
        let temp = TempDir::new().unwrap();
        let nested = temp.path().join("org").join("nested");
        fs::create_dir_all(&nested).unwrap();
        init_test_repo(&nested);
        let provider = CliGitProvider;

        assert!(
            scan_streaming(&provider, temp.path(), 1)
                .unwrap()
                .0
                .is_empty()
        );
        assert_eq!(
            scan_streaming(&provider, temp.path(), 2).unwrap().0.len(),
            1
        );

        let child = nested.join("child");
        fs::create_dir_all(&child).unwrap();
        init_test_repo(&child);
        assert_eq!(
            scan_streaming(&provider, temp.path(), 3).unwrap().0.len(),
            1
        );
    }

    fn linked_worktree_fixture() -> (TempDir, PathBuf) {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "my-repo");
        run_test_git(&repo, &["branch", "feat/worktree"]);
        let linked = temp.path().join("my-repo-worktree");
        run_test_git(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "feat/worktree"],
        );
        (temp, repo)
    }

    #[test]
    fn streaming_scan_deduplicates_linked_worktree_paths() {
        let (temp, repo) = linked_worktree_fixture();
        let provider = CliGitProvider;
        let streamed = RefCell::new(Vec::new());
        let warnings = provider
            .scan_repos_streaming(temp.path(), 1, &|| false, &|found| {
                streamed.borrow_mut().push(found);
            })
            .unwrap();

        assert!(warnings.is_empty());
        let streamed = streamed.into_inner();
        assert_eq!(streamed.len(), 1, "linked worktree must be deduplicated");
        assert_eq!(streamed[0].path, fs::canonicalize(repo).unwrap());
    }

    #[test]
    fn lists_local_branches_and_worktrees() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        run_test_git(&repo, &["branch", "feat/test"]);
        let provider = CliGitProvider;

        let branches = provider.list_branches(&repo).unwrap();
        assert!(branches.iter().any(|branch| branch == "feat/test"));
        assert_eq!(provider.list_worktrees(&repo).unwrap().len(), 1);
        assert!(provider.default_branch(&repo, &branches).unwrap().is_some());
    }

    #[test]
    fn lists_fetches_and_filters_remotes() {
        let temp = TempDir::new().unwrap();
        let remote = temp.path().join("remote.git");
        fs::create_dir(&remote).unwrap();
        run_test_git(&remote, &["init", "--bare"]);
        let source = repo_fixture(temp.path(), "source");
        run_test_git(
            &source,
            &["remote", "add", "upstream", remote.to_str().unwrap()],
        );
        run_test_git(&source, &["push", "upstream", "HEAD:main"]);
        let provider = CliGitProvider;

        assert_eq!(provider.list_remotes(&source).unwrap(), ["upstream"]);
        provider.fetch_remote(&source, "upstream").unwrap();
        assert_eq!(
            provider
                .list_remote_branches_for_remote(&source, "upstream")
                .unwrap()
                .items,
            ["main"]
        );
    }

    #[test]
    fn creates_tracking_branch_from_non_origin_remote_and_rejects_existing_branch() {
        let temp = TempDir::new().unwrap();
        let remote = temp.path().join("remote.git");
        fs::create_dir(&remote).unwrap();
        run_test_git(&remote, &["init", "--bare"]);

        let seed = repo_fixture(temp.path(), "seed");
        run_test_git(&seed, &["branch", "feature"]);
        run_test_git(
            &seed,
            &["remote", "add", "upstream", remote.to_str().unwrap()],
        );
        run_test_git(&seed, &["push", "upstream", "feature"]);

        let local = repo_fixture(temp.path(), "local");
        run_test_git(
            &local,
            &["remote", "add", "upstream", remote.to_str().unwrap()],
        );
        run_test_git(&local, &["fetch", "upstream"]);
        let provider = CliGitProvider;
        provider
            .create_tracking_branch(&local, "feature", "upstream")
            .unwrap();

        let upstream =
            run_git(&local, ["rev-parse", "--abbrev-ref", "feature@{upstream}"]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&upstream.stdout).trim(),
            "upstream/feature"
        );
        let error = provider
            .create_tracking_branch(&local, "feature", "upstream")
            .unwrap_err();
        assert!(
            crate::git::is_local_branch_already_exists(&error),
            "an existing local branch must produce the typed race outcome"
        );
    }

    #[test]
    fn fetch_disables_terminal_prompts() {
        let command = fetch_command(Path::new("/repo"), "upstream");
        let prompt = command
            .get_envs()
            .find(|(key, _)| *key == "GIT_TERMINAL_PROMPT")
            .and_then(|(_, value)| value);
        assert_eq!(prompt, Some(std::ffi::OsStr::new("0")));
    }

    #[cfg(unix)]
    #[test]
    fn fetch_timeout_kills_a_sleeping_git_process() {
        let temp = TempDir::new().unwrap();
        let fake_git = temp.path().join("git");
        let completion_marker = temp.path().join("completed");
        fs::write(&fake_git, "#!/bin/sh\nsleep 5\nprintf completed > \"$1\"\n").unwrap();
        let mut permissions = fs::metadata(&fake_git).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&fake_git, permissions).unwrap();
        let mut command = Command::new(&fake_git);
        command.arg(&completion_marker);

        let error = run_fetch_with_timeout(command, Duration::from_millis(75)).unwrap_err();

        assert!(error.to_string().contains("git fetch timed out"));
        assert!(!completion_marker.exists());
    }

    #[test]
    fn remote_parser_strips_the_exact_remote_and_skips_symbolic_head() {
        let output = b"team/origin/HEAD\0refs/remotes/team/origin/main\n\
                       team/origin/feature\0\n\
                       team/origin/topic/nested\0\n\
                       other/branch\0\n";

        assert_eq!(
            parse_remote_branches_for_remote(output, "team/origin").items,
            ["feature", "topic/nested"]
        );
    }

    #[test]
    fn branch_parser_skips_non_utf8_refs_and_reports_them_once() {
        let listed = utf8_lines(b"main\ninvalid-\xff\nfeature\n");

        assert_eq!(listed.items, ["main", "feature"]);
        assert!(listed.skipped_unsupported_refs);

        let remote = parse_remote_branches_for_remote(
            b"origin/main\0\norigin/invalid-\xff\0\norigin/feature\0\n",
            "origin",
        );
        assert_eq!(remote.items, ["main", "feature"]);
        assert!(remote.skipped_unsupported_refs);
    }

    #[test]
    fn removes_registered_worktree_and_prunes() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        run_test_git(&repo, &["branch", "feature"]);
        let linked = temp.path().join("feature-worktree");
        run_test_git(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "feature"],
        );
        let provider = CliGitProvider;

        provider.remove_worktree(&repo, &linked, false).unwrap();
        provider.prune_worktrees(&repo).unwrap();
        assert!(!linked.exists());
        assert_eq!(provider.list_worktrees(&repo).unwrap().len(), 1);
    }

    #[test]
    fn validates_branch_names_with_git_check_ref_format() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        let provider = CliGitProvider;
        assert!(provider.is_valid_branch_name(&repo, "feat/valid").unwrap());
        assert!(!provider.is_valid_branch_name(&repo, "bad..name").unwrap());
    }

    #[test]
    fn dirty_worktree_refusal_is_typed_and_force_removes_it() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        run_test_git(&repo, &["branch", "feature"]);
        let linked = temp.path().join("feature-worktree");
        run_test_git(
            &repo,
            &["worktree", "add", linked.to_str().unwrap(), "feature"],
        );
        fs::write(linked.join("untracked.txt"), "dirty").unwrap();
        let provider = CliGitProvider;

        let error = provider.remove_worktree(&repo, &linked, false).unwrap_err();
        assert!(crate::git::is_dirty_worktree_requires_force(&error));
        assert!(linked.exists());
        provider.remove_worktree(&repo, &linked, true).unwrap();
        assert!(!linked.exists());
    }

    #[test]
    fn refuses_to_remove_an_existing_unregistered_directory_but_accepts_an_absent_path() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        let stale = temp.path().join("stale-worktree");
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("untracked"), "data").unwrap();
        let provider = CliGitProvider;

        let error = provider.remove_worktree(&repo, &stale, false).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("git no longer tracks this checkout; not deleting")
        );
        assert!(stale.exists());
        assert_eq!(fs::read_to_string(stale.join("untracked")).unwrap(), "data");

        let absent = temp.path().join("already-removed-worktree");
        provider.remove_worktree(&repo, &absent, false).unwrap();
        provider.prune_worktrees(&repo).unwrap();
        assert!(!absent.exists());
    }

    #[test]
    fn missing_search_directory_is_an_error() {
        let provider = CliGitProvider;
        let error = provider
            .scan_repos_streaming(
                Path::new("/definitely/not/here/herdr-kiosk"),
                1,
                &|| false,
                &|_| {},
            )
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to read search directory")
        );
    }

    #[cfg(unix)]
    #[test]
    fn self_looping_directory_symlink_terminates_without_duplicates() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let container = temp.path().join("container");
        fs::create_dir(&container).unwrap();
        let repo = repo_fixture(&container, "repo");
        symlink(&container, container.join("loop")).unwrap();

        let (repos, _) = scan_streaming(&CliGitProvider, temp.path(), u16::MAX).unwrap();

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].path, fs::canonicalize(repo).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_subdirectory_is_not_descended_into() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        repo_fixture(outside.path(), "hidden-repo");
        symlink(outside.path(), temp.path().join("linked")).unwrap();

        let (repos, warnings) = scan_streaming(&CliGitProvider, temp.path(), 2).unwrap();

        assert!(repos.is_empty());
        assert!(warnings.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn broken_git_entry_warns_and_prevents_descent() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let candidate = temp.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        symlink(candidate.join("missing-git-dir"), candidate.join(".git")).unwrap();
        repo_fixture(&candidate, "nested-repo");

        let (repos, warnings) = scan_streaming(&CliGitProvider, temp.path(), 2).unwrap();

        assert!(repos.is_empty());
        assert!(warnings.iter().any(|warning| {
            warning.path == candidate.join(".git")
                && warning.message.contains("failed to inspect .git entry")
        }));
    }

    #[test]
    fn scan_checks_cancellation_while_iterating_directories() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("nested")).unwrap();
        let checks = Cell::new(0);
        let mut found = Vec::new();

        let warnings = walk_repos_with_cancel(
            temp.path(),
            u16::MAX,
            &|| {
                checks.set(checks.get() + 1);
                checks.get() > 1
            },
            &mut |repo| found.push(repo.to_path_buf()),
        )
        .unwrap();

        assert!(warnings.is_empty());
        assert!(found.is_empty());
        assert!(checks.get() > 1);
    }

    #[cfg(unix)]
    #[test]
    fn nested_unreadable_directory_warns_and_scan_continues() {
        struct PermissionRestore {
            path: PathBuf,
            permissions: fs::Permissions,
        }

        impl Drop for PermissionRestore {
            fn drop(&mut self) {
                fs::set_permissions(&self.path, self.permissions.clone()).unwrap();
            }
        }

        let temp = TempDir::new().unwrap();
        repo_fixture(temp.path(), "good-repo");
        let unreadable = temp.path().join("unreadable");
        fs::create_dir(&unreadable).unwrap();
        let original_permissions = fs::metadata(&unreadable).unwrap().permissions();
        let _restore = PermissionRestore {
            path: unreadable.clone(),
            permissions: original_permissions.clone(),
        };
        let mut blocked_permissions = original_permissions;
        blocked_permissions.set_mode(0o0);
        fs::set_permissions(&unreadable, blocked_permissions).unwrap();

        if fs::read_dir(&unreadable).is_ok() {
            return;
        }

        let (repos, warnings) = scan_streaming(&CliGitProvider, temp.path(), 2).unwrap();
        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].name, "good-repo");
        assert!(
            warnings.iter().any(|warning| {
                warning.path == unreadable.join(".git")
                    && warning.message.contains("failed to inspect .git entry")
            }),
            "warnings were: {warnings:?}"
        );
    }
}
