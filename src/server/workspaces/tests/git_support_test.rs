use std::path::{Path, PathBuf};

pub(super) fn temp_test_dir(name: &str) -> PathBuf {
    let path = crate::utils::test_temp::unique_temp_path(&format!("bus-workspace-tests-{name}"));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn init_repo_with_commit(repo: &Path) {
    std::fs::create_dir_all(repo).unwrap();
    run_git(repo, &["init", "--quiet"]);
    run_git(repo, &["config", "user.email", "bus@example.invalid"]);
    run_git(repo, &["config", "user.name", "Bus Test"]);
    run_git(
        repo,
        &["commit", "--quiet", "--allow-empty", "-m", "initial"],
    );
}

pub(crate) fn create_repo_with_linked_worktree(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = temp_test_dir(name);
    let repo = base.join("bus");
    let checkout = base.join("testr56");
    init_repo_with_commit(&repo);
    run_git(
        &repo,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "testr56",
            checkout.to_str().unwrap(),
            "HEAD",
        ],
    );
    (base, repo, checkout)
}

pub(crate) fn create_bare_repo_with_linked_worktree(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let base = temp_test_dir(name);
    let seed = base.join("seed");
    let bare = base.join(".bare");
    let checkout = base.join("feature");
    init_repo_with_commit(&seed);
    run_git(
        &base,
        &[
            "clone",
            "--quiet",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    run_git(
        &bare,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "feature",
            checkout.to_str().unwrap(),
            "HEAD",
        ],
    );
    (base, bare, checkout)
}

pub(super) fn run_git(cwd: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
