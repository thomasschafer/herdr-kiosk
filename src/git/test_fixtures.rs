//! Real Git repositories for tests.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub(crate) fn init_test_repo(dir: &Path) {
    run_test_git(dir, &["init"]);
    run_test_git(dir, &["config", "user.email", "test@example.com"]);
    run_test_git(dir, &["config", "user.name", "Test"]);
    fs::write(dir.join("README.md"), "# test").unwrap();
    run_test_git(dir, &["add", "."]);
    run_test_git(dir, &["commit", "-m", "init"]);
}

pub(crate) fn run_test_git(dir: &Path, args: &[&str]) {
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

pub(crate) fn repo_fixture(parent: &Path, name: &str) -> PathBuf {
    let repo = parent.join(name);
    fs::create_dir_all(&repo).unwrap();
    init_test_repo(&repo);
    repo
}
