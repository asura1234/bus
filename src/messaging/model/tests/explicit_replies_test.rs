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

fn provider_error(state: &mut BusState, agent: AgentId, error: &str) -> CallbackDisposition {
    state.accept_callback(ProviderCallback {
        callback_id: "native-failure".into(),
        sequence: 30,
        occurred_at_ms: 40,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: None,
        kind: CallbackEventKind::Error {
            message: "Provider turn failed".into(),
            provider_error: Some(error.into()),
        },
    })
}

/// A transient provider error after an explicit reply must not reopen a reply wait.
#[test]
fn an_explicit_reply_then_a_provider_error_stays_replied_and_releases_the_queue() {
    let (mut state, room, agent, request) = bound_turn("question");
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, agent, "explicit answer".into(), 15)
        .unwrap();
    let queued = submit_text(&mut state, room, agent, "next task");
    assert_eq!(
        provider_error(
            &mut state,
            agent,
            "You've hit your 5-hour limit · resets 12:10am (Asia/Singapore) (rate_limit)",
        ),
        CallbackDisposition::AcceptedError
    );
    let settled = state.request(request).unwrap();
    assert!(settled.settled());
    assert_eq!(
        settled.reply().map(|reply| reply.text.as_str()),
        Some("explicit answer")
    );
    assert_eq!(settled.delivery_stage(), "replied");
    assert!(
        settled.failure_reason.is_none(),
        "{:?}",
        settled.failure_reason
    );
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(state.next_queued_request(agent), Some(queued));
}

/// A non-transient provider error closes the turn. It does not wait for a reply.
#[test]
fn a_provider_error_closes_the_turn_without_waiting_for_a_reply() {
    let (mut state, room, agent, request) = bound_turn("question");
    let queued = submit_text(&mut state, room, agent, "next task");
    assert_eq!(
        provider_error(&mut state, agent, "boom"),
        CallbackDisposition::AcceptedError
    );
    let settled = state.request(request).unwrap();
    assert!(settled.settled());
    assert!(settled.reply().is_none());
    assert_ne!(
        settled.failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert!(
        settled
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("provider_error:")),
        "{:?}",
        settled.failure_reason
    );
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(state.next_queued_request(agent), Some(queued));
}

/// Deleting the recipient closes the open turn and does not leave the queue waiting.
#[test]
fn deleting_the_recipient_closes_the_open_turn_and_drops_the_queue() {
    let (mut state, room, agent, request) = bound_turn("question");
    let queued = submit_text(&mut state, room, agent, "next task");
    state.delete_agent(agent, 50).unwrap();
    for id in [request, queued] {
        let closed = state.request(id).unwrap();
        assert!(closed.settled(), "{id:?} stays open after delete");
        assert_eq!(closed.phase, RequestPhase::Abandoned);
        assert_eq!(closed.failure_reason.as_deref(), Some("agent_deleted"));
        assert!(closed.reply().is_none());
    }
    assert!(state.agent(agent).is_none());
    assert!(state.queued_requests(agent).is_empty());
    assert!(state.settle_ended_request(agent, 80, |_| None).is_none());
    assert_eq!(
        state.request(request).unwrap().failure_reason.as_deref(),
        Some("agent_deleted")
    );
}

/// An explicit reply is already terminal, so deleting the recipient does not reopen it.
#[test]
fn an_explicit_reply_stays_settled_when_the_recipient_is_deleted() {
    let (mut state, _, agent, request) = bound_turn("question");
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, agent, "explicit answer".into(), 15)
        .unwrap();
    state.delete_agent(agent, 50).unwrap();
    let kept = state.request(request).unwrap();
    assert!(kept.settled());
    assert_eq!(
        kept.reply().map(|reply| reply.text.as_str()),
        Some("explicit answer")
    );
    assert_eq!(kept.delivery_stage(), "replied");
    assert_ne!(kept.failure_reason.as_deref(), Some("agent_deleted"));
    assert!(state.queued_requests(agent).is_empty());
}
