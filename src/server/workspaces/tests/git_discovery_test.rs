#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::server::workspaces::git_label::test_support::run_git;

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = format!(
            "bus-workspace-tests-{}-{}-{}",
            name,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn git_repo_root_ignores_invalid_git_marker() {
        let base = temp_test_dir("invalid-git-root");
        let cwd = base.join("workspace");
        std::fs::create_dir_all(base.join(".git")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        assert_eq!(git_repo_root(&cwd), None);

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn git_repo_root_ignores_standalone_non_bare_git_dir_layout() {
        let root = temp_test_dir("standalone-non-bare-git-dir");
        std::fs::write(root.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(root.join("objects")).unwrap();
        std::fs::create_dir_all(root.join("refs")).unwrap();
        std::fs::write(root.join("config"), "[core]\n\tbare = false\n").unwrap();

        assert_eq!(git_repo_root(&root.join("refs")), None);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_space_metadata_supports_standalone_bare_repo() {
        let bare = temp_test_dir("bare-space");
        run_git(&bare, &["init", "--bare", "."]);
        let nested = bare.join("refs");

        let info = git_worktree_info(&nested).expect("bare repo should be discovered");
        assert!(info.is_bare);
        assert!(!info.is_linked_worktree);
        assert_eq!(info.git_dir, canonicalize_best_effort_path(&bare));

        let metadata = git_space_metadata(&nested).expect("bare repo should map to a git space");
        assert_eq!(
            canonicalize_best_effort_path(&metadata.repo_root),
            canonicalize_best_effort_path(&bare)
        );
        assert!(!metadata.is_linked_worktree);

        std::fs::remove_dir_all(bare).unwrap();
    }

    #[test]
    fn bare_source_and_linked_checkout_share_repo_name_but_not_auto_label() {
        let (base, bare, checkout) =
            crate::server::workspaces::git_label::test_support::create_bare_repo_with_linked_worktree(
                "bare-linked-labels",
            );

        let bare_space = git_space_metadata(&bare).unwrap();
        let checkout_space = git_space_metadata(&checkout).unwrap();
        let bare_auto_label = automatic_workspace_label(&bare, &bare_space.repo_root);
        let checkout_auto_label = automatic_workspace_label(&checkout, &checkout_space.repo_root);

        assert_eq!(bare_space.key, checkout_space.key);
        assert_eq!(bare_space.repo_name, ".bare");
        assert_eq!(checkout_space.repo_name, bare_space.repo_name);
        assert_eq!(bare_auto_label, bare.file_name().unwrap().to_str().unwrap());
        assert_eq!(
            checkout_auto_label,
            checkout.file_name().unwrap().to_str().unwrap()
        );

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn embedded_dot_bare_source_and_checkout_use_container_repo_name() {
        let base = temp_test_dir("embedded-dot-bare");
        let seed = base.join("seed");
        let repo = base.join("reported-repo");
        let bare = repo.join(".bare");
        let checkout = repo.join("develop");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&seed, &["init", "--quiet"]);
        run_git(&seed, &["config", "user.email", "bus@example.invalid"]);
        run_git(&seed, &["config", "user.name", "Bus Test"]);
        run_git(
            &seed,
            &["commit", "--quiet", "--allow-empty", "-m", "initial"],
        );
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
        std::fs::write(repo.join(".git"), "gitdir: ./.bare\n").unwrap();
        run_git(
            &bare,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "develop",
                checkout.to_str().unwrap(),
                "HEAD",
            ],
        );

        let source = git_space_metadata(&repo).unwrap();
        let linked = git_space_metadata(&checkout).unwrap();

        assert_eq!(source.repo_name, "reported-repo");
        assert_eq!(linked.repo_name, source.repo_name);

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn git_space_metadata_marks_bare_dot_git_repo() {
        let root = temp_test_dir("bare-dot-git");
        run_git(&root, &["init", "--bare", ".git"]);

        let info = git_worktree_info(&root).expect("bare .git repo should be discovered");
        assert!(info.is_bare);
        assert!(!info.is_linked_worktree);
        assert_eq!(
            info.git_dir,
            canonicalize_best_effort_path(&root.join(".git"))
        );

        let metadata = git_space_metadata(&root).expect("bare .git repo should map to a git space");
        assert_eq!(
            canonicalize_best_effort_path(&metadata.repo_root),
            canonicalize_best_effort_path(&root)
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_space_metadata_discovers_bare_repo_with_numeric_true_config() {
        let bare = temp_test_dir("bare-numeric-true");
        run_git(&bare, &["init", "--bare", "."]);
        run_git(&bare, &["config", "core.bare", "1"]);
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&bare)
            .args(["rev-parse", "--is-bare-repository"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "true");

        let cwd = bare.join("refs");
        let info = git_worktree_info(&cwd);
        let label = crate::server::workspaces::workspace_auto_label(&cwd);
        let expected_label = bare.file_name().unwrap().to_str().unwrap().to_string();
        std::fs::remove_dir_all(&bare).unwrap();

        assert_eq!(label, expected_label);
        let info = info.expect("Git's numeric true boolean must identify a bare repo");
        assert!(info.is_bare);
        assert_eq!(info.repo_root, bare);
    }
}
