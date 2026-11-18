//! A turn that ends with a captured final answers the request it ran for.
use super::*;

fn bound_turn(text: &str) -> (BusState, RoomId, AgentId, RequestId) {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let request = submit_text(&mut state, room, agent, text);
    start_request(&mut state, request, "launch-codex", 10);
    (state, room, agent, request)
}

fn finish_turn(state: &mut BusState, agent: AgentId, request: RequestId, text: &str) {
    let payload = state
        .request(request)
        .expect("request")
        .prompt
        .rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "turn-final",
            20,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            &payload,
            text,
        )),
        CallbackDisposition::AcceptedCompleted
    );
}

/// Live 2026-11-18: developer message 17148 settled "ended" with
/// `reply_captured_at_ms` set, `reply_delivered_at_ms` null and `reply` null.
/// The captured final must be readable as the request's answer.
#[test]
fn a_captured_turn_final_is_the_requests_reply() {
    let (mut state, room, agent, request) = bound_turn("hello");
    finish_turn(&mut state, agent, request, "Hello — the answer.");

    let done = state.request(request).unwrap();
    assert_eq!(done.phase, RequestPhase::Completed);
    assert_eq!(
        done.reply().map(|reply| reply.text.as_str()),
        Some("Hello — the answer."),
        "{done:?}"
    );
    assert_eq!(
        done.completed_reply.as_ref().and_then(|reply| reply.group),
        Some(request)
    );
    assert_eq!(done.delivery_stage(), "replied");
    assert_eq!(
        done.latency.reply_captured_at_ms,
        Some(20),
        "the captured final records when the reply was captured"
    );
    // Replies are not wake messages: nothing is queued, forwarded or cached.
    assert!(state.room(room).unwrap().latest_replies.is_empty());
    assert_eq!(state.agent(agent).unwrap().current_request, None);
}

/// The worker's own `bus send` during its turn is an ordinary message. It
/// neither consumes nor reroutes the deliverable: the turn final still answers
/// the inbound request, and the posted message stays a message.
#[test]
fn an_agents_own_send_during_its_turn_neither_consumes_nor_reroutes_the_final() {
    let (mut state, room, agent, request) = bound_turn("hello");
    let posted = state
        .post_to_developer(
            room,
            agent,
            "Hi. Room 824 is idle; one decision is open.".into(),
            Vec::new(),
            15,
        )
        .unwrap();
    // A cross-room send to a peer during the same turn.
    let other = state.create_room("other").unwrap();
    let peer = state
        .create_agent(other, "peer", Provider::Codex, PathBuf::from("/repo"), None)
        .unwrap();
    let sent = state
        .submit_message_with(
            other,
            Draft {
                text: "take the cleanup lane".into(),
                files: Vec::new(),
                recipient_ids: [peer].into(),
            },
            Author::Agent(agent),
            16,
            false,
        )
        .unwrap()[0];

    finish_turn(&mut state, agent, request, "Hello — the answer.");

    let done = state.request(request).unwrap();
    assert_eq!(
        done.reply().map(|reply| reply.text.as_str()),
        Some("Hello — the answer."),
        "the own send must not consume the captured final: {done:?}"
    );
    assert_eq!(done.delivery_stage(), "replied");
    // The messages the agent posted are untouched, ordinary traffic.
    let notice = state.notice(posted).expect("posted notice");
    assert_eq!(notice.text, "Hi. Room 824 is idle; one decision is open.");
    assert_eq!(
        state.request(sent).unwrap().prompt.text,
        "take the cleanup lane"
    );
    assert!(state.request(sent).unwrap().reply().is_none());
}

/// An explicit `bus send --reply-to` answer is authoritative. The turn final
/// that follows settles the turn without replacing it.
#[test]
fn an_explicit_reply_keeps_its_answer_over_a_later_turn_final() {
    let (mut state, _room, agent, request) = bound_turn("question");
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, agent, "explicit answer".into(), 15)
        .unwrap();

    finish_turn(&mut state, agent, request, "THE LATER TURN FINAL");

    let done = state.request(request).unwrap();
    assert_eq!(
        done.reply().map(|reply| reply.text.as_str()),
        Some("explicit answer")
    );
    assert_eq!(
        done.completed_reply.as_ref().and_then(|reply| reply.group),
        None,
        "an explicit reply keeps its provenance"
    );
}

/// A textless final ends the turn without inventing a reply.
#[test]
fn a_textless_turn_final_stores_no_reply() {
    let (mut state, _room, agent, request) = bound_turn("hello");
    finish_turn(&mut state, agent, request, "   ");

    let done = state.request(request).unwrap();
    assert_eq!(done.phase, RequestPhase::Completed);
    assert!(done.reply().is_none(), "{done:?}");
    assert_eq!(done.delivery_stage(), "ended");
    assert_eq!(done.latency.reply_captured_at_ms, None);
}
