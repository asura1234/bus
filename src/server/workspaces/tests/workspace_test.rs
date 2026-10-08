use super::*;

#[test]
fn generated_workspace_ids_are_short_base32_handles() {
    let first = generate_workspace_id();
    let second = generate_workspace_id();

    assert!(first.starts_with('w'));
    assert!(second.starts_with('w'));
    assert_ne!(first, second);
    assert!(first.len() <= 3, "unexpectedly long workspace id: {first}");
    assert!(
        second.len() <= 3,
        "unexpectedly long workspace id: {second}"
    );
}

#[test]
fn public_numbers_round_trip_readable_base32_handles() {
    assert_eq!(encode_public_number(1), "1");
    assert_eq!(encode_public_number(9), "9");
    assert_eq!(encode_public_number(10), "A");
    assert_eq!(encode_public_number(31), "Z");
    assert_eq!(encode_public_number(32), "0");
    assert_eq!(encode_public_number(33), "11");

    for value in [1, 9, 10, 31, 32, 33, 1024, 1025] {
        let encoded = encode_public_number(value);
        assert_eq!(decode_public_number(&encoded), Some(value));
    }
}

#[test]
fn reserving_restored_workspace_ids_prevents_reuse() {
    let mut restored = Workspace::test_new("restored");
    restored.id = "wZ".to_string();

    reserve_workspace_ids(&[restored]);

    let generated = generate_workspace_id();
    assert_ne!(generated, "wZ");
    assert!(public_workspace_number(&generated) > public_workspace_number("wZ"));
}

#[test]
fn pane_public_numbers_are_stable_and_not_reused_after_close() {
    let mut ws = Workspace::test_new("test");
    let root = ws.tabs[0].root_pane;
    let second = ws.test_split(Direction::Horizontal);
    let third = ws.test_split(Direction::Vertical);

    assert_eq!(ws.public_pane_number(root), Some(1));
    assert_eq!(ws.public_pane_number(second), Some(2));
    assert_eq!(ws.public_pane_number(third), Some(3));

    assert!(!ws.close_pane(second));

    assert_eq!(ws.public_pane_number(root), Some(1));
    assert_eq!(ws.public_pane_number(second), None);
    assert_eq!(ws.public_pane_number(third), Some(3));

    let fourth = ws.test_split(Direction::Horizontal);
    assert_eq!(ws.public_pane_number(fourth), Some(4));
}

#[test]
fn tab_public_numbers_are_stable_and_not_reused_after_close() {
    let mut ws = Workspace::test_new("test");
    let first_root = ws.tabs[0].root_pane;
    let second_tab = ws.test_add_tab(None);
    let second_root = ws.tabs[second_tab].root_pane;
    let third_tab = ws.test_add_tab(None);
    let third_root = ws.tabs[third_tab].root_pane;

    assert_eq!(ws.public_tab_number_for_pane(first_root), Some(1));
    assert_eq!(ws.public_tab_number_for_pane(second_root), Some(2));
    assert_eq!(ws.public_tab_number_for_pane(third_root), Some(3));

    assert!(ws.close_tab(second_tab));

    assert_eq!(ws.public_tab_number_for_pane(first_root), Some(1));
    assert_eq!(ws.public_tab_number_for_pane(third_root), Some(3));

    let fourth_tab = ws.test_add_tab(None);
    let fourth_root = ws.tabs[fourth_tab].root_pane;
    assert_eq!(ws.public_tab_number_for_pane(fourth_root), Some(4));
    ws.assert_invariants_for_test();
}

#[test]
fn adversarial_identity_state_satisfies_workspace_invariants_after_mutation() {
    let mut ws = Workspace::test_adversarial_identity_state();
    ws.assert_invariants_for_test();

    let active_public = ws.tabs[ws.active_tab].number;
    assert_ne!(ws.active_tab + 1, active_public);
    let divergent_pane = ws
        .public_pane_numbers
        .iter()
        .find_map(|(pane_id, public_number)| {
            (pane_id.raw() as usize != *public_number).then_some(*pane_id)
        })
        .expect("adversarial state should contain raw/public pane divergence");
    assert_ne!(
        divergent_pane.raw() as usize,
        ws.public_pane_number(divergent_pane).unwrap()
    );

    let new_pane = ws.test_split(Direction::Vertical);
    assert!(ws.public_pane_number(new_pane).is_some());
    ws.assert_invariants_for_test();
}

#[test]
fn linked_worktree_auto_label_uses_checkout_name_not_repo_name() {
    let (base, repo, checkout) =
        self::git_label::test_support::create_repo_with_linked_worktree("linked-auto-label");

    let space = self::git_label::git_space_metadata(&checkout).unwrap();

    assert_eq!(space.repo_name, repo.file_name().unwrap().to_str().unwrap());
    assert_eq!(
        workspace_auto_label(&checkout),
        checkout.file_name().unwrap().to_str().unwrap()
    );

    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn display_name_reads_cached_identity_without_rechecking_filesystem() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "herdr-workspace-label-cache-{}-{stamp}",
        std::process::id()
    ));
    let cwd = root.join("deep/nested");
    std::fs::create_dir_all(&cwd).expect("create nested cwd");

    let mut ws = Workspace::test_new("ignored");
    ws.custom_name = None;
    ws.identity_cwd = cwd.clone();
    ws.tabs.clear();
    ws.cached_identity_cwd = cwd;
    ws.cached_auto_label = "cached-repo".into();

    std::fs::remove_dir_all(root).expect("remove cwd after cache admission");

    assert_eq!(ws.display_name(), "cached-repo");
}

#[test]
fn terminal_aware_display_name_uses_latest_admitted_identity_cache() {
    let mut ws = Workspace::test_new("ignored");
    let root_pane = ws.tabs[0].root_pane;
    let terminal_id = ws.tabs[0].terminal_id(root_pane).unwrap().clone();
    ws.custom_name = None;
    ws.identity_cwd = PathBuf::from("/old/workspace");
    ws.cached_identity_cwd = PathBuf::from("/new/repo/deep");
    ws.cached_auto_label = "repo".into();
    let terminals = HashMap::from([(
        terminal_id.clone(),
        TerminalState::new(terminal_id, PathBuf::from("/new/repo/deep")),
    )]);

    assert_eq!(ws.display_name_from_terminals(&terminals), "repo");
    assert_eq!(ws.display_name(), "workspace");
}

#[test]
fn workspace_identity_follows_first_tab_root_pane_cwd() {
    let mut ws = Workspace::test_new("ignored");
    ws.custom_name = None;
    let root_pane = ws.tabs[0].root_pane;
    let terminal_id = ws.tabs[0].terminal_id(root_pane).unwrap().clone();
    let mut terminals = HashMap::new();
    terminals.insert(
        terminal_id.clone(),
        TerminalState::new(terminal_id, PathBuf::from("/herdr-test/pion")),
    );
    let terminal_runtimes = TerminalRuntimeRegistry::new();

    assert_eq!(ws.display_name_from(&terminals, &terminal_runtimes), "pion");
    assert_eq!(
        ws.resolved_identity_cwd_from(&terminals, &terminal_runtimes),
        Some(PathBuf::from("/herdr-test/pion"))
    );
}
