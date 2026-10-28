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
