use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail};

use super::{GitProvider, Repo, RepoScan, ScanWarning, Worktree, parse_worktree_porcelain};

const GIT_DIR_ENTRY: &str = ".git";
const GITDIR_FILE_PREFIX: &str = "gitdir:";

#[derive(Debug, Clone, Default)]
pub struct CliGitProvider;

impl GitProvider for CliGitProvider {
    fn scan_repos(&self, dirs: &[(PathBuf, u16)]) -> Result<RepoScan> {
        self.scan_dirs(dirs, false)
    }

    fn scan_repos_streaming(
        &self,
        dir: &Path,
        depth: u16,
        on_found: &dyn Fn(Repo),
    ) -> Result<Vec<ScanWarning>> {
        let mut seen_paths = HashSet::new();
        walk_repos(dir, depth, &mut |path| {
            if seen_paths.insert(path.to_path_buf())
                && let Some(repo) = Self::build_repo_stub(path)
            {
                on_found(repo);
            }
        })
    }

    fn discover_repos(&self, dirs: &[(PathBuf, u16)]) -> Result<RepoScan> {
        self.scan_dirs(dirs, true)
    }

    fn list_branches(&self, repo_path: &Path) -> Result<Vec<String>> {
        let output = run_git(repo_path, ["branch", "--format=%(refname:short)"])?;
        Ok(lines(&output.stdout))
    }

    fn list_remote_branches(&self, repo_path: &Path) -> Result<Vec<String>> {
        let output = run_git(repo_path, ["branch", "-r", "--format=%(refname:short)"])?;
        Ok(parse_remote_branches(&output.stdout))
    }

    fn list_remote_branches_for_remote(
        &self,
        repo_path: &Path,
        remote: &str,
    ) -> Result<Vec<String>> {
        let pattern = format!("{remote}/*");
        let output = run_git(
            repo_path,
            [
                "branch",
                "-r",
                "--format=%(refname:short)",
                "--list",
                &pattern,
            ],
        )?;
        Ok(parse_remote_branches(&output.stdout))
    }

