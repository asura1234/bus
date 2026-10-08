use super::*;

#[test]
fn submission_preserves_first_recipient_selection_order() {
    let (mut state, room, codex, claude) = state_with_room_and_agents();
    state
        .set_draft_recipients(room, [claude, codex, claude])
        .expect("ordered recipients");
    state.set_draft_text(room, "review this").expect("draft");

    let requests = state.submit_draft(room, 99).expect("submit");
    let request_agents = requests
        .iter()
        .map(|id| state.request(*id).expect("request").agent_id)
        .collect::<Vec<_>>();
    let prompt = &state.request(requests[0]).expect("request").prompt;

    assert_eq!(request_agents, [claude, codex]);
    assert_eq!(
        prompt.recipient_ids.iter().copied().collect::<Vec<_>>(),
        [claude, codex]
    );
    let restored: BusState =
        serde_json::from_value(serde_json::to_value(&state).expect("serialize"))
            .expect("deserialize");
    assert_eq!(
        restored
            .request(requests[0])
            .expect("restored request")
            .prompt
            .recipient_ids
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [claude, codex]
    );
}

#[test]
fn file_only_submission_builds_stable_per_agent_requests_and_clears_only_draft() {
    let (mut state, room, codex, claude) = state_with_room_and_agents();
    let first_path = PathBuf::from("/one/report.md");
    let second_path = PathBuf::from("/two/report.md");
    state.attach_file(room, first_path.clone()).expect("attach");
    state
        .attach_file(room, second_path.clone())
        .expect("attach");
    state
        .attach_file(room, first_path.clone())
        .expect("deduplicate");
    state.remove_file(room, &second_path).expect("remove");
    state
        .attach_file(room, second_path.clone())
        .expect("reattach");
    state
        .set_draft_recipients(room, [codex, claude])
        .expect("recipients");

    let requests = state.submit_draft(room, 99).expect("file-only submit");

    assert_eq!(requests.len(), 2);
    assert_ne!(requests[0], requests[1]);
    let prompt = state.request(requests[0]).expect("request").prompt.clone();
    assert_eq!(prompt.text, "");
    assert_eq!(prompt.files, vec![first_path, second_path]);
    assert_eq!(
        prompt.rendered_payload(),
        "\"/one/report.md\" \"/two/report.md\""
    );
    assert!(state.room(room).expect("room").draft.files.is_empty());
    assert_eq!(
        state
            .room(room)
            .expect("room")
            .latest_prompt
            .as_ref()
            .expect("latest")
            .files,
        prompt.files
    );
}

#[test]
fn turn_ended_needs_work_after_submission_then_idle_and_ignores_reply_capture() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Working, 1)
        .unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 2).unwrap();
    let first = submit_text(&mut state, room, agent, "first");
    let second = submit_text(&mut state, room, agent, "second");
    let ended = |state: &BusState, id| state.turn_ended(state.request(id).unwrap());
    assert!(!ended(&state, first), "queued");

    // Typed, but no turn start reported: only status polls tell work here.
    submit_request(&mut state, first, "launch-codex", 5);
    // Work seen before the submission does not count, nor idle without work.
    state.observe_status(agent, RuntimeStatus::Idle, 3).unwrap();
    assert!(!ended(&state, first));
    state
        .observe_status(agent, RuntimeStatus::Working, 4)
        .unwrap();
    assert!(!ended(&state, first));
    // A blocked agent is not idle.
    state
        .observe_status(agent, RuntimeStatus::Blocked, 5)
        .unwrap();
    assert!(!ended(&state, first));
    state.observe_status(agent, RuntimeStatus::Idle, 6).unwrap();
    // No reply was captured, yet the turn ended.
    assert_eq!(state.request(first).unwrap().phase, RequestPhase::Active);
    assert!(ended(&state, first));
    assert!(!ended(&state, second), "the next message is still queued");

    state.requests.get_mut(&first).unwrap().phase = RequestPhase::Completed;
    assert!(ended(&state, first), "a captured reply counts too");
    state.requests.get_mut(&first).unwrap().phase = RequestPhase::Abandoned;
    assert!(!ended(&state, first));
}

#[test]
fn a_queued_message_refused_by_the_input_box_stalls_with_that_reason() {
    let (mut state, agent, request) = idle_with_queued(1_000, "go");
    state.begin_submission(request, "launch-codex", 5).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::DefinitelyRejected {
                message: "Claude input box is not empty; prompt was not sent".into(),
            },
        )
        .unwrap();
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Queued);
    assert_eq!(
        state.agent(agent).unwrap().delivery_rejection.as_deref(),
        Some("input_box_not_empty")
    );
    assert_eq!(stall_at(&state, request, 1_000 + QUEUED_STALL_MS - 1), None);
    assert_eq!(
        stall_at(&state, request, 1_000 + QUEUED_STALL_MS).as_deref(),
        Some("input_box_not_empty")
    );
    // A confirmed submission clears the refusal.
    state.begin_submission(request, "launch-codex", 6).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: None,
                provider_turn_id: None,
            },
        )
        .unwrap();
    assert_eq!(state.agent(agent).unwrap().delivery_rejection, None);
}

