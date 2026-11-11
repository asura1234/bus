use super::*;
use crate::messaging::model::{AgentRuntimeIdentity, ExitSnapshot, SubmissionOutcome};
use std::path::PathBuf;

fn bound_kimi_request() -> (BusState, AgentId, RequestId) {
    let mut state = BusState::new();
    let room = state.create_room("test").unwrap();
    let agent = state
        .create_agent(
            room,
            "worker",
            Provider::Kimi,
            PathBuf::from("/removed"),
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
    state.set_draft_text(room, "finish work").unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let id = state.submit_draft(room, 10).unwrap()[0];
    state.begin_submission(id, "launch", 10).unwrap();
    state
        .record_submission(
            id,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("1".into()),
            },
        )
        .unwrap();
    state.requests.get_mut(&id).unwrap().trusted_start_bound = true;
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    state
        .fail_missing_launch_directory(agent, "launch directory removed".into(), 30)
        .unwrap();
    (state, agent, id)
}

#[test]
fn missing_directory_keeps_kimi_live_wire_completion() {
    let (mut state, agent, id) = bound_kimi_request();
    assert_eq!(state.settle_ended_request(agent, 40, |_| None), None);
    assert_eq!(state.request(id).unwrap().phase, RequestPhase::Active);
    assert_eq!(
        state.settle_ended_request(agent, 50, |_| Some(RecordedTurnEnd::NoReply)),
        Some(id)
    );
    assert_eq!(state.request(id).unwrap().phase, RequestPhase::Completed);
    assert!(state.agent(agent).unwrap().current_request.is_none());
    assert!(state.request(id).unwrap().reply().is_none());
}

#[test]
fn missing_directory_proved_kimi_exit_retires_an_existing_retry_hold() {
    let (mut state, agent, id) = bound_kimi_request();
    let error = || {
        Some(RecordedTurnEnd::Error(
            "HTTP 503 Service Unavailable".into(),
        ))
    };
    assert_eq!(state.settle_ended_request(agent, 40, |_| error()), Some(id));
    assert_eq!(state.request(id).unwrap().phase, RequestPhase::Active);
    assert!(state.agent(agent).unwrap().provider_retry.is_some());
    state
        .record_exit_snapshot(
            agent,
            ExitSnapshot {
                launch_id: Some("launch".into()),
                terminal_id: "terminal".into(),
                path: None,
                error: Some("native process exited".into()),
                tail: String::new(),
                at_ms: 100,
                terminal_closed: Some(true),
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 100)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(agent, 101 + TURN_SETTLE_MS, |_| error()),
        Some(id)
    );
    assert_eq!(state.request(id).unwrap().phase, RequestPhase::Abandoned);
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
    assert!(state.agent(agent).unwrap().current_request.is_none());
}
