use super::*;

fn callback(
    agent: AgentId,
    sequence: u64,
    turn: &str,
    prompt: Option<&str>,
    kind: CallbackEventKind,
) -> ProviderCallback {
    ProviderCallback {
        callback_id: format!("callback-{sequence}"),
        sequence,
        occurred_at_ms: sequence,
        agent_id: agent,
        launch_id: "launch".into(),
        provider_session_id: Some("session".into()),
        provider_turn_id: Some(turn.into()),
        provider_prompt_id: Some(turn.into()),
        prompt_payload: prompt.map(str::to_owned),
        kind,
    }
}

fn submit(state: &mut BusState, room: RoomId, agent: AgentId, text: &str, time: u64) -> RequestId {
    state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            time,
        )
        .unwrap()[0]
}

fn confirmed() -> SubmissionOutcome {
    SubmissionOutcome::Confirmed {
        provider_session_id: Some("session".into()),
        provider_turn_id: None,
    }
}

#[test]
fn rejected_new_turn_interim_must_not_make_answered_joined_messages_stall() {
    let mut state = BusState::default();
    let room = state.ensure_master_room();
    let agent = state
        .create_agent(
            room,
            "mobile master",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let lead = submit(&mut state, room, agent, "question", 2);
    state.begin_submission(lead, "launch", 326).unwrap();
    state.record_submission(lead, confirmed()).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Working, 3)
        .unwrap();
    assert_eq!(
        state.accept_callback(callback(
            agent,
            327,
            "turn-1",
            Some("question"),
            CallbackEventKind::PromptStarted
        )),
        CallbackDisposition::AcceptedBinding
    );

    let mut joined = Vec::new();
    for (sequence, text) in [(328, "authorization"), (329, "retraction")] {
        let id = submit(&mut state, room, agent, text, sequence);
        state.begin_steering(id, lead).unwrap();
        state.record_steering(id, confirmed()).unwrap();
        // Both submit hooks named turn-1, though the transcript later
        // executed the retraction under turn-2.
        assert_eq!(
            state.accept_callback(callback(
                agent,
                sequence,
                "turn-1",
                Some(text),
                CallbackEventKind::PromptStarted
            )),
            CallbackDisposition::AcceptedSteering
        );
        joined.push(id);
    }

    // Replay consume_callback_record: shell-bearing Finals become
    // BackgroundPending, then their captured text goes into shell_hold.
    assert_eq!(
        state.accept_callback(callback(
            agent,
            330,
            "turn-1",
            None,
            CallbackEventKind::BackgroundPending
        )),
        CallbackDisposition::AcceptedProgress
    );
    state.hold_background_reply(
        agent,
        PendingFinal {
            callback_id: "callback-330".into(),
            text: "first answer".into(),
            received_at_ms: 330,
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn-1".into()),
        },
    );
    assert!(state.request(lead).unwrap().has_held_background_reply());

    assert_eq!(
        state.accept_callback(callback(
            agent,
            331,
            "turn-2",
            None,
            CallbackEventKind::BackgroundPending
        )),
        CallbackDisposition::Rejected(CallbackRejection::WrongTurn)
    );
    // The coordinator still passes the rejected callback's interim here.
    state.hold_background_reply(
        agent,
        PendingFinal {
            callback_id: "callback-331".into(),
            text: "retraction acknowledged".into(),
            received_at_ms: 331,
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn-2".into()),
        },
    );
    state
        .observe_status(agent, RuntimeStatus::Idle, 332)
        .unwrap();
    let check_at = 332 + DELIVERED_STALL_MS;
    for id in std::iter::once(lead).chain(joined.iter().copied()) {
        let request = state.request(id).unwrap();
        if id != lead {
            assert_eq!(request.group, Some(lead));
        }
        assert_eq!(
            state.stall_reason(request, check_at),
            None,
            "an answered group with a captured shell-held reply must not report transcript_unmatched"
        );
    }
    let held = state.request(lead).unwrap();
    assert!(held.has_held_background_reply());
    assert_eq!(
        held.shell_hold.as_ref().unwrap().interim.text,
        "first answer"
    );
    assert_eq!(
        state.release_shell_hold(agent, check_at, DELIVERED_STALL_MS),
        Some(lead)
    );
    for id in std::iter::once(lead).chain(joined) {
        let request = state.request(id).unwrap();
        assert_eq!(request.phase, RequestPhase::Completed);
        assert_eq!(request.reply().unwrap().text, "first answer");
        assert_eq!(
            request.failure_reason.as_deref(),
            Some("background_shells_timeout")
        );
    }
}
