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
    (typed, behind, 1_000 + UNCONFIRMED_SUBMISSION_MS)
}

fn tagged_master_notices(state: &mut BusState) -> usize {
    let master = state.ensure_master_room();
    state
        .notices(master)
        .filter(|notice| crate::messaging::attention::text(&notice.text).is_some())
        .count()
}

fn assert_unknown_outcome_failure(state: &BusState, request: RequestId) {
    let request = state.request(request).unwrap();
    assert_eq!(request.phase, RequestPhase::Abandoned);
    assert_eq!(
        request.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(request.uncertain_outcome);
    assert!(request.pending_final.is_none());
}

#[test]
fn swallowed_enter_releases_idle_claude_queue_five_seconds_after_the_write() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::ClaudeCode;
    let (typed, behind, _) = uncertain_write(&mut state, room, agent);
    state
        .record_native_timestamps(
            typed,
            NativeSubmissionTimestamps {
                enter_written_at_ms: Some(2_000),
                ..Default::default()
            },
        )
        .unwrap();
    // Repeated idle observations must not renew the confirmation deadline.
    for observed_at in [3_000, 5_000, 6_999] {
        state
            .observe_status(agent, RuntimeStatus::Idle, observed_at)
            .unwrap();
        state.fail_unconfirmed_submissions(observed_at).unwrap();
        assert_eq!(state.agent(agent).unwrap().current_request, Some(typed));
        assert_eq!(state.request(behind).unwrap().phase, RequestPhase::Queued);
    }
    state.fail_unconfirmed_submissions(7_000).unwrap();
    assert_unknown_outcome_failure(&state, typed);
    assert_eq!(state.next_queued_request(agent), Some(behind));
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert!(state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap()
        .contains("submission_unconfirmed"));
    let restored: BusState = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_unknown_outcome_failure(&restored, typed);
    assert_eq!(restored.next_queued_request(agent), Some(behind));
}

#[test]
fn a_failed_unconfirmed_write_preserves_its_unknown_outcome_without_chat() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, _, bound) = uncertain_write(&mut state, room, agent);

    // An open uncertain write stays open until its own recovery deadline.
    let mut open = state.clone();
    assert_eq!(open.request(typed).unwrap().phase, RequestPhase::Submitting);
    assert!(open.request(typed).unwrap().uncertain_outcome);
    assert_eq!(tagged_master_notices(&mut open), 0);

    // The coordinator's expiry order: fail the write, then the queued sweep.
    state.fail_unconfirmed_submissions(bound).unwrap();
    state.expire_stalled_queued_requests(bound).unwrap();
    assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Abandoned);
    assert_eq!(
        tagged_master_notices(&mut state),
        0,
        "the unknown outcome belongs in request and agent state, not unsolicited chat"
    );
    assert_unknown_outcome_failure(&state, typed);
    state.fail_unconfirmed_submissions(bound + 1).unwrap();
    state.expire_stalled_queued_requests(bound + 1).unwrap();
    assert_eq!(tagged_master_notices(&mut state), 0);
    assert_unknown_outcome_failure(&state, typed);
}

#[test]
fn recording_an_unconfirmed_recovery_reason_preserves_the_later_failure_without_chat() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, behind, bound) = uncertain_write(&mut state, room, agent);
    assert_eq!(
        recovery_at(&state, typed, bound).as_deref(),
        Some("submission_unconfirmed")
    );
    assert_eq!(tagged_master_notices(&mut state), 0);
    state.fail_unconfirmed_submissions(bound).unwrap();
    state.expire_stalled_queued_requests(bound).unwrap();
    assert_unknown_outcome_failure(&state, typed);
    assert_eq!(state.next_queued_request(agent), Some(behind));
    assert_eq!(tagged_master_notices(&mut state), 0);
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
        .fail_unconfirmed_submissions(1_000 + UNCONFIRMED_SUBMISSION_MS)
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
    let later = 3_000 + UNCONFIRMED_SUBMISSION_MS;
    state.fail_unconfirmed_submissions(later).unwrap();
    assert!(state
        .expire_stalled_queued_requests(later)
        .unwrap()
        .is_empty());
    assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Abandoned);
    assert_eq!(state.next_queued_request(agent), Some(behind));
    assert_unknown_outcome_failure(&state, typed);
    assert_eq!(tagged_master_notices(&mut state), 0);
}