    fn list_worktrees(&self, repo_path: &Path) -> Result<Vec<Worktree>> {
        let output = run_git(repo_path, ["worktree", "list", "--porcelain"])?;
        let worktrees = parse_worktree_porcelain(&String::from_utf8_lossy(&output.stdout));
        if worktrees.is_empty() {
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
        run_git(repo_path, ["fetch", remote])?;
        Ok(())
    }

    fn create_tracking_branch(&self, repo_path: &Path, branch: &str, remote: &str) -> Result<()> {
        let upstream = format!("{remote}/{branch}");
        run_git(repo_path, ["branch", "--track", branch, &upstream])?;
        Ok(())
    }

    fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path) -> Result<()> {
        let canonical =
            fs::canonicalize(worktree_path).unwrap_or_else(|_| worktree_path.to_path_buf());
        let output = Command::new("git")
            .env("LC_ALL", "C")
            .args(["worktree", "remove"])
            .arg(&canonical)
            .current_dir(repo_path)
            .output()
            .with_context(|| format!("failed to run git in {}", repo_path.display()))?;

        if output.status.success() {
            return Ok(());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("is not a working tree") {
            if canonical.exists() {
                fs::remove_dir_all(&canonical).with_context(|| {
                    format!("failed to remove stale worktree {}", canonical.display())
                })?;
            }
            return Ok(());
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
    fn scan_dirs(&self, dirs: &[(PathBuf, u16)], with_worktrees: bool) -> Result<RepoScan> {
        let mut repos = Vec::new();
        let mut warnings = Vec::new();
        for (dir, depth) in dirs {
            let mut enrichment_warnings = Vec::new();
            let walk_warnings = walk_repos(dir, *depth, &mut |path| {
                if with_worktrees {
                    match self.build_repo(path) {
                        Ok(Some(repo)) => repos.push(repo),
                        Ok(None) => {}
                        Err(error) => enrichment_warnings.push(ScanWarning {
                            path: path.to_path_buf(),
                            message: format!("failed to enrich repository: {error:#}"),
                        }),
                    }
                } else if let Some(repo) = Self::build_repo_stub(path) {
                    repos.push(repo);
                }
            })?;
            warnings.extend(walk_warnings);
            warnings.append(&mut enrichment_warnings);
        }

        let mut seen_paths = HashSet::new();
        repos.retain(|repo| seen_paths.insert(repo.path.clone()));
        repos.sort_by_cached_key(|repo| repo.name.to_lowercase());
        Ok(RepoScan { repos, warnings })
    }

    fn build_repo_stub(path: &Path) -> Option<Repo> {
        Some(Repo {
            name: path.file_name()?.to_string_lossy().into_owned(),
            path: path.to_path_buf(),
            worktrees: Vec::new(),
        })
    }

    fn build_repo(&self, path: &Path) -> Result<Option<Repo>> {
        let Some(mut repo) = Self::build_repo_stub(path) else {
            return Ok(None);
        };
        repo.worktrees = self.list_worktrees(path)?;
        Ok(Some(repo))
    }
}

/// Walk a directory tree to `depth` without descending into repositories.
pub fn walk_repos(
    dir: &Path,
    depth: u16,
    on_repo: &mut dyn FnMut(&Path),
) -> Result<Vec<ScanWarning>> {
    if depth == 0 {
        bail!("repository scan depth must be at least 1");
    }
    let mut warnings = Vec::new();
    walk_repos_inner(dir, depth, true, on_repo, &mut warnings)?;
    Ok(warnings)
}

fn walk_repos_inner(
    dir: &Path,
    depth: u16,
    is_search_root: bool,
    on_repo: &mut dyn FnMut(&Path),
    warnings: &mut Vec<ScanWarning>,
) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if is_search_root => {
            return Err(error)
                .with_context(|| format!("failed to read search directory {}", dir.display()));
        }
        Err(error) => {
            warnings.push(ScanWarning {
                path: dir.to_path_buf(),
                message: format!("failed to read nested directory: {error}"),
            });
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(ScanWarning {
                    path: dir.to_path_buf(),
                    message: format!("failed to read directory entry: {error}"),
                });
                continue;
            }
        };
        let path = entry.path();
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                warnings.push(ScanWarning {
                    path: path.clone(),
                    message: format!("failed to inspect directory entry: {error}"),
                });
                continue;
            }
        };
        if !metadata.is_dir() {
            continue;
        }

        if path.join(GIT_DIR_ENTRY).exists() {
            let canonical = fs::canonicalize(&path).unwrap_or(path);
            let repo_root = resolve_main_repo_from_linked_worktree(&canonical)
                .map(|root| fs::canonicalize(&root).unwrap_or(root))
                .unwrap_or(canonical);
            on_repo(&repo_root);
        } else if depth > 1 {
            walk_repos_inner(&path, depth - 1, false, on_repo, warnings)?;
        }
    }

    Ok(())
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

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_remote_branches(bytes: &[u8]) -> Vec<String> {
    lines(bytes)
        .into_iter()
        // Skip remote HEAD pointers such as `origin/HEAD -> origin/main`.
        .filter(|line| !line.contains("->"))
        .filter_map(|line| line.split_once('/').map(|(_, branch)| branch.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs};

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

    #[test]
    fn scans_and_discovers_real_repositories() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "my-repo");
        fs::create_dir(temp.path().join("not-a-repo")).unwrap();
        let provider = CliGitProvider;

        let scanned = provider
            .scan_repos(&[(temp.path().to_path_buf(), 1)])
            .unwrap();
        assert!(scanned.warnings.is_empty());
        assert_eq!(scanned.repos.len(), 1);
        assert_eq!(scanned.repos[0].name, "my-repo");
        assert!(scanned.repos[0].worktrees.is_empty());

        let discovered = provider
            .discover_repos(&[(temp.path().to_path_buf(), 1)])
            .unwrap();
        assert!(discovered.warnings.is_empty());
        assert_eq!(discovered.repos[0].path, fs::canonicalize(repo).unwrap());
        assert_eq!(discovered.repos[0].worktrees.len(), 1);
    }

    #[test]
    fn scan_depth_stops_at_repositories() {
        let temp = TempDir::new().unwrap();
        let nested = temp.path().join("org").join("nested");
        fs::create_dir_all(&nested).unwrap();
        init_test_repo(&nested);
        let provider = CliGitProvider;

        assert!(
            provider
                .scan_repos(&[(temp.path().to_path_buf(), 1)])
                .unwrap()
                .repos
                .is_empty()
        );
        assert_eq!(
            provider
                .scan_repos(&[(temp.path().to_path_buf(), 2)])
                .unwrap()
                .repos
                .len(),
            1
        );

        let child = nested.join("child");
        fs::create_dir_all(&child).unwrap();
        init_test_repo(&child);
        assert_eq!(
            provider
                .scan_repos(&[(temp.path().to_path_buf(), 3)])
                .unwrap()
                .repos
                .len(),
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
    fn batch_scan_deduplicates_linked_worktree_paths() {
        let (temp, repo) = linked_worktree_fixture();
        let provider = CliGitProvider;

        for repos in [
            provider
                .scan_repos(&[(temp.path().to_path_buf(), 1)])
                .unwrap()
                .repos,
            provider
                .discover_repos(&[(temp.path().to_path_buf(), 1)])
                .unwrap()
                .repos,
        ] {
            assert_eq!(repos.len(), 1, "linked worktree must be deduplicated");
            assert_eq!(repos[0].path, fs::canonicalize(&repo).unwrap());
        }
    }

    #[test]
    fn streaming_scan_deduplicates_linked_worktree_paths() {
        let (temp, repo) = linked_worktree_fixture();
        let provider = CliGitProvider;
        let streamed = RefCell::new(Vec::new());
        let warnings = provider
            .scan_repos_streaming(temp.path(), 1, &|found| {
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
                .unwrap(),
            ["main"]
        );
        assert!(
            provider
                .list_remote_branches(&source)
                .unwrap()
                .contains(&"main".into())
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
        assert!(
            provider
                .create_tracking_branch(&local, "feature", "upstream")
                .is_err(),
            "an existing local branch must not be silently reused"
        );
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

        provider.remove_worktree(&repo, &linked).unwrap();
        provider.prune_worktrees(&repo).unwrap();
        assert!(!linked.exists());
        assert_eq!(provider.list_worktrees(&repo).unwrap().len(), 1);
    }

    #[test]
    fn removes_unregistered_directory_with_not_working_tree_fallback() {
        let temp = TempDir::new().unwrap();
        let repo = repo_fixture(temp.path(), "repo");
        let stale = temp.path().join("stale-worktree");
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join("untracked"), "data").unwrap();
        let provider = CliGitProvider;

        provider.remove_worktree(&repo, &stale).unwrap();
        provider.prune_worktrees(&repo).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn missing_search_directory_is_an_error() {
        let provider = CliGitProvider;
        let error = provider
            .scan_repos(&[(PathBuf::from("/definitely/not/here/herdr-kiosk"), 1)])
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("failed to read search directory")
        );
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

        let scan = CliGitProvider
            .scan_repos(&[(temp.path().to_path_buf(), 2)])
            .unwrap();
        assert_eq!(scan.repos.len(), 1);
        assert_eq!(scan.repos[0].name, "good-repo");
        assert!(
            scan.warnings.iter().any(|warning| {
                warning.path == unreadable
                    && warning.message.contains("failed to read nested directory")
            }),
            "warnings were: {:?}",
            scan.warnings
        );
    }

    #[test]
    fn broken_repo_warns_and_does_not_abort_discovery() {
        let temp = TempDir::new().unwrap();
        let good_repo = repo_fixture(temp.path(), "good-repo");
        let broken_repo = temp.path().join("broken-repo");
        fs::create_dir(&broken_repo).unwrap();
        fs::write(
            broken_repo.join(".git"),
            "gitdir: /definitely/missing/herdr-kiosk/gitdir\n",
        )
        .unwrap();

        let scan = CliGitProvider
            .discover_repos(&[(temp.path().to_path_buf(), 1)])
            .unwrap();
        assert_eq!(scan.repos.len(), 1);
        assert_eq!(scan.repos[0].path, fs::canonicalize(good_repo).unwrap());
        assert!(
            scan.warnings.iter().any(|warning| {
                warning
                    .path
                    .file_name()
                    .is_some_and(|name| name == "broken-repo")
                    && warning.message.contains("failed to enrich repository")
            }),
            "warnings were: {:?}",
            scan.warnings
        );
    }
}
