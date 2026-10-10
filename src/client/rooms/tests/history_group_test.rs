use super::*;
use crate::messaging::model::{Draft, Provider};

#[test]
fn a_cross_room_group_shows_its_reply_once_in_each_room() {
    let mut state = BusState::default();
    state.ensure_master_room();
    let master = state.master_room().unwrap().id;
    let work = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    let mut requests = Vec::new();
    for (room, text) in [
        (work, "dialog"),
        (master, "human question"),
        (work, "follow-up"),
    ] {
        requests.push(
            state
                .submit_message_from(
                    room,
                    Draft {
                        text: text.into(),
                        files: Vec::new(),
                        recipient_ids: [agent].into(),
                    },
                    Author::Human,
                    1_000 + requests.len() as u64,
                )
                .unwrap()[0],
        );
    }
    let mut saved = serde_json::to_value(&state).unwrap();
    for id in &requests {
        let request = &mut saved["requests"][id.0.to_string()];
        request["phase"] = "completed".into();
        request["pending_final"] = serde_json::json!({
            "callback_id": "final", "text": "shared reply", "received_at_ms": 2_000,
            "provider_session_id": "session", "provider_turn_id": "turn"
        });
        if *id != requests[0] {
            request["group"] = requests[0].0.into();
        }
    }
    let state: BusState = serde_json::from_value(saved).unwrap();
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    for room in [work, master] {
        let lines = history.lines(
            &state,
            state.room(room).unwrap(),
            100,
            1,
            3_000,
            &mut thumbnails,
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.text.contains("shared reply"))
                .count(),
            1,
            "{room:?}"
        );
        assert!(lines.iter().all(|line| !line.text.contains('…')));
    }
}

#[test]
fn cached_work_history_updates_a_renamed_master_recipient() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(
            master,
            "old-orchestrator",
            Provider::Codex,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    state
        .submit_message_from(
            work,
            Draft {
                text: "question for the orchestrator".into(),
                files: Vec::new(),
                recipient_ids: [orchestrator].into(),
            },
            Author::Human,
            1_000,
        )
        .unwrap();
    let mut history = History::default();
    let before = history.lines(
        &state,
        state.room(work).unwrap(),
        100,
        1,
        2_000,
        &mut Thumbnails::default(),
    );
    assert!(before
        .iter()
        .any(|line| line.text.contains("old-orchestrator")));

    state
        .rename_agent(orchestrator, "new-orchestrator")
        .unwrap();
    let after = history.lines(
        &state,
        state.room(work).unwrap(),
        100,
        2,
        2_000,
        &mut Thumbnails::default(),
    );
    assert!(
        after
            .iter()
            .any(|line| line.text.starts_with("You → new-orchestrator")),
        "a new revision must show the recipient's current name: {:?}",
        after
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
    );
}
