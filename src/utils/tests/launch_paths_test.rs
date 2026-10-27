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
