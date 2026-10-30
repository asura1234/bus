use super::*;

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
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn collisions_report_provider_precedence_without_warning_about_user_scope_itself() {
    use crate::agents::providers::ProviderKind;
    let home = crate::utils::test_temp::unique_temp_path("bus-skill-shadow");
    let repo = home.join("repo");
    let nested = repo.join("child");
    for directory in [
        home.join(".claude/skills/pr"),
        home.join(".agents/skills/pr"),
        repo.join(".claude/skills/pr"),
        nested.join(".agents/skills/pr"),
    ] {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("SKILL.md"), "skill").unwrap();
    }
    std::process::Command::new("git")
        .arg("init")
        .arg(&repo)
        .output()
        .unwrap();
    assert!(warnings_for_launch(ProviderKind::ClaudeCode, &home, &home)
        .unwrap()
        .is_empty());
    assert!(warnings_for_launch(ProviderKind::Codex, &home, &home)
        .unwrap()
        .is_empty());
    std::process::Command::new("git")
        .arg("init")
        .arg(&home)
        .output()
        .unwrap();
    assert!(warnings_for_launch(ProviderKind::ClaudeCode, &home, &home)
        .unwrap()
        .is_empty());
    let claude = warnings_for_launch(ProviderKind::ClaudeCode, &nested, &home).unwrap();
    assert_eq!(claude.len(), 1);
    assert!(claude[0].contains("user copy precedence"));
    assert!(claude[0].contains("repo/.claude/skills/pr"));
    let codex = warnings_for_launch(ProviderKind::Codex, &nested, &home).unwrap();
    assert_eq!(codex.len(), 1);
    assert!(codex[0].contains("lists both copies"));
    assert!(codex[0].contains("child/.agents/skills/pr"));
    assert!(warnings_for_launch(ProviderKind::Cursor, &nested, &home)
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(feature = "dev-tools")]
#[test]
fn fake_agent_does_not_inspect_provider_skill_directories() {
    use crate::agents::providers::ProviderKind;
    let missing = crate::utils::test_temp::unique_temp_path("fake-no-skill-root");
    assert!(warnings_for_launch(ProviderKind::Fake, &missing, &missing)
        .unwrap()
        .is_empty());
}
