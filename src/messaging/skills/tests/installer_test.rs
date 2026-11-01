use super::*;

#[cfg(unix)]
#[test]
fn install_blocks_past_the_append_timeout() {
    use std::process::{Command, Stdio};
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-lock-wait");
    std::fs::create_dir_all(&home).unwrap();
    let data = home.join(".local/share/bus");
    crate::messaging::storage::io::private_dir(&data).unwrap();
    let path = data.join("skills-install.lock");
    let marker = data.join("skills-install.lock.held");
    let stderr_path = data.join("holder-stderr");
    let stderr = std::fs::File::create(&stderr_path).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "messaging::storage::io::tests::subprocess_lock_holder",
            "--test-threads",
            "1",
        ])
        .env("BUS_TEST_LOCK_HOLD", &path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut locked = false;
    while std::time::Instant::now() < deadline {
        if marker.is_file() {
            locked = true;
            break;
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if !locked {
        let _ = child.kill();
        let _ = child.wait();
        let detail = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&home);
        panic!("child did not acquire install lock: {detail}");
    }
    let stdin = child.stdin.take().unwrap();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(4));
        drop(stdin);
    });
    let started = std::time::Instant::now();
    let (sender, receiver) = std::sync::mpsc::channel();
    let install_home = home.clone();
    std::thread::spawn(move || {
        let _ = sender.send(install(&install_home));
    });
    let result = receiver.recv_timeout(std::time::Duration::from_secs(30));
    let elapsed = started.elapsed();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&home);
    let report = result
        .expect("install did not return")
        .expect("install failed");
    assert!(report
        .skills
        .iter()
        .all(|skill| skill.action == Some("installed")));
    assert!(
        elapsed >= std::time::Duration::from_secs(3),
        "install returned in {elapsed:?} while another process held the lock"
    );
}

#[test]
fn upgrades_replace_only_matching_owned_trees() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-upgrade");
    std::fs::create_dir_all(&home).unwrap();
    install(&home).unwrap();
    let path = home.join(".agents/skills/worktree-new/SKILL.md");
    let current = std::fs::read(&path).unwrap();
    std::fs::write(&path, "previous Bus release").unwrap();
    let manifest_path = home.join(".local/share/bus/skills-manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["installed"]["codex/worktree-new"]["hash"] =
        serde_json::json!(super::install::tree_hash(path.parent().unwrap()).unwrap());
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(status(&home)
        .unwrap()
        .skills
        .iter()
        .any(|skill| skill.provider == "codex"
            && skill.name == "worktree-new"
            && skill.state == "outdated"));
    install(&home).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), current);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn install_reports_installed_updated_unchanged_and_preserved_conflict() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-actions");
    std::fs::create_dir_all(&home).unwrap();
    let first = install(&home).unwrap();
    assert!(first
        .skills
        .iter()
        .all(|skill| skill.action == Some("installed")));
    let second = install(&home).unwrap();
    assert!(second
        .skills
        .iter()
        .all(|skill| skill.action == Some("unchanged")));

    let path = home.join(".agents/skills/worktree-new/SKILL.md");
    let current = std::fs::read(&path).unwrap();
    std::fs::write(&path, "previous Bus release").unwrap();
    let manifest_path = home.join(".local/share/bus/skills-manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["installed"]["codex/worktree-new"]["hash"] =
        serde_json::json!(super::install::tree_hash(path.parent().unwrap()).unwrap());
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let updated = install(&home).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), current);
    assert!(updated.skills.iter().all(|skill| {
        skill.action
            == Some(
                if skill.provider == "codex" && skill.name == "worktree-new" {
                    "updated"
                } else {
                    "unchanged"
                },
            )
    }));

    let user = home.join(".claude/skills/pr/SKILL.md");
    std::fs::write(&user, "my custom skill\n").unwrap();
    let preserved = install(&home).unwrap();
    assert_eq!(std::fs::read(&user).unwrap(), b"my custom skill\n");
    let conflict = preserved
        .skills
        .iter()
        .find(|skill| skill.provider == "claude" && skill.name == "pr")
        .unwrap();
    assert_eq!(conflict.state, "conflict");
    assert_eq!(conflict.action, Some("preserved-conflict"));
    assert!(preserved.skills.iter().any(|skill| {
        skill.provider == "codex" && skill.name == "pr" && skill.action == Some("unchanged")
    }));
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn corrupt_manifest_stops_before_copying_and_python_caches_do_not_change_ownership() {
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-manifest");
    std::fs::create_dir_all(home.join(".local/share/bus")).unwrap();
    let path = home.join(".local/share/bus/skills-manifest.json");
    std::fs::write(&path, "invalid").unwrap();
    assert!(install(&home).is_err());
    assert!(!home.join(".agents").exists());
    std::fs::remove_file(path).unwrap();
    install(&home).unwrap();
    let cache = home.join(".agents/skills/review-pr/scripts/__pycache__");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join("generated.pyc"), "cache").unwrap();
    assert!(status(&home)
        .unwrap()
        .skills
        .iter()
        .all(|skill| skill.state == "installed"));
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_skills_and_symlink_registries_are_never_written_through() {
    use std::os::unix::fs::symlink;
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-links");
    let outside = home.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::create_dir_all(home.join(".agents")).unwrap();
    symlink(&outside, home.join(".agents/skills")).unwrap();
    std::fs::create_dir_all(home.join(".claude/skills")).unwrap();
    symlink(&outside, home.join(".claude/skills/pr")).unwrap();
    let report = install(&home).unwrap();
    assert!(!report.warnings.is_empty());
    assert!(report
        .skills
        .iter()
        .any(|skill| { skill.provider == "codex" && skill.action == Some("blocked-link") }));
    assert!(report.skills.iter().any(|skill| {
        skill.provider == "claude" && skill.name == "pr" && skill.action == Some("blocked-link")
    }));
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    std::fs::remove_dir_all(home).unwrap();
}
