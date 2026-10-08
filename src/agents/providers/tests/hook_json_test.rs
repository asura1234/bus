// Included in the same legacy launch test namespace.

#[test]
fn hook_merge_preserves_existing_json_is_idempotent_and_conflicts_fail_closed() {
    let binary = Path::new("/tmp/a path/it's bus");
    let original = json!({"other":true,"hooks":{"Stop":[{"hooks":[{"type":"command","command":"existing"}]}]}});
    let merged = merge_hooks(original.clone(), Provider::Codex, binary).unwrap();
    assert_eq!(merged["other"], true);
    assert_eq!(merged["hooks"]["Stop"][0], original["hooks"]["Stop"][0]);
    assert_eq!(
        merge_hooks(merged.clone(), Provider::Codex, binary).unwrap(),
        merged
    );
    assert!(hook_command(binary, Provider::Codex).starts_with("'/tmp/a path/it'\\''s bus' "));
}

#[test]
fn hook_merge_moves_bus_entries_from_another_executable_and_fails_closed_on_lookalikes() {
    let old = Path::new("/tmp/a path/it's bus-frozen");
    let current = Path::new("/repo/target/debug/bus");
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let user = if provider == Provider::Cursor {
            json!({"command":"existing"})
        } else {
            json!({"hooks":[{"type":"command","command":"existing"}]})
        };
        let mut stale = merge_hooks(json!({"other":true}), provider, old).unwrap();
        for (event, _) in hook_entries(provider, old) {
            let group = stale["hooks"][event].as_array_mut().unwrap();
            group.insert(0, user.clone());
            group.push(user.clone());
        }
        let moved = merge_hooks(stale, provider, current).unwrap();
        assert_eq!(moved["other"], true);
        for (event, owned) in hook_entries(provider, current) {
            // The Bus entry keeps its place between the user's own hooks.
            assert_eq!(moved["hooks"][event], json!([user, owned, user]));
        }
        assert_eq!(
            merge_hooks(moved.clone(), provider, current).unwrap(),
            moved
        );
    }

    // Only Bus's exact entry shape and quoting count as owned.
    let owned = |command: &str| json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":command,"timeout":5}]}]}});
    for lookalike in [
        owned("sh -c 'bus' --bus-callback codex-hook"),
        owned("/tmp/bus --bus-callback codex-hook; rm -rf /"),
        owned("'/tmp/bus' --bus-callback codex-hook"),
        owned("/tmp/bus --bus-callback claude-hook"),
        owned(" --bus-callback codex-hook"),
        json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"/tmp/bus --bus-callback codex-hook","timeout":5,"extra":1}]}]}}),
        json!({"hooks":{"Stop":[{"matcher":"x","hooks":[{"type":"command","command":"/tmp/bus --bus-callback codex-hook","timeout":5}]}]}}),
    ] {
        assert!(
            merge_hooks(lookalike.clone(), Provider::Codex, current).is_err(),
            "{lookalike}"
        );
    }
}

#[test]
fn project_hook_install_preserves_unrelated_entries_and_refuses_corrupt_files() {
    let dir = std::env::temp_dir().join(format!("bus-hook-install-{}", bus_io::now_ns()));
    bus_io::private_dir(&dir).unwrap();
    let path = dir.join("hooks.json");
    let original = json!({"custom":42,"hooks":{"stop":[{"command":"existing"}]}});
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let binary = Path::new("/tmp/bus fixture");
    install_hooks(&path, Provider::Cursor, binary).unwrap();
    let installed = std::fs::read(&path).unwrap();
    let document: Value = serde_json::from_slice(&installed).unwrap();
    assert_eq!(document["custom"], 42);
    assert_eq!(document["hooks"]["stop"][0], original["hooks"]["stop"][0]);
    assert_eq!(document["hooks"]["stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        document["hooks"]["sessionStart"].as_array().unwrap().len(),
        1
    );
    install_hooks(&path, Provider::Cursor, binary).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), installed);
    std::fs::write(&path, b"corrupt original").unwrap();
    assert!(install_hooks(&path, Provider::Cursor, binary).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt original");
    std::fs::remove_dir_all(dir).unwrap();
}