#[test]
fn an_unconfirmed_write_waits_after_its_idle_dialog_closes() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, behind, bound) = uncertain_write(&mut state, room, agent);
    // Codex reports Idle while an approval dialog blocks its work.
    state
        .observe_dialog(agent, true, state.agent(agent).unwrap().observed_at_ms)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, bound)
        .unwrap();
    state.fail_unconfirmed_submissions(bound).unwrap();
    assert_eq!(
        state.request(typed).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        state.agent(agent).unwrap().shown_status(),
        RuntimeStatus::Blocked
    );

    // Closing that block begins the agent's usable idle period; time spent
    // waiting for the developer must not consume its entire idle grace.
    let resumed_at = bound + 1_000;
    state.observe_dialog(agent, false, resumed_at).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, resumed_at)
        .unwrap();
    state.fail_unconfirmed_submissions(resumed_at).unwrap();
    assert_eq!(
        state.request(typed).unwrap().phase,
        RequestPhase::Submitting,
        "a dialog-blocked agent must receive the same fresh idle grace as a status-blocked one"
    );
    assert_eq!(state.request(behind).unwrap().phase, RequestPhase::Queued);
    state
        .fail_unconfirmed_submissions(resumed_at + UNCONFIRMED_SUBMISSION_MS - 1)
        .unwrap();
    assert_eq!(
        state.request(typed).unwrap().phase,
        RequestPhase::Submitting
    );
    state
        .fail_unconfirmed_submissions(resumed_at + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Abandoned);
}

#[test]
fn unconfirmed_submission_idle_dialog_bound_uses_the_closure_observation() {
    for status_first in [false, true] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let (typed, _, bound) = uncertain_write(&mut state, room, agent);
        state
            .observe_dialog(agent, true, state.agent(agent).unwrap().observed_at_ms)
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Idle, bound)
            .unwrap();
        let closed_at = bound + 1_000;
        if status_first {
            state
                .observe_status(agent, RuntimeStatus::Idle, closed_at)
                .unwrap();
        }
        state.observe_dialog(agent, false, closed_at).unwrap();
        if !status_first {
            state
                .observe_status(agent, RuntimeStatus::Idle, closed_at)
                .unwrap();
        }
        state.fail_unconfirmed_submissions(closed_at).unwrap();
        assert_eq!(
            state.request(typed).unwrap().phase,
            RequestPhase::Submitting
        );
        // Repeated closed observations must not keep extending the grace.
        state
            .observe_dialog(agent, false, closed_at + 1_000)
            .unwrap();
        state
            .fail_unconfirmed_submissions(closed_at + UNCONFIRMED_SUBMISSION_MS - 1)
            .unwrap();
        assert_eq!(
            state.request(typed).unwrap().phase,
            RequestPhase::Submitting
        );
        state
            .fail_unconfirmed_submissions(closed_at + UNCONFIRMED_SUBMISSION_MS)
            .unwrap();
        assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Abandoned);
    }
}