#[test]
fn a_queued_message_without_a_refusal_stalls_with_the_wait_reason() {
    let (state, agent, request) = idle_with_queued(1_000, "go");
    let expected = crate::messaging::diagnostics::wait_reason(state.agent(agent).unwrap())
        .unwrap_or("not_submitted");
    assert_eq!(
        stall_at(&state, request, 1_000 + QUEUED_STALL_MS).as_deref(),
        Some(expected)
    );
}

#[test]
fn typed_messages_stall_by_how_far_they_got_while_the_agent_idles() {
    // Typed with no response yet. The clock starts at the poll before
    // the submission (1_000), not at later idle polls.
    let (mut state, agent, submitting) = idle_with_queued(1_000, "a");
    state
        .begin_submission(submitting, "launch-codex", 5)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 2_000)
        .unwrap();
    assert_eq!(
        stall_at(&state, submitting, 1_000 + DELIVERED_STALL_MS - 1),
        None
    );
    assert_eq!(
        stall_at(&state, submitting, 1_000 + DELIVERED_STALL_MS).as_deref(),
        Some("submission_unconfirmed")
    );

    // Typed and confirmed, but no provider turn start ever came.
    let (mut state, agent, unstarted) = idle_with_queued(1_000, "b");
    submit_request(&mut state, unstarted, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Idle, 2_000)
        .unwrap();
    assert_eq!(
        stall_at(&state, unstarted, 1_000 + DELIVERED_STALL_MS).as_deref(),
        Some("no_start_hook")
    );

    // Started, finished, but no final reply matched the transcript.
    let (mut state, agent, started) = idle_with_queued(1_000, "c");
    start_request(&mut state, started, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Idle, 2_000)
        .unwrap();
    let progress = state.request(started).unwrap().progress_at_ms.unwrap();
    assert_eq!(
        stall_at(&state, started, progress.max(2_000) + DELIVERED_STALL_MS).as_deref(),
        Some("transcript_unmatched")
    );
    // A later hook event restarts the clock.
    state.requests.get_mut(&started).unwrap().progress_at_ms = Some(500_000);
    assert_eq!(
        stall_at(&state, started, 500_000 + DELIVERED_STALL_MS - 1),
        None
    );
}

#[test]
fn a_long_working_turn_never_stalls_and_a_long_block_is_reported_not_stalled() {
    let (mut state, agent, request) = idle_with_queued(1_000, "go");
    start_request(&mut state, request, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Working, 2_000)
        .unwrap();
    let hour = 60 * 60 * 1000;
    assert_eq!(stall_at(&state, request, 2_000 + hour), None);
    assert!(!state.blocked_unanswered(state.request(request).unwrap(), 2_000 + hour));

    state
        .observe_status(agent, RuntimeStatus::Blocked, 3_000)
        .unwrap();
    assert_eq!(
        stall_at(&state, request, 3_000 + hour),
        None,
        "blocked is not stalled"
    );
    let blocked = |now| state.blocked_unanswered(state.request(request).unwrap(), now);
    assert!(!blocked(3_000 + BLOCKED_STALL_MS - 1));
    assert!(blocked(3_000 + BLOCKED_STALL_MS));
}

#[test]
fn a_turn_between_two_idle_polls_still_ends_through_its_provider_start() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let request = submit_text(&mut state, room, agent, "quick task");
    submit_request(&mut state, request, "launch-codex", 5);
    // The whole turn runs between two polls: every poll sees Idle, and
    // the reply is never captured. Only the provider's turn start says
    // the agent took the message.
    state.observe_status(agent, RuntimeStatus::Idle, 2).unwrap();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "fast-start".into(),
            sequence: 12,
            occurred_at_ms: 12,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("quick task".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    let ended = |state: &BusState| state.turn_ended(state.request(request).unwrap());
    // Idle seen before the start event does not end the turn.
    assert!(!ended(&state));
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .unwrap();
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);
    assert!(ended(&state));
}

#[test]
fn queued_requests_remain_fifo_while_active_request_is_blocked_twice() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let first = submit_text(&mut state, room, agent, "first");
    let second = submit_text(&mut state, room, agent, "second");
    let third = submit_text(&mut state, room, agent, "third");

    assert_eq!(state.next_queued_request(agent), Some(first));
    start_request(&mut state, first, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Blocked, 20)
        .expect("blocked");
    state
        .observe_status(agent, RuntimeStatus::Working, 21)
        .expect("working");
    state
        .observe_status(agent, RuntimeStatus::Blocked, 22)
        .expect("blocked again");
    state
        .observe_status(agent, RuntimeStatus::Working, 23)
        .expect("working again");

    assert_eq!(
        state.request(first).expect("first").phase,
        RequestPhase::Active
    );
    assert_eq!(state.next_queued_request(agent), None);
    assert_eq!(state.queued_requests(agent), &[second, third]);
}

#[test]
fn recovery_abandons_only_an_idle_current_request_and_releases_its_agent() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "wedged request");
    let queued = submit_text(&mut state, room, agent, "next request");
    start_request(&mut state, request, "launch-codex", 10);

    assert_eq!(
        state.recover_idle_request(request, 20),
        Err(ModelError::AgentNotIdle)
    );
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    state.recover_idle_request(request, 22).unwrap();

    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(state.request(request).unwrap().completed_at_ms, Some(22));
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(state.next_queued_request(agent), Some(queued));
    assert!(state.room(room).unwrap().latest_replies.is_empty());
}
