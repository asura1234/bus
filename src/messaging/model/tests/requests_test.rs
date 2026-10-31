use super::*;

#[test]
fn pure_wait_reason_preserves_precedence_and_codex_first_session_exception() {
    let (state, id, request) = idle_with_queued(1_000, "go");
    let mut agent = state.agent(id).unwrap().clone();
    agent.status = RuntimeStatus::Idle;
    agent.hook_setup_confirmed = true;
    agent.runtime_identity.launch_id = Some("launch".into());
    agent.runtime_identity.terminal_id = Some("terminal".into());
    agent.runtime_identity.pane_id = Some("pane".into());
    agent.runtime_identity.session_id = None;
    agent.current_request = None;
    agent.provider = Provider::Codex;
    assert_eq!(agent.wait_reason(), None);
    agent.dialog = true;
    assert_eq!(agent.shown_status(), RuntimeStatus::Blocked);
    assert_eq!(agent.wait_reason(), None);
    agent.provider = Provider::ClaudeCode;
    assert_eq!(agent.wait_reason(), Some("session_hook_missing"));
    agent.runtime_identity.pane_id = None;
    assert_eq!(agent.wait_reason(), Some("terminal_identity_missing"));
    agent.current_request = Some(request);
    assert_eq!(agent.wait_reason(), Some("prior_request_active"));
    for (status, reason) in [
        (RuntimeStatus::Unavailable, "agent_unavailable"),
        (RuntimeStatus::Launching, "agent_launching"),
        (RuntimeStatus::Working, "agent_working"),
        (RuntimeStatus::Blocked, "agent_blocked"),
    ] {
        agent.status = status;
        assert_eq!(agent.wait_reason(), Some(reason));
    }
    agent.hook_setup_confirmed = false;
    assert_eq!(agent.wait_reason(), Some("hook_setup_unconfirmed"));
    agent.session_binding_invalidated = true;
    assert_eq!(agent.wait_reason(), Some("session_invalidated"));
    agent.deletion_pending = true;
    assert_eq!(agent.wait_reason(), Some("deletion_pending"));
}

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
fn kimi_intermediate_idle_does_not_end_its_started_turn() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run shell and edit a file");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    assert!(!state.turn_ended(state.request(request).unwrap()));
    assert!(state
        .settle_ended_request(agent, 100_000, |_| None)
        .is_none());
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);
    state
        .observe_status(agent, RuntimeStatus::Working, 100_001)
        .unwrap();
    let payload = state.request(request).unwrap().prompt.rendered_payload();
    let final_event = ProviderCallback::final_event(
        "kimi-final",
        100_002,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        &payload,
        "done",
    );
    assert!(matches!(
        state.accept_callback(final_event),
        CallbackDisposition::AcceptedCompleted | CallbackDisposition::AcceptedPendingSettlement
    ));
}

#[test]
fn kimi_terminal_failure_can_still_settle_without_a_final_reply() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    let ended = RecordedTurnEnd::Error("Kimi turn failed".into());
    assert_eq!(
        state.settle_ended_request(agent, 100_000, |_| Some(ended)),
        Some(request)
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(state
        .request(request)
        .unwrap()
        .failure_reason
        .as_deref()
        .unwrap()
        .contains("Kimi"));
}

#[test]
fn kimi_completed_wire_recovers_one_final_reply_after_intermediate_idle() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    assert!(state
        .settle_ended_request(agent, 100_000, |_| None)
        .is_none());
    let prompt = state.request(request).unwrap().prompt.rendered_payload();
    let record = RecordedTurnEnd::Final {
        turn_id: "turn-1".into(),
        text: "finished".into(),
        prompts: vec![prompt],
    };
    assert_eq!(
        state.settle_ended_request(agent, 100_001, |_| Some(record)),
        Some(request)
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert!(state
        .settle_ended_request(agent, 100_002, |_| None)
        .is_none());
}

#[test]
fn kimi_process_exit_settles_confirmed_submission_before_start_hook() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    state.begin_submission(request, "launch-codex", 10).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    assert!(!state.request(request).unwrap().trusted_start_bound);
    state
        .record_exit_snapshot(
            agent,
            ExitSnapshot {
                launch_id: Some("launch-codex".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("process exited".into()),
                tail: String::new(),
                at_ms: 100,
                terminal_closed: None,
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 100)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(agent, 10_000, |_| None),
        Some(request),
        "a confirmed submission must settle when its provider exits"
    );
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
}