#[test]
fn restored_unconfirmed_write_waits_for_fresh_idle_observation_and_bound() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, behind, bound) = uncertain_write(&mut state, room, agent);
    let encoded = serde_json::to_vec(&state).unwrap();
    let mut restored: BusState = serde_json::from_slice(&encoded).unwrap();
    // Worker::open invalidates all restored runtime observations in this way.
    let restarted_at = bound + 60_000;
    restored
        .observe_status(agent, RuntimeStatus::Unavailable, restarted_at)
        .unwrap();
    restored
        .fail_unconfirmed_submissions(restarted_at + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    assert_eq!(
        restored.request(typed).unwrap().phase,
        RequestPhase::Submitting
    );
    let idle_at = restarted_at + UNCONFIRMED_SUBMISSION_MS + 1;
    restored
        .observe_status(agent, RuntimeStatus::Idle, idle_at)
        .unwrap();
    restored
        .fail_unconfirmed_submissions(idle_at + UNCONFIRMED_SUBMISSION_MS - 1)
        .unwrap();
    assert_eq!(
        restored.request(typed).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        restored.request(behind).unwrap().phase,
        RequestPhase::Queued
    );
    restored
        .fail_unconfirmed_submissions(idle_at + UNCONFIRMED_SUBMISSION_MS)
        .unwrap();
    assert_eq!(
        restored.request(typed).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(restored.next_queued_request(agent), Some(behind));
    assert_eq!(tagged_master_notices(&mut restored), 0);
    assert_unknown_outcome_failure(&restored, typed);

    let encoded = serde_json::to_vec(&restored).unwrap();
    let mut restored_again: BusState = serde_json::from_slice(&encoded).unwrap();
    restored_again
        .fail_unconfirmed_submissions(idle_at + UNCONFIRMED_SUBMISSION_MS + 1)
        .unwrap();
    assert_eq!(restored_again.next_queued_request(agent), Some(behind));
    assert_eq!(tagged_master_notices(&mut restored_again), 0);
    assert_unknown_outcome_failure(&restored_again, typed);
}

#[test]
fn unconfirmed_settlement_preserves_each_native_cleanup_outcome() {
    for detail in [
        "The remaining draft was cleared",
        "The draft was left because it was edited",
    ] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let (typed, _, bound) = uncertain_write(&mut state, room, agent);
        state
            .record_submission(
                typed,
                SubmissionOutcome::Uncertain {
                    message: detail.into(),
                },
            )
            .unwrap();
        state.fail_unconfirmed_submissions(bound).unwrap();
        let restored: BusState =
            serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
        let error = restored
            .agent(agent)
            .unwrap()
            .actionable_error
            .as_deref()
            .unwrap();
        for expected in [detail, "submission_unconfirmed", "before resending"] {
            assert!(error.contains(expected), "missing {expected}: {error}");
        }
    }
}

/// Replays live request 11513 (2026-11-13): Claude dropped the Enter after an
/// image paste, yet Bus recorded the write as confirmed. The idle agent never
/// went busy and no submit hook arrived.
fn falsely_confirmed_write(
    state: &mut BusState,
    room: RoomId,
    agent: AgentId,
    provider: Provider,
) -> (RequestId, RequestId) {
    state.agents.get_mut(&agent).unwrap().provider = provider;
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let typed = submit_text(state, room, agent, "grey square\n\"/tmp/IMG_0055.PNG\"");
    state.begin_submission(typed, "launch-codex", 5).unwrap();
    state
        .record_native_timestamps(
            typed,
            NativeSubmissionTimestamps {
                enter_written_at_ms: Some(2_000),
                ..Default::default()
            },
        )
        .unwrap();
    state
        .record_submission(
            typed,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Active);
    let behind = submit_text(state, room, agent, "filter MR by author");
    (typed, behind)
}

#[test]
fn a_confirmed_write_the_idle_agent_never_started_fails_visibly_after_the_bound() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        for restart in [false, true] {
            let (mut state, room, agent, _) = state_with_room_and_agents();
            let (typed, behind) = falsely_confirmed_write(&mut state, room, agent, provider);
            let bound = 2_000 + UNCONFIRMED_SUBMISSION_MS;
            state
                .observe_status(agent, RuntimeStatus::Idle, bound - 1)
                .unwrap();
            state.fail_unconfirmed_submissions(bound - 1).unwrap();
            assert_eq!(state.request(typed).unwrap().phase, RequestPhase::Active);
            if restart {
                state = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
            }
            assert_eq!(
                recovery_at(&state, typed, bound).as_deref(),
                Some("submission_unconfirmed"),
                "{provider:?}: message status must name the stall"
            );
            state.fail_unconfirmed_submissions(bound).unwrap();
            let failed = state.request(typed).unwrap();
            assert_eq!(failed.phase, RequestPhase::Abandoned, "{provider:?}");
            assert_eq!(
                failed.failure_reason.as_deref(),
                Some("submission_unconfirmed")
            );
            assert!(
                state
                    .agent(agent)
                    .unwrap()
                    .actionable_error
                    .as_deref()
                    .is_some_and(|error| error.contains("before resending")),
                "{provider:?}: the unsent composer text must not be silent"
            );
            assert_eq!(state.next_queued_request(agent), Some(behind));
        }
    }
}

