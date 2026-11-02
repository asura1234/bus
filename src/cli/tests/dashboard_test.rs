#[test]
fn dashboard_is_dev_only_and_session_selection_does_not_write_the_registry() {
    use crate::cli::session_pick::parse_invocation;
    let args = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(parse_invocation(&args(&["dashboard"])).is_err());
    let invocation = parse_invocation(&args(&["--dev", "dashboard"])).unwrap();
    assert_eq!(format!("{:?}", invocation.action), "Dashboard");
    assert!(parse_invocation(&args(&["--dev", "dashboard", "extra"])).is_err());
    let root = crate::utils::test_temp::unique_temp_path("dashboard-registry");
    let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(root.clone());
    let session = registry.create().unwrap();
    std::fs::remove_file(root.join("session-registry.lock")).unwrap();
    let before = std::fs::metadata(root.join("last-session"))
        .unwrap()
        .modified()
        .unwrap();
    let selected = crate::cli::last_session(&registry, &invocation.action).unwrap();
    assert_eq!(selected.id, session.id);
    assert_eq!(
        std::fs::metadata(root.join("last-session"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert!(!root.join("session-registry.lock").exists());
    std::fs::remove_dir_all(root).unwrap();
}
