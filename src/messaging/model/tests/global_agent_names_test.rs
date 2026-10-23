use super::*;

#[test]
fn global_names_creation_rejects_duplicates_in_every_room_without_mutation() {
    let mut state = BusState::new();
    let home = state.create_room("home").unwrap();
    let side = state.create_room("side").unwrap();
    let master = state.ensure_master_room();
    let owner = state
        .create_agent(home, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    for room in [home, side, master] {
        let before = state.clone();
        let error = state
            .create_agent(room, " builder ", Provider::Codex, "/repo".into(), None)
            .expect_err("agent names must be unique across all rooms")
            .to_string();
        assert!(error.contains("builder") && error.contains(&owner.0.to_string()));
        assert!(error.contains(&format!("room {}", home.0)), "{error}");
        assert_eq!(
            state, before,
            "rejection must not allocate an ID or change colors"
        );
    }
}

#[test]
fn global_names_rename_rejects_other_agents_but_allows_own_trimmed_name() {
    let (mut state, room, first, second) = state_with_room_and_agents();
    let side = state.create_room("side").unwrap();
    let remote = state
        .create_agent(side, "remote", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let name = state.agent(first).unwrap().name.clone();
    for id in [second, remote] {
        let before = state.clone();
        assert!(
            state.rename_agent(id, &name).is_err(),
            "duplicate rename in room {room:?}"
        );
        assert_eq!(state, before);
    }
    state.rename_agent(first, &format!(" {name} ")).unwrap();
    assert_eq!(state.agent(first).unwrap().name, name);
}

#[test]
fn global_names_pending_deletion_reserves_name_until_agent_is_removed() {
    let mut state = BusState::new();
    let home = state.create_room("home").unwrap();
    let side = state.create_room("side").unwrap();
    let first = state
        .create_agent(home, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    state.prepare_delete_agent(first).unwrap();
    assert!(state
        .create_agent(side, "builder", Provider::Codex, "/repo".into(), None)
        .is_err());
    state.delete_agent(first, 10).unwrap();
    let replacement = state
        .create_agent(side, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    assert_ne!(replacement, first);
    assert_eq!(state.retired_agents[&first].name, "builder");
}
