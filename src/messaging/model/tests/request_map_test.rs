use crate::messaging::model::{Author, BusState, Draft, Provider};

#[test]
fn indexed_history_tracks_individual_answers_across_snapshot_reload_and_archive() {
    let mut state = BusState::default();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    let mut submit = |author, text: &str| {
        state
            .submit_message_from(
                room,
                Draft {
                    text: text.into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                author,
                1000,
            )
            .unwrap()[0]
    };
    let lead = submit(Author::Agent(agent), "peer lead");
    let developer = submit(Author::Developer, "developer follower");
    state.coalesce_queue(lead).unwrap();
    let before = state.clone();
    assert!(
        !state.durable_poll_change_from(&before),
        "index warming must not cause a durable write"
    );

    let message = state.request(developer).unwrap().prompt.id;
    state
        .answer_message(message, agent, "individual answer".into(), 2000)
        .unwrap();

    let json = serde_json::to_value(&state).unwrap();
    assert_eq!(json["requests"].as_object().unwrap().len(), 2);
    assert!(json["requests"][developer.0.to_string()]["completed_reply"].is_object());
    let mut reloaded: BusState = serde_json::from_value(json).unwrap();
    reloaded.finish_for_test(lead, "peer answer", 2001);
    reloaded.archive(1_000_000);
    assert_eq!(reloaded.history().len(), 2);
    assert_eq!(
        reloaded
            .recent_room_requests(room, u64::MAX)
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [developer, lead]
    );
    assert_eq!(
        reloaded
            .recent_room_requests(room, developer.0)
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        [lead]
    );
}