#[test]
fn a_confirmed_write_keeps_its_turn_when_work_hooks_or_dialogs_explain_the_wait() {
    let far = 2_000 + 100 * UNCONFIRMED_SUBMISSION_MS;
    for case in [
        "went_busy",
        "hooks_unconfirmed",
        "kimi",
        "dialog",
        "start_bound",
    ] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let provider = if case == "kimi" {
            Provider::Kimi
        } else {
            Provider::ClaudeCode
        };
        let (typed, _) = falsely_confirmed_write(&mut state, room, agent, provider);
        match case {
            "went_busy" => {
                state
                    .observe_status(agent, RuntimeStatus::Working, 2_500)
                    .unwrap();
                state
                    .observe_status(agent, RuntimeStatus::Idle, 3_000)
                    .unwrap();
            }
            "hooks_unconfirmed" => {
                state.agents.get_mut(&agent).unwrap().hook_setup_confirmed = false
            }
            "dialog" => state.agents.get_mut(&agent).unwrap().dialog = true,
            "start_bound" => {
                state.requests.get_mut(&typed).unwrap().trusted_start_bound = true;
            }
            _ => {}
        }
        state.fail_unconfirmed_submissions(far).unwrap();
        assert_ne!(
            state.request(typed).unwrap().failure_reason.as_deref(),
            Some("submission_unconfirmed"),
            "{case}"
        );
    }
}

fn own_turn_callback(agent: AgentId, sequence: u64, kind: CallbackEventKind) -> ProviderCallback {
    ProviderCallback {
        callback_id: format!("own-turn-{sequence}"),
        sequence,
        occurred_at_ms: sequence * 1_000,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("own-turn".into()),
        provider_prompt_id: None,
        prompt_payload: Some("<task-notification>background command completed".into()),
        kind,
    }
}

#[test]
fn a_turn_of_the_agents_own_does_not_take_steering_meant_for_an_unstarted_write() {
    // Live 2026-11-13: a Claude task notification made the agent Working, so
    // 11518 was steered into unstarted 11513 and abandoned with it although
    // the model answered it. It must wait, and the release must be visible.
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let (typed, behind) = falsely_confirmed_write(&mut state, room, agent, Provider::ClaudeCode);
    assert_eq!(
        state.accept_callback(own_turn_callback(
            agent,
            6,
            CallbackEventKind::PromptStarted
        )),
        CallbackDisposition::Rejected(CallbackRejection::UnrelatedTurn)
    );
    assert!(state
        .request(typed)
        .unwrap()
        .foreign_turn_started_at_ms
        .is_some());
    state
        .observe_status(agent, RuntimeStatus::Working, 6_001)
        .unwrap();
    assert_eq!(
        state.next_steering(agent),
        None,
        "a message steered into the unstarted write would share its abandonment"
    );
    state.accept_callback(own_turn_callback(
        agent,
        7,
        CallbackEventKind::Final {
            text: "background check done".into(),
        },
    ));
    state
        .observe_status(agent, RuntimeStatus::Idle, 7_001)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 7_000 + UNBOUND_SETTLE_MS)
        .unwrap();
    let released = state.request(typed).unwrap();
    assert_eq!(released.phase, RequestPhase::Abandoned);
    assert_eq!(
        released.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .is_some_and(|error| error.contains("before resending")));
    assert_eq!(state.request(behind).unwrap().phase, RequestPhase::Queued);
    assert_eq!(state.next_queued_request(agent), Some(behind));
}
