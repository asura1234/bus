use super::*;

fn refused_once(message: &str, busy: RuntimeStatus) -> (BusState, AgentId, RequestId) {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            room,
            "recipient",
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
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let head = state
        .submit_message_from(
            room,
            Draft {
                text: "developer message".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            1_000,
        )
        .unwrap()[0];
    state.begin_submission(head, "launch", 0).unwrap();
    // The pane changed between Bus's Idle view and the paste.
    state.observe_status(agent, busy, 1_010).unwrap();
    state
        .record_submission(
            head,
            SubmissionOutcome::DefinitelyRejected {
                message: message.into(),
            },
        )
        .unwrap();
    (state, agent, head)
}

#[test]
fn a_turn_that_starts_during_one_refused_paste_keeps_the_message_queued_through_a_long_turn() {
    let (mut state, agent, head) = refused_once(
        "Agent is not idle; prompt was not sent",
        RuntimeStatus::Working,
    );
    let later = 1_010 + QUEUED_STALL_MS + 60_000;
    assert!(state
        .expire_stalled_queued_requests(later)
        .unwrap()
        .is_empty());
    assert_eq!(state.queued_requests(agent), &[head]);
    assert_eq!(state.request(head).unwrap().failure_reason, None);
}

#[test]
fn a_not_idle_refusal_stays_queued_past_the_stall_and_closes_with_that_cause() {
    let (mut state, agent, head) = refused_once(
        "Agent is not idle; prompt was not sent",
        RuntimeStatus::Working,
    );
    // Saturation: the turn ends and the agent looks free, but delivery does not
    // retry before the stall clock. The refusal must not become not_submitted.
    state
        .observe_status(agent, RuntimeStatus::Idle, 2_000)
        .unwrap();
    state.agents.get_mut(&agent).unwrap().delivery_rejection = None;
    let later = 2_000 + QUEUED_STALL_MS + 1;
    assert!(
        state
            .expire_stalled_queued_requests(later)
            .unwrap()
            .is_empty(),
        "a live agent keeps a busy refusal"
    );
    assert_eq!(state.queued_requests(agent), &[head]);
    assert_eq!(state.request(head).unwrap().phase, RequestPhase::Queued);
    assert!(state.request(head).unwrap().failure_reason.is_none());
    assert_eq!(
        state
            .request(head)
            .unwrap()
            .queued_rejection
            .as_ref()
            .map(|rejection| rejection.reason.as_str()),
        Some("agent_not_idle")
    );

    state
        .record_exit_snapshot(
            agent,
            ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("process exited".into()),
                tail: String::new(),
                at_ms: later,
                terminal_closed: Some(true),
            },
        )
        .unwrap();
    let expired = state.expire_stalled_queued_requests(later).unwrap();
    assert_eq!(expired, vec![head]);
    assert_eq!(
        state.request(head).unwrap().failure_reason.as_deref(),
        Some("agent_not_idle")
    );
}

#[test]
fn an_orchestrator_not_idle_refusal_stays_queued_past_the_stall() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(
            master,
            "bus master",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    state
        .set_agent_runtime_identity(
            orchestrator,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(orchestrator).unwrap();
    state
        .observe_status(orchestrator, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let head = state
        .submit_message_from(
            master,
            Draft {
                text: "breaker report".into(),
                files: vec![],
                recipient_ids: [orchestrator].into(),
            },
            Author::Agent(orchestrator),
            1_000,
        )
        .unwrap()[0];
    state.begin_submission(head, "launch", 0).unwrap();
    state
        .observe_status(orchestrator, RuntimeStatus::Working, 1_010)
        .unwrap();
    state
        .record_submission(
            head,
            SubmissionOutcome::DefinitelyRejected {
                message: "Agent is not idle; prompt was not sent".into(),
            },
        )
        .unwrap();
    state
        .observe_status(orchestrator, RuntimeStatus::Idle, 2_000)
        .unwrap();
    let later = 2_000 + QUEUED_STALL_MS + 60_000;
    assert!(state
        .expire_stalled_queued_requests(later)
        .unwrap()
        .is_empty());
    assert_eq!(state.queued_requests(orchestrator), &[head]);
    assert_eq!(state.request(head).unwrap().phase, RequestPhase::Queued);
    assert!(state.request(head).unwrap().failure_reason.is_none());
}

#[test]
fn a_dialog_that_appears_during_one_refused_paste_keeps_the_message_queued_until_answered() {
    let (mut state, agent, head) = refused_once(
        "agent recipient is blocked and requires interactive input",
        RuntimeStatus::Blocked,
    );
    let later = 1_010 + QUEUED_STALL_MS + 60_000;
    assert!(state
        .expire_stalled_queued_requests(later)
        .unwrap()
        .is_empty());
    assert_eq!(state.queued_requests(agent), &[head]);
    assert_eq!(state.request(head).unwrap().failure_reason, None);
}
