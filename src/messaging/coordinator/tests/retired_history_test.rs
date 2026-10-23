//! One canonical retired identity serves history rendering and stale-ID routing.
use super::*;

fn legacy_fixture(version: u64, canonical: bool) -> (JsonStore, PathBuf, AgentId, RoomId) {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.state.delete_agent(agent, io::now_ms()).unwrap();
    worker.save(worker.state.clone()).unwrap();
    drop(worker);
    let path = dir.join("state.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["version"] = json!(version);
    if canonical {
        let record = &mut value["state"]["retired_agents"][agent.0.to_string()];
        record["name"] = json!("canonical identity");
        record.as_object_mut().unwrap().remove("provider");
        record.as_object_mut().unwrap().remove("cwd");
    } else {
        value["state"]
            .as_object_mut()
            .unwrap()
            .remove("retired_agents");
    }
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    std::fs::write(
        dir.join("history.jsonl"),
        json!({"agent_deleted": {
            "id": agent, "name": "legacy identity", "provider": Provider::Codex, "cwd": dir
        }})
        .to_string()
            + "\n",
    )
    .unwrap();
    (JsonStore::new(path), dir, agent, room)
}

fn record(state: &BusState, id: AgentId) -> serde_json::Value {
    serde_json::to_value(state).unwrap()["retired_agents"][id.0.to_string()].clone()
}

#[test]
fn retired_migration_format_one_prefers_canonical_identity() {
    legacy_retired_identity_label(1);
}

#[test]
fn retired_migration_format_two_prefers_canonical_identity() {
    legacy_retired_identity_label(2);
}

fn legacy_retired_identity_label(version: u64) {
    let (store, dir, agent, room) = legacy_fixture(version, true);
    let mut restored = store.load().unwrap().unwrap();
    assert_eq!(restored.agent_label(agent), "canonical identity [deleted]");
    let retired = restored.retired_agent(agent).unwrap();
    assert_eq!(retired.provider, Some(Provider::Codex));
    assert_eq!(retired.cwd.as_deref(), Some(dir.as_path()));
    assert_eq!(record(&restored, agent)["room"], json!(room));
    let replacement = restored
        .create_agent(
            room,
            "canonical identity",
            Provider::Codex,
            dir.clone(),
            None,
        )
        .unwrap();
    assert_eq!(
        restored
            .retired_recipient(&agent.0.to_string(), room, room)
            .unwrap(),
        Some(replacement)
    );
    store.save(&restored).unwrap();
    assert!(store.compact_history().unwrap());
    let reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded
            .retired_recipient(&agent.0.to_string(), room, room)
            .unwrap(),
        Some(replacement)
    );
    assert_eq!(
        serde_json::to_value(&reloaded).unwrap()["retired_agents"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert!(!std::fs::read_to_string(dir.join("history.jsonl"))
        .unwrap()
        .contains("agent_deleted"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retired_formats_one_and_two_default_rendering_metadata_and_keep_numeric_room() {
    for version in [1, 2] {
        let (store, dir, agent, room) = legacy_fixture(version, true);
        std::fs::write(dir.join("history.jsonl"), "").unwrap();
        let mut state = store.load().unwrap().unwrap();
        let retired = state.retired_agent(agent).unwrap();
        assert_eq!(retired.provider, None);
        assert_eq!(retired.cwd, None);
        assert_eq!(record(&state, agent)["room"], json!(room));
        let replacement = state
            .create_agent(
                room,
                "canonical identity",
                Provider::ClaudeCode,
                dir.clone(),
                None,
            )
            .unwrap();
        store.save(&state).unwrap();
        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("state.json")).unwrap()).unwrap();
        assert_eq!(document["version"], 3);
        assert_eq!(
            document["state"]["retired_agents"][agent.0.to_string()]["room"],
            json!(room)
        );
        let reloaded = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(reloaded.retired_agent(agent).unwrap().provider, None);
        assert_eq!(
            reloaded
                .retired_recipient(&agent.0.to_string(), room, room)
                .unwrap(),
            Some(replacement)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn retired_migration_keeps_all_existing_canonical_fields() {
    let (store, dir, agent, room) = legacy_fixture(2, true);
    let path = dir.join("state.json");
    let mut doc: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let master = doc["state"]["rooms"]
        .as_object()
        .unwrap()
        .iter()
        .find(|(_, r)| r["kind"] == "master")
        .unwrap()
        .0
        .parse::<u64>()
        .unwrap();
    doc["state"]["retired_agents"][agent.0.to_string()] = json!({
        "room": master, "orchestrates": room, "name": "canonical identity",
        "provider": Provider::ClaudeCode, "cwd": "/canonical-cwd"
    });
    std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
    let state = store.load().unwrap().unwrap();
    assert_eq!(
        record(&state, agent),
        doc["state"]["retired_agents"][agent.0.to_string()]
    );
    let mut state = state;
    let replacement = state
        .create_agent(
            RoomId(master),
            "current orchestrator",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(replacement, room).unwrap();
    assert_eq!(
        state
            .retired_recipient(&agent.0.to_string(), room, room)
            .unwrap(),
        Some(replacement)
    );
    store.save(&state).unwrap();
    let reloaded = JsonStore::new(path).load().unwrap().unwrap();
    assert_eq!(
        reloaded
            .retired_recipient(&agent.0.to_string(), room, room)
            .unwrap(),
        Some(replacement)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retired_legacy_only_missing_room_never_forwards_or_masks_known_history() {
    let (store, dir, agent, room) = legacy_fixture(3, true);
    let unknown = AgentId(999_999);
    let legacy = json!({"agent_deleted": {"id":unknown,"name":"canonical identity", "orchestrates":room,
        "provider":Provider::Codex,"cwd":dir}});
    std::fs::write(dir.join("history.jsonl"), legacy.to_string() + "\n").unwrap();
    let mut state = store.load().unwrap().unwrap();
    assert_eq!(state.agent_label(unknown), "canonical identity [deleted]");
    assert!(state
        .retired_recipient(&unknown.0.to_string(), room, room)
        .unwrap_err()
        .contains("no recorded original room"));
    let replacement = state
        .create_agent(
            room,
            "canonical identity",
            Provider::Codex,
            dir.clone(),
            None,
        )
        .unwrap();
    assert_eq!(
        state
            .retired_recipient("canonical identity", room, room)
            .unwrap(),
        Some(replacement)
    );
    assert_eq!(
        state
            .retired_recipient(&agent.0.to_string(), room, room)
            .unwrap(),
        Some(replacement)
    );
    assert!(state.notices(room).next().is_none());
    store.save(&state).unwrap();
    store.compact_history().unwrap();
    let reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(record(&reloaded, unknown).get("room").is_none());
    assert!(reloaded
        .retired_recipient(&unknown.0.to_string(), room, room)
        .unwrap_err()
        .contains("no recorded original room"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn retired_legacy_migration_is_durable_before_compaction_even_after_stale_state_save() {
    let (store, dir, agent, room) = legacy_fixture(3, false);
    // A torn last record must not obstruct migration of committed identities.
    let path = dir.join("history.jsonl");
    let before = std::fs::read(&path).unwrap();
    std::fs::write(&path, [before.as_slice(), b"{\"agent_deleted\":"].concat()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let state = store.load().unwrap().unwrap();
    assert_eq!(state.agent_label(agent), "legacy identity [deleted]");
    assert!(state
        .retired_recipient(&agent.0.to_string(), room, room)
        .unwrap_err()
        .contains("no recorded original room"));
    assert!(!store.compact_history().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    store.fail_once_at(SaveStage::TempWrite);
    assert!(store.save(&state).is_err());
    assert!(!store.compact_history().unwrap());
    let stale = store.load_base().unwrap().unwrap();
    assert!(stale.retired_agent(agent).is_none());
    store.save(&stale).unwrap();
    assert!(!store.compact_history().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    store.save(&state).unwrap();
    assert!(store.compact_history().unwrap());
    let mut reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    reloaded.archive(io::now_ms() + HISTORY_RETENTION_MS * 3);
    assert_eq!(reloaded.agent_label(agent), "legacy identity [deleted]");
    assert_eq!(
        reloaded.retired_agent(agent).unwrap().cwd.as_deref(),
        Some(dir.as_path())
    );
    assert!(!std::fs::read_to_string(path)
        .unwrap()
        .contains("agent_deleted"));
    std::fs::remove_dir_all(dir).unwrap();
}
