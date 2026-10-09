use super::*;

#[test]
fn recover_not_submitted_queued_request_releases_only_that_request() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let failed = submit_text(&mut state, room, agent, "not submitted");
    let next = submit_text(&mut state, room, agent, "next request");
    assert_eq!(
        stall_at(&state, failed, 10 + QUEUED_STALL_MS).as_deref(),
        Some("not_submitted")
    );
    state.agents.get_mut(&agent).unwrap().delivery_rejection = Some("delivery_rejected".into());
    state
        .set_agent_error(agent, Some("prior refusal".into()))
        .unwrap();
    state
        .recover_idle_request(failed, 20)
        .expect("queued not_submitted is recoverable");
    assert_eq!(
        state.request(failed).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(state.request(failed).unwrap().completed_at_ms, Some(20));
    assert_eq!(state.queued_requests(agent), &[next]);
    assert_eq!(state.next_queued_request(agent), Some(next));
    assert_eq!(state.agent(agent).unwrap().delivery_rejection, None);
    assert_eq!(state.agent(agent).unwrap().actionable_error, None);
    assert!(state.room(room).unwrap().latest_replies.is_empty());
}

#[test]
fn recover_queued_request_preserves_another_current_owner() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let current = submit_text(&mut state, room, agent, "current request");
    state.begin_submission(current, "launch-codex", 5).unwrap();
    let queued = submit_text(&mut state, room, agent, "queued request");
    state
        .set_agent_error(agent, Some("current request error".into()))
        .unwrap();
    state.recover_idle_request(queued, 20).unwrap();
    assert_eq!(state.agent(agent).unwrap().current_request, Some(current));
    assert_eq!(
        state.agent(agent).unwrap().actionable_error.as_deref(),
        Some("current request error")
    );
    assert_eq!(
        state.request(current).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        state.request(queued).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(state.queued_requests(agent).is_empty());
}

#[test]
fn recover_not_submitted_request_still_requires_an_idle_agent() {
    for status in [
        RuntimeStatus::Working,
        RuntimeStatus::Blocked,
        RuntimeStatus::Launching,
        RuntimeStatus::Unavailable,
    ] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "not submitted");
        state.observe_status(agent, status, 1).unwrap();
        let before = state.clone();
        assert_eq!(
            state.recover_idle_request(request, 20),
            Err(ModelError::AgentNotIdle)
        );
        assert_eq!(state, before);
    }
}
