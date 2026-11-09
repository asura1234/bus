use super::*;

const DECISION: &str = "retained worker decision";

fn worker_woken_turn() -> (BusState, RoomId, AgentId, AgentId, RequestId) {
    let (mut state, room, worker, _) = state_with_room_and_agents();
    let master = state.ensure_master_room();
    let orch = state
        .create_agent(
            master,
            "decision-maker",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("launch-orch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("provider-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(orch).unwrap();
    state.observe_status(orch, RuntimeStatus::Idle, 1).unwrap();
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "worker result".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(worker),
            2,
        )
        .unwrap()[0];
    start_request(&mut state, request, "launch-orch", 10);
    (state, master, worker, orch, request)
}

fn finish_decision(state: &mut BusState, orch: AgentId, request: RequestId) {
    let prompt = state.request(request).unwrap().prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "decision-final",
            13,
            orch,
            "launch-orch",
            "provider-session",
            "turn-1",
            &prompt,
            DECISION
        )),
        CallbackDisposition::AcceptedCompleted
    );
}

#[test]
fn deleted_worker_keeps_the_orchestrators_master_decision() {
    let (mut state, master, worker, orch, request) = worker_woken_turn();
    state.delete_agent(worker, 12).unwrap();
    assert!(state.agent(worker).is_none());
    finish_decision(&mut state, orch, request);
    let settled = state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(settled.reply().is_none());
    assert!(settled.failure_reason.is_none());
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        0,
        "a captured turn final is not copied into MASTER"
    );
}

