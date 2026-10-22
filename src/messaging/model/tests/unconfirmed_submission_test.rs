use super::*;

/// An idle agent with `typed` written but its outcome unknown, and `behind`
/// queued after it; returns the bound at which the write is failed.
fn uncertain_write(
    state: &mut BusState,
    room: RoomId,
    agent: AgentId,
) -> (RequestId, RequestId, u64) {
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let typed = submit_text(state, room, agent, "worker final");
    state.begin_submission(typed, "launch-codex", 5).unwrap();
    state
        .record_submission(
            typed,
            SubmissionOutcome::Uncertain {
                message: "Submit outcome unknown; no automatic retry: timeout".into(),
            },
        )
        .unwrap();
    let behind = submit_text(state, room, agent, "blocked notice");
    (typed, behind, 1_000 + DELIVERED_STALL_MS)
}

fn tagged_master_notices(state: &mut BusState) -> usize {
    let master = state.ensure_master_room();
    state
        .notices(master)
        .filter(|notice| crate::messaging::attention::text(&notice.text).is_some())
        .count()
}

fn assert_unknown_outcome_notice(state: &BusState) {
    let master = state.master_room().unwrap();
    let notice = state
        .notices(master.id)
        .find(|notice| notice.text.contains("submission_unconfirmed"))
        .unwrap();
    assert!(notice.text.contains("is unknown"));
    assert!(notice.text.contains("may have run"));
    assert!(notice.text.contains("before resending"));
}

#[test]
fn a_failed_unconfirmed_write_still_tells_the_developer() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, _, bound) = uncertain_write(&mut state, room, agent);

    // Left open, the write past its bound raises developer attention.
    let mut open = state.clone();
    open.notify_stalled_attention(bound).unwrap();
    assert_eq!(
        tagged_master_notices(&mut open),
        1,
        "baseline: open stall alerts"
    );

    // The coordinator's expiry order: fail the write, then the queued sweep.
    state.fail_unconfirmed_submissions(bound).unwrap();
    state.expire_stalled_queued_requests(bound).unwrap();
    assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Abandoned);
    assert_eq!(
        tagged_master_notices(&mut state),
        1,
        "Bus closed a message it never confirmed; the developer must learn its outcome is unknown"
    );
    assert_unknown_outcome_notice(&state);
    state.fail_unconfirmed_submissions(bound + 1).unwrap();
    state.expire_stalled_queued_requests(bound + 1).unwrap();
    assert_eq!(tagged_master_notices(&mut state), 1);
}

#[test]
fn an_unconfirmed_write_already_reported_is_not_reported_twice() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (_, _, bound) = uncertain_write(&mut state, room, agent);
    state.notify_stalled_attention(bound).unwrap();
    assert_unknown_outcome_notice(&state);
    state.fail_unconfirmed_submissions(bound).unwrap();
    state.expire_stalled_queued_requests(bound).unwrap();
    assert_eq!(tagged_master_notices(&mut state), 1);
}

#[test]
fn a_failed_unconfirmed_write_leaves_the_agent_an_actionable_error() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (_, _, bound) = uncertain_write(&mut state, room, agent);
    assert!(state.agent(agent).unwrap().actionable_error.is_some());
    state.fail_unconfirmed_submissions(bound).unwrap();
    assert!(
        state.agent(agent).unwrap().actionable_error.is_some(),
        "the failed write should stay visible on the agent, as a closed queued one does"
    );
    let error = state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_ref()
        .unwrap();
    assert!(error.contains("is unknown"));
    assert!(error.contains("may have run"));
    assert!(error.contains("before resending"));
}

#[test]
fn coalesced_messages_failed_with_their_unconfirmed_lead_carry_its_reason() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let lead = submit_text(&mut state, room, agent, "first");
    let member = submit_text(&mut state, room, agent, "second");
    assert!(state.coalesce_queue(lead).is_some());
    state.begin_submission(lead, "launch-codex", 5).unwrap();
    state
        .record_submission(
            lead,
            SubmissionOutcome::Uncertain {
                message: "timeout".into(),
            },
        )
        .unwrap();
    state
        .fail_unconfirmed_submissions(1_000 + DELIVERED_STALL_MS)
        .unwrap();
    let joined = state.request(member).unwrap();
    assert_eq!(joined.phase, RequestPhase::Abandoned);
    assert_eq!(
        joined.failure_reason.as_deref(),
        Some("submission_unconfirmed"),
        "the joined message's sender sees why it failed"
    );
}

#[test]
fn an_unconfirmed_write_waits_its_full_bound_after_the_agent_flaps_idle() {
    // The bound counts from the latest idle start, not the write.
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, behind, bound) = uncertain_write(&mut state, room, agent);
    state
        .observe_status(agent, RuntimeStatus::Working, 2_000)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 3_000)
        .unwrap();
    state.fail_unconfirmed_submissions(bound).unwrap();
    assert!(state
        .expire_stalled_queued_requests(bound)
        .unwrap()
        .is_empty());
    assert_eq!(
        state.request(typed).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(state.request(behind).unwrap().phase, RequestPhase::Queued);
    assert_eq!(tagged_master_notices(&mut state), 0);
    let later = 3_000 + DELIVERED_STALL_MS;
    state.fail_unconfirmed_submissions(later).unwrap();
    assert!(state
        .expire_stalled_queued_requests(later)
        .unwrap()
        .is_empty());
    assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Abandoned);
    assert_eq!(state.next_queued_request(agent), Some(behind));
    assert_unknown_outcome_notice(&state);
}