#[test]
fn kimi_trusted_turn_does_not_stall_on_transient_idle() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    let progress = state.request(request).unwrap().progress_at_ms.unwrap();
    assert_eq!(
        stall_at(&state, request, progress.max(21) + DELIVERED_STALL_MS),
        None
    );
}

#[test]
fn kimi_persistent_invalidation_still_reports_session_invalidated() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    let request = submit_text(&mut state, room, agent, "run task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .agents
        .get_mut(&agent)
        .unwrap()
        .session_binding_invalidated = true;
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 21)
        .unwrap();
    let progress = state.request(request).unwrap().progress_at_ms.unwrap();
    assert_eq!(
        stall_at(&state, request, progress.max(21) + DELIVERED_STALL_MS).as_deref(),
        Some("session_invalidated")
    );
}

#[test]
fn uncertain_submission_cannot_end_from_unrelated_status_activity() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "first");
    state.begin_submission(request, "launch-codex", 5).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: "timeout".into(),
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Blocked, 10)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 11)
        .unwrap();
    assert!(!state.turn_ended(state.request(request).unwrap()));
    assert!(state
        .settle_ended_request(agent, 100_000, |_| None)
        .is_none());
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
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
    let expected = state
        .agent(agent)
        .unwrap()
        .wait_reason()
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

#[test]
fn invalidated_unavailable_turn_and_joined_member_stall_after_grace() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let lead = submit_text(&mut state, room, agent, "lead");
    start_request(&mut state, lead, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Working, 2_000)
        .unwrap();
    let member = submit_text(&mut state, room, agent, "joined");
    state.begin_steering(member, lead).unwrap();
    state
        .record_steering(
            member,
            SubmissionOutcome::Uncertain {
                message: "restart".into(),
            },
        )
        .unwrap();
    state.invalidate_agent_session(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 3_000)
        .unwrap();
    for request in [lead, member] {
        assert_eq!(
            stall_at(&state, request, 3_000 + DELIVERED_STALL_MS - 1),
            None
        );
        assert_eq!(
            stall_at(&state, request, 3_000 + DELIVERED_STALL_MS).as_deref(),
            Some("session_invalidated")
        );
        assert_eq!(state.request(request).unwrap().phase, RequestPhase::Active);
    }
    assert_eq!(state.request(member).unwrap().group, Some(lead));
    assert!(state.request(member).unwrap().uncertain_outcome);
}

#[test]
fn transient_unavailable_turn_does_not_stall_without_session_invalidation() {
    let (mut state, agent, request) = idle_with_queued(1_000, "go");
    start_request(&mut state, request, "launch-codex", 5);
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 3_000)
        .unwrap();
    assert_eq!(
        stall_at(&state, request, 3_000 + DELIVERED_STALL_MS * 10),
        None
    );
}

#[test]
fn invalidated_typed_requests_preserve_live_status_and_progress_grace() {
    for phase in [RequestPhase::Submitting, RequestPhase::Active] {
        let (mut state, agent, request) = idle_with_queued(1_000, "go");
        state.begin_submission(request, "launch-codex", 5).unwrap();
        state.requests.get_mut(&request).unwrap().phase = phase;
        state.invalidate_agent_session(agent).unwrap();
        for status in [
            RuntimeStatus::Working,
            RuntimeStatus::Blocked,
            RuntimeStatus::Launching,
        ] {
            state.observe_status(agent, status, 3_000).unwrap();
            assert_eq!(
                stall_at(&state, request, 3_000 + DELIVERED_STALL_MS * 10),
                None
            );
        }
        state
            .observe_status(agent, RuntimeStatus::Unavailable, 4_000)
            .unwrap();
        state.requests.get_mut(&request).unwrap().progress_at_ms = Some(10_000);
        assert_eq!(
            stall_at(&state, request, 10_000 + DELIVERED_STALL_MS - 1),
            None
        );
        assert_eq!(
            stall_at(&state, request, 10_000 + DELIVERED_STALL_MS).as_deref(),
            Some("session_invalidated")
        );
        state.agents.get_mut(&agent).unwrap().dialog = true;
        assert_eq!(
            stall_at(&state, request, 10_000 + DELIVERED_STALL_MS * 10),
            None
        );
    }
}
