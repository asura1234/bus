use super::*;

fn bound_turn(text: &str) -> (BusState, RoomId, AgentId, RequestId) {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let request = submit_text(&mut state, room, agent, text);
    start_request(&mut state, request, "launch-codex", 10);
    (state, room, agent, request)
}

fn finish_turn(state: &mut BusState, agent: AgentId, payload: &str, text: &str) {
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "turn-final",
            20,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            payload,
            text,
        )),
        CallbackDisposition::AcceptedCompleted
    );
}

/// A turn final closes the request with no reply and frees the agent queue.
#[test]
fn turn_final_settles_without_a_reply_and_releases_the_queue() {
    let (mut state, room, agent, request) = bound_turn("do the work");
    let queued = submit_text(&mut state, room, agent, "next task");
    let payload = state.request(request).unwrap().prompt.rendered_payload();
    finish_turn(&mut state, agent, &payload, "FINAL_TEXT_SHOULD_NOT_LEAK");

    let settled = state.request(request).unwrap();
    assert!(
        settled.settled(),
        "the request stays open after the turn final"
    );
    assert!(
        settled.reply().is_none(),
        "a turn final must not settle the request as a reply: {settled:?}"
    );
    assert!(
        state.room(room).unwrap().latest_replies.is_empty(),
        "a turn final must not publish a reply entry"
    );
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(state.next_queued_request(agent), Some(queued));

    let mut forwarded = state.clone();
    forwarded
        .route_worker_replies(30_000, &Default::default())
        .unwrap();
    let leaked = forwarded.requests().any(|candidate| {
        candidate.prompt.text.contains("FINAL_TEXT_SHOULD_NOT_LEAK")
            || candidate.prompt.text.contains("Reply to message")
    }) || forwarded.worker_events.values().any(|event| {
        event.text.contains("FINAL_TEXT_SHOULD_NOT_LEAK")
            || event.text.contains("Reply to message")
            || event.text.contains("Turn final")
            || event.text.contains("Late reply")
    });
    assert!(
        !leaked,
        "the turn final was forwarded as a reply: {:?}",
        forwarded
            .requests()
            .map(|candidate| candidate.prompt.text.as_str())
            .collect::<Vec<_>>()
    );
}

/// `bus send --reply-to` still settles the open request before the turn ends.
#[test]
fn explicit_reply_to_settles_the_request_before_the_turn_ends() {
    let (mut state, room, agent, request) = bound_turn("question");
    let message = state.request(request).unwrap().prompt.id;
    let settled = state
        .answer_message(message, agent, "explicit answer".into(), 15)
        .unwrap();
    assert_eq!(settled, request);
    let open_turn = state.request(request).unwrap();
    assert!(open_turn.settled());
    assert_eq!(
        open_turn.reply().map(|reply| reply.text.as_str()),
        Some("explicit answer")
    );
    assert_eq!(state.agent(agent).unwrap().current_request, Some(request));
    assert!(state.next_queued_request(agent).is_none());

    let queued = submit_text(&mut state, room, agent, "next task");
    let payload = state.request(request).unwrap().prompt.rendered_payload();
    finish_turn(&mut state, agent, &payload, "FINAL_TEXT_SHOULD_NOT_LEAK");
    let done = state.request(request).unwrap();
    assert_eq!(
        done.reply().map(|reply| reply.text.as_str()),
        Some("explicit answer")
    );
    assert!(state
        .room(room)
        .unwrap()
        .latest_replies
        .values()
        .all(|reply| !reply.text.contains("FINAL_TEXT_SHOULD_NOT_LEAK")));
    assert_eq!(state.next_queued_request(agent), Some(queued));
}
