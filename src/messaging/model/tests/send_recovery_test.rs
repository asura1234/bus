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
    state
        .observe_status(agent, RuntimeStatus::Working, 6)
        .unwrap();
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
fn recover_not_submitted_request_is_safe_while_the_agent_is_not_idle() {
    for status in [
        RuntimeStatus::Working,
        RuntimeStatus::Blocked,
        RuntimeStatus::Launching,
        RuntimeStatus::Unavailable,
    ] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "not submitted");
        state.observe_status(agent, status, 1).unwrap();
        state
            .recover_idle_request(request, 20)
            .expect("nothing was typed; queued recovery is safe");
        assert_eq!(
            state.request(request).unwrap().phase,
            RequestPhase::Abandoned
        );
        assert!(state.queued_requests(agent).is_empty());
        assert_eq!(state.agent(agent).unwrap().status, status);
    }
}

#[test]
fn queued_expiry_preserves_busy_work_and_captures_each_rejection_reason() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Blocked] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "waiting on work");
        state.observe_status(agent, status, 1).unwrap();
        assert!(state
            .expire_stalled_queued_requests(10 + QUEUED_STALL_MS)
            .unwrap()
            .is_empty());
        assert_eq!(state.queued_requests(agent), &[request]);
    }

    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let lead = submit_text(&mut state, room, agent, "lead");
    let member = submit_text(&mut state, room, agent, "member");
    assert!(state.coalesce_queue(lead).is_some());
    state.begin_submission(lead, "launch-codex", 5).unwrap();
    state
        .record_submission(
            lead,
            SubmissionOutcome::DefinitelyRejected {
                message: "input box is not empty".into(),
            },
        )
        .unwrap();
    assert_eq!(state.queued_requests(agent), &[lead, member]);
    let expired = state
        .expire_stalled_queued_requests(10 + QUEUED_STALL_MS)
        .unwrap();
    assert_eq!(expired, vec![lead, member]);
    assert!(state.queued_requests(agent).is_empty());
    for id in expired {
        let request = state.request(id).unwrap();
        assert_eq!(request.phase, RequestPhase::Abandoned);
        assert_eq!(
            request.failure_reason.as_deref(),
            Some("input_box_not_empty")
        );
        let reason = request.failure_reason.clone();
        state
            .recover_idle_request(id, 20 + QUEUED_STALL_MS)
            .unwrap();
        assert_eq!(state.request(id).unwrap().failure_reason, reason);
    }
}

#[test]
fn queued_expiry_preserves_messages_behind_an_idle_codex_approval_dialog() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.confirm_hook_setup(agent).unwrap();
    let current = submit_text(&mut state, room, agent, "run the requested command");
    start_request(&mut state, current, "launch-codex", 10);
    state.observe_status(agent, RuntimeStatus::Working, 12).unwrap();
    state.observe_dialog(agent, true).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 13).unwrap();
    let queued = submit_text(&mut state, room, agent, "continue after approval");

    let expired = state.expire_stalled_queued_requests(13 + QUEUED_STALL_MS).unwrap();

    assert!(
        expired.is_empty(),
        "an agent waiting on a visible approval is blocked, so its queued work must not expire"
    );
    assert_eq!(state.queued_requests(agent), &[queued]);
    assert_eq!(state.request(queued).unwrap().phase, RequestPhase::Queued);
    assert_eq!(state.agent(agent).unwrap().current_request, Some(current));
}

#[test]
fn a_rejected_steering_write_preserves_the_original_turn_end_evidence() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let lead = submit_text(&mut state, room, agent, "original task");
    start_request(&mut state, lead, "launch-codex", 10);
    state.observe_status(agent, RuntimeStatus::Working, 12).unwrap();
    let correction = submit_text(&mut state, room, agent, "correction");
    assert_eq!(state.next_steering(agent), Some((correction, lead)));
    state.begin_steering(correction, lead).unwrap();
    state.record_steering(correction, SubmissionOutcome::DefinitelyRejected {
        message: "Agent is not ready; prompt was not sent".into(),
    }).unwrap();
    // The original turn finished before the native steering write arrived;
    // no correction was typed, and no final reply was captured.
    state.observe_status(agent, RuntimeStatus::Idle, 13).unwrap();

    assert!(
        state.turn_ended(state.request(lead).unwrap()),
        "rejecting a correction must preserve the original request's observed work"
    );
    assert_eq!(state.settle_ended_request(agent, 13 + TURN_SETTLE_MS), Some(lead));
    assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Abandoned);
    assert_eq!(state.next_queued_request(agent), Some(correction));
}
