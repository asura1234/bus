use super::{prepare_fixture_home, validate};

#[test]
fn fixture_home_exists_before_a_child_selects_it_as_cwd() {
    let root = std::env::temp_dir().join(format!(
        "bus-prepared-home-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let home = root.join("runtime/home");
    assert!(!home.exists());
    prepare_fixture_home(&home).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .current_dir(&home)
        .output()
        .unwrap();
    assert!(child.status.success(), "{child:?}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn shared_validation_refuses_the_protected_home_and_data_root() {
    let protected = std::env::temp_dir();
    let scratch = protected.join("bus-distinct-home");
    assert!(validate(&protected, None, &protected).is_err());
    assert!(validate(
        &scratch,
        Some(&protected.join(".local/share/bus")),
        &protected
    )
    .is_err());
}

#[cfg(unix)]
#[test]
fn state_components_and_link_targets_cannot_enter_protected_roots() {
    use std::os::unix::fs::symlink;
    let root = std::env::temp_dir().join(format!(
        "bus-state-alias-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let protected = root.join("protected");
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&protected).unwrap();
    for directory in [
        ".local",
        ".claude",
        ".agents",
        ".codex",
        ".cursor",
        ".grok",
        ".kimi-code",
    ] {
        std::fs::create_dir_all(protected.join(directory)).unwrap();
        symlink(protected.join(directory), home.join(directory)).unwrap();
        assert!(validate(&home, None, &protected).is_err(), "{directory}");
        std::fs::remove_file(home.join(directory)).unwrap();
    }
    let actual = root.join("actual-state");
    std::fs::create_dir_all(&actual).unwrap();
    std::fs::create_dir_all(protected.join(".local/share")).unwrap();
    symlink(&actual, protected.join(".local/share/bus")).unwrap();
    assert!(validate(&home, Some(&actual.join("missing/leaf")), &protected).is_err());
    symlink(actual.join("../safe"), root.join("crossing")).unwrap();
    assert!(validate(&home, Some(&root.join("crossing")), &protected).is_err());
    assert!(validate(&home, Some(&root.join("actual-state-sibling")), &protected).is_ok());
    symlink("cycle", root.join("cycle")).unwrap();
    assert!(validate(&home, Some(&root.join("cycle")), &protected).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
