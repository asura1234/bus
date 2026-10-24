//! Canonical retired identities retain old-format defaults and routing.
use super::*;

#[test]
fn retired_formats_one_and_two_default_rendering_metadata_and_keep_numeric_room() {
    for version in [1, 2] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        worker.state.delete_agent(agent, io::now_ms()).unwrap();
        worker.save(worker.state.clone()).unwrap();
        drop(worker);
        let path = dir.join("state.json");
        let mut document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        document["version"] = json!(version);
        let retired = document["state"]["retired_agents"][agent.0.to_string()]
            .as_object_mut()
            .unwrap();
        retired.insert("name".into(), json!("canonical identity"));
        retired.remove("provider");
        retired.remove("cwd");
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        let store = JsonStore::new(path);
        let mut state = store.load().unwrap().unwrap();
        let retired = state.retired_agent(agent).unwrap();
        assert_eq!(retired.provider, None);
        assert_eq!(retired.cwd, None);
        assert_eq!(
            serde_json::to_value(&state).unwrap()["retired_agents"][agent.0.to_string()]["room"],
            json!(room)
        );
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