#[test]
fn an_individual_answer_keeps_its_reply_and_the_later_decision_reaches_master() {
    let (mut state, master, worker, orch, request) = worker_woken_turn();
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, orch, "Use the fallback".into(), 12)
        .unwrap();
    finish_decision(&mut state, orch, request);
    let settled = state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert_eq!(
        settled.reply().map(|reply| reply.text.as_str()),
        Some("Use the fallback")
    );
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        0,
        "a later captured final is not published over the explicit answer"
    );
    state
        .route_worker_replies(100_000, &std::collections::BTreeSet::new())
        .unwrap();
    let instructions: Vec<_> = state
        .requests()
        .filter(|r| r.agent_id == worker && r.prompt.author == Author::Agent(orch))
        .collect();
    assert!(
        instructions.is_empty(),
        "neither the explicit answer nor the captured final is forwarded: {:?}",
        instructions
            .iter()
            .map(|r| r.prompt.text.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn an_explicit_master_post_is_not_repeated_by_the_captured_final() {
    let (mut state, master, _, orch, request) = worker_woken_turn();
    state
        .post_to_developer(master, orch, DECISION.into(), vec![], 12)
        .unwrap();
    finish_decision(&mut state, orch, request);
    let settled = state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(
        settled.reply().is_none(),
        "an explicit master post is not stored as the turn reply"
    );
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        1,
        "the explicit post stays once and the captured final does not repeat it"
    );
}

#[test]
fn a_new_decision_after_answering_a_master_worker_wake_is_published() {
    let (mut state, room, worker, _) = state_with_room_and_agents();
    let master = state.ensure_master_room();
    let orch = state
        .create_agent(
            master,
            "master-decision-maker",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("launch-orch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("provider-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(orch).unwrap();
    state.observe_status(orch, RuntimeStatus::Idle, 1).unwrap();
    // Reply notifications use this production constructor and the source
    // request's room, including MASTER for a task sent there by its orchestrator.
    let request = state
        .submit_bus_message_with(
            master,
            Draft {
                text: "Reply to message 19 (request 20).\nworker result".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(worker),
            2,
            false,
        )
        .unwrap()[0];
    assert!(state.request(request).unwrap().bus_generated);
    start_request(&mut state, request, "launch-orch", 10);
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, orch, "Use the fallback".into(), 12)
        .unwrap();
    finish_decision(&mut state, orch, request);
    let settled = state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert_eq!(
        settled.reply().map(|reply| reply.text.as_str()),
        Some("Use the fallback")
    );
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        0,
        "a captured final after an explicit answer is not published to MASTER"
    );
}

#[test]
fn worker_woken_decision_with_unclaimed_notification_is_published_after_turn_proof() {
    let (mut state, room, worker, _) = state_with_room_and_agents();
    let master = state.ensure_master_room();
    let orch = state
        .create_agent(
            master,
            "developer-lead-decision-maker",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("launch-orch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("provider-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(orch).unwrap();
    state.observe_status(orch, RuntimeStatus::Idle, 1).unwrap();
    let lead = state
        .submit_message_from(
            room,
            Draft {
                text: "developer lead".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Developer,
            2,
        )
        .unwrap()[0];
    start_request(&mut state, lead, "launch-orch", 10);
    state
        .observe_status(orch, RuntimeStatus::Working, 12)
        .unwrap();
    let worker_request = state
        .submit_bus_message_with(
            room,
            Draft {
                text: "worker reply notification".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(worker),
            12,
            false,
        )
        .unwrap()[0];
    assert!(state.request(worker_request).unwrap().bus_generated);
    let worker_prompt = state.render_request_payload(worker_request).unwrap();
    state.begin_steering(worker_request, lead).unwrap();
    state
        .record_steering(
            worker_request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    let prompt = state.request(lead).unwrap().prompt.rendered_payload();
    assert!(matches!(
        state.accept_callback(ProviderCallback::final_event(
            "decision-final-unclaimed",
            13,
            orch,
            "launch-orch",
            "provider-session",
            "turn-1",
            &prompt,
            DECISION
        )),
        CallbackDisposition::AcceptedCompleted | CallbackDisposition::AcceptedPendingSettlement
    ));
    state.observe_status(orch, RuntimeStatus::Idle, 20).unwrap();
    assert!(state.settle_unclaimed_steering(orch, 21, |_| {
        Some(RecordedTurnEnd::Final {
            turn_id: "turn-1".into(),
            text: DECISION.into(),
            prompts: vec![prompt.clone(), worker_prompt.clone()],
        })
    }));
    let lead_settled = state.request(lead).unwrap();
    assert_eq!(lead_settled.phase, RequestPhase::Completed);
    assert!(lead_settled.reply().is_none());
    let worker_settled = state.request(worker_request).unwrap();
    assert_eq!(worker_settled.phase, RequestPhase::Completed);
    assert!(worker_settled.reply().is_none());
    assert!(worker_settled.failure_reason.is_none());
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        0,
        "turn proof completes the steered request without publishing the captured final"
    );
}

#[test]
fn worker_woken_identical_decisions_from_independent_turns_each_publish() {
    independently_submitted_worker_turns_keep_decisions(DECISION, false);
}

#[test]
fn worker_woken_decision_after_a_silent_turn_is_not_an_earlier_explicit_copy() {
    independently_submitted_worker_turns_keep_decisions("", true);
}

fn independently_submitted_worker_turns_keep_decisions(first_reply: &str, explicit_first: bool) {
    let (mut state, master, worker, orch, first) = worker_woken_turn();
    state
        .observe_status(orch, RuntimeStatus::Working, 12)
        .unwrap();
    let second = state
        .submit_bus_message_with(
            state.request(first).unwrap().room_id,
            Draft {
                text: "independent worker result".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(worker),
            12,
            true,
        )
        .unwrap()[0];
    if explicit_first {
        state
            .post_to_developer(master, orch, DECISION.into(), vec![], 12)
            .unwrap();
    }
    let first_prompt = state.request(first).unwrap().prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "first-independent-final",
            13,
            orch,
            "launch-orch",
            "provider-session",
            "turn-1",
            &first_prompt,
            first_reply,
        )),
        CallbackDisposition::AcceptedCompleted
    );
    state.observe_status(orch, RuntimeStatus::Idle, 14).unwrap();
    state.begin_submission(second, "launch-orch", 14).unwrap();
    state
        .record_submission(
            second,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-2".into()),
            },
        )
        .unwrap();
    let prompt = state.request(second).unwrap().prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "trusted-independent-start".into(),
            sequence: 15,
            occurred_at_ms: 15,
            agent_id: orch,
            launch_id: "launch-orch".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-2".into()),
            provider_prompt_id: None,
            prompt_payload: Some(prompt.clone()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "decision-final-independent",
            16,
            orch,
            "launch-orch",
            "provider-session",
            "turn-2",
            &prompt,
            DECISION
        )),
        CallbackDisposition::AcceptedCompleted
    );
    for id in [first, second] {
        let settled = state.request(id).unwrap();
        assert_eq!(settled.phase, RequestPhase::Completed);
        assert!(settled.reply().is_none());
        assert!(settled.failure_reason.is_none());
    }
    let published = state.notices(master).filter(|p| p.text == DECISION).count();
    if explicit_first {
        assert_eq!(
            published, 1,
            "the explicit post stays once; captured finals do not copy or repeat it"
        );
    } else {
        assert_eq!(
            published, 0,
            "captured finals from separate turns are not published"
        );
    }
}
