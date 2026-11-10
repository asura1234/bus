use super::*;

#[test]
fn recover_not_submitted_queued_request_releases_only_that_request() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let failed = submit_text(&mut state, room, agent, "not submitted");
    let next = submit_text(&mut state, room, agent, "next request");
    assert_eq!(
        recovery_at(&state, failed, 10 + QUEUED_STALL_MS).as_deref(),
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
    state
        .observe_status(agent, RuntimeStatus::Working, 12)
        .unwrap();
    state
        .observe_dialog(agent, true, state.agent(agent).unwrap().observed_at_ms)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .unwrap();
    let queued = submit_text(&mut state, room, agent, "continue after approval");

    let expired = state
        .expire_stalled_queued_requests(13 + QUEUED_STALL_MS)
        .unwrap();

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
    state
        .observe_status(agent, RuntimeStatus::Working, 12)
        .unwrap();
    let correction = submit_text(&mut state, room, agent, "correction");
    assert_eq!(state.next_steering(agent), Some((correction, lead)));
    state.begin_steering(correction, lead).unwrap();
    state
        .record_steering(
            correction,
            SubmissionOutcome::DefinitelyRejected {
                message: "Agent is not ready; prompt was not sent".into(),
            },
        )
        .unwrap();
    // The original turn finished before the native steering write arrived;
    // no correction was typed, and no final reply was captured.
    state
        .observe_status(agent, RuntimeStatus::Idle, 13)
        .unwrap();

    assert!(
        state.turn_ended(state.request(lead).unwrap()),
        "rejecting a correction must preserve the original request's observed work"
    );
    assert_eq!(
        state.settle_ended_request(agent, 13 + TURN_SETTLE_MS, |_| None),
        Some(lead)
    );
    let settled = state.request(lead).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(settled.reply().is_none());
    assert_eq!(settled.delivery_stage(), "ended");
    assert!(settled.failure_reason.is_none());
    assert_eq!(state.next_queued_request(agent), Some(correction));
}

#[test]
fn uncertain_steering_preserves_native_submission_failure_detail() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let lead = submit_text(&mut state, room, agent, "original task");
    start_request(&mut state, lead, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 12)
        .unwrap();
    let correction = submit_text(&mut state, room, agent, "review reply");
    state.begin_steering(correction, lead).unwrap();
    let native_detail = concat!(
        "Provider did not accept prompt before submission deadline. ",
        "The draft was left because it was edited or could not be attributed to Bus. ",
        "No Enter was sent by Bus. Inspect the agent's transcript and composer before ",
        "resending; preserve your edits while removing any remaining Bus draft"
    );
    state
        .record_steering(
            correction,
            SubmissionOutcome::Uncertain {
                message: native_detail.into(),
            },
        )
        .unwrap();
    assert_eq!(
        state
            .request(correction)
            .unwrap()
            .submission_error
            .as_deref(),
        Some(native_detail),
        "recording uncertain steering must retain the native refusal"
    );

    // The original turn's final does not claim the pasted review reply.
    let payload = state.request(lead).unwrap().prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "original-final",
            20,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            &payload,
            "original task completed",
        )),
        CallbackDisposition::AcceptedPendingSettlement
    );
    let pending = state.request(correction).unwrap();
    assert_eq!(pending.phase, RequestPhase::Submitting);
    assert_eq!(pending.submission_error.as_deref(), Some(native_detail));
    assert!(pending
        .submission_notice()
        .unwrap()
        .ends_with(native_detail));

    state
        .observe_status(agent, RuntimeStatus::Idle, 21)
        .unwrap();
    assert!(state.settle_unclaimed_steering(agent, 21 + TURN_SETTLE_MS, |_| None));
    let settled = state.request(correction).unwrap();
    assert_eq!(settled.phase, RequestPhase::Abandoned);
    assert_eq!(
        settled.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert_eq!(settled.submission_error.as_deref(), Some(native_detail));
    assert!(settled
        .submission_notice()
        .unwrap()
        .ends_with(native_detail));
}

#[test]
fn grok_working_turn_keeps_a_follow_up_queued_instead_of_steering_it() {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Grok, "/scratch".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch-grok".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let lead = submit_text(&mut state, room, agent, "sleep in the foreground");
    start_request(&mut state, lead, "launch-grok", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 12)
        .unwrap();
    let follow = submit_text(&mut state, room, agent, "reply after it exits");

    assert_eq!(state.next_steering(agent), None);
    assert_eq!(state.queued_requests(agent), &[follow]);
    assert_eq!(state.request(follow).unwrap().phase, RequestPhase::Queued);
    assert!(!state.request(follow).unwrap().steered);
}

#[test]
fn invalidated_unavailable_recovery_closes_joined_group_and_preserves_queue() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
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
    let queued = submit_text(&mut state, room, agent, "queued");
    state.invalidate_agent_session(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 3_000)
        .unwrap();
    let recovered_at = 3_000 + DELIVERED_STALL_MS;
    assert_eq!(
        state.recover_idle_request(member, recovered_at),
        Err(ModelError::InvalidTransition)
    );
    state.recover_idle_request(lead, recovered_at).unwrap();
    state.recover_idle_request(lead, recovered_at + 1).unwrap();
    for request in [lead, member] {
        assert_eq!(
            state.request(request).unwrap().phase,
            RequestPhase::Abandoned
        );
        assert_eq!(
            state.request(request).unwrap().completed_at_ms,
            Some(recovered_at)
        );
    }
    assert_eq!(state.request(member).unwrap().group, Some(lead));
    assert!(state.request(member).unwrap().uncertain_outcome);
    assert_eq!(state.queued_requests(agent), &[queued]);
    assert_eq!(state.request(queued).unwrap().phase, RequestPhase::Queued);
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(
        state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    assert!(state.agent(agent).unwrap().session_binding_invalidated);
    assert!(state.room(room).unwrap().latest_replies.is_empty());
}

#[test]
fn typed_recovery_requires_idle_or_both_invalidated_and_unavailable() {
    for phase in [RequestPhase::Submitting, RequestPhase::Active] {
        for invalidated in [false, true] {
            for status in [
                RuntimeStatus::Working,
                RuntimeStatus::Blocked,
                RuntimeStatus::Launching,
                RuntimeStatus::Unavailable,
            ] {
                let (mut state, agent, request) = idle_with_queued(1_000, "go");
                state.begin_submission(request, "launch-codex", 5).unwrap();
                state.requests.get_mut(&request).unwrap().phase = phase;
                if invalidated {
                    state.invalidate_agent_session(agent).unwrap();
                }
                state.observe_status(agent, status, 3_000).unwrap();
                let outcome = state.recover_idle_request(request, 4_000);
                if invalidated && status == RuntimeStatus::Unavailable {
                    assert!(outcome.is_ok(), "{phase:?} {outcome:?}");
                    assert_eq!(
                        state.request(request).unwrap().phase,
                        RequestPhase::Abandoned
                    );
                } else {
                    assert_eq!(outcome, Err(ModelError::AgentNotIdle));
                    assert_eq!(state.request(request).unwrap().phase, phase);
                    assert_eq!(state.agent(agent).unwrap().current_request, Some(request));
                }
            }
        }
    }
}

#[test]
fn queued_request_expired_during_shutdown_preserves_failure_without_chat() {
    // Reopening a session first closes queued work whose deadline passed
    // while Bus was down; the failure remains available through request state.
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let request = submit_text(&mut state, room, agent, "queued before shutdown");
    let reopened_at = 10 + QUEUED_STALL_MS + 60_000;
    let expired = state.expire_stalled_queued_requests(reopened_at).unwrap();
    assert_eq!(
        expired,
        vec![request],
        "the expired request is closed on reopen"
    );
    state
        .observe_status(agent, RuntimeStatus::Unavailable, reopened_at)
        .unwrap();
    let master = state.ensure_master_room();
    let tagged = state
        .notices(master)
        .filter(|notice| crate::messaging::attention::text(&notice.text).is_some())
        .count();
    assert_eq!(tagged, 0, "the failure must not create unsolicited chat");
    let failed = state.request(request).unwrap();
    assert_eq!(failed.phase, RequestPhase::Abandoned);
    assert_eq!(failed.failure_reason.as_deref(), Some("not_submitted"));
    assert_eq!(failed.completed_at_ms, Some(reopened_at));
    assert_eq!(failed.prompt.text, "queued before shutdown");
    assert!(state.queued_requests(agent).is_empty());
}

#[test]
fn an_uncertain_write_the_idle_agent_never_took_fails_and_frees_its_queue() {
    // 17582: a write timeout left the request uncertain, the agent stayed
    // idle with no start hook, and every message behind it expired.
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let typed = submit_text(&mut state, room, agent, "worker final");
    state.begin_submission(typed, "launch-codex", 5).unwrap();
    state
        .record_submission(
            typed,
            SubmissionOutcome::Uncertain {
                message: "Submit outcome unknown; no automatic retry: timeout".into(),
            },
        )
        .unwrap();
    let behind = submit_text(&mut state, room, agent, "blocked notice");

    // Within its bound the uncertain write holds the agent, and the message
    // behind it waits rather than expiring as prior_request_active.
    let bound = 1_000 + UNCONFIRMED_SUBMISSION_MS;
    state.fail_unconfirmed_submissions(bound - 1).unwrap();
    assert!(state
        .expire_stalled_queued_requests(bound - 1)
        .unwrap()
        .is_empty());
    assert_eq!(state.agent(agent).unwrap().current_request, Some(typed));
    assert_eq!(state.queued_requests(agent), &[behind]);

    // At the bound it is declared failed, and the queue moves on.
    state.fail_unconfirmed_submissions(bound).unwrap();
    assert!(state
        .expire_stalled_queued_requests(bound)
        .unwrap()
        .is_empty());
    let failed = state.request(typed).unwrap();
    assert_eq!(failed.phase, RequestPhase::Abandoned);
    assert_eq!(
        failed.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert_eq!(failed.completed_at_ms, Some(bound));
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(state.next_queued_request(agent), Some(behind));
    assert_eq!(state.request(behind).unwrap().phase, RequestPhase::Queued);
}

#[test]
fn an_uncertain_write_is_kept_while_the_agent_works_on_it() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Idle, 1_000)
        .unwrap();
    let typed = submit_text(&mut state, room, agent, "go");
    state.begin_submission(typed, "launch-codex", 5).unwrap();
    state
        .record_submission(
            typed,
            SubmissionOutcome::Uncertain {
                message: "timeout".into(),
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Working, 2_000)
        .unwrap();
    let hour = 60 * 60 * 1000;
    state.fail_unconfirmed_submissions(2_000 + hour).unwrap();
    assert_eq!(
        state.request(typed).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(state.agent(agent).unwrap().current_request, Some(typed));
}

#[test]
fn missing_launch_directory_preserves_busy_queue() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Blocked] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let request = submit_text(&mut state, room, agent, "waiting on work");
        state.observe_status(agent, status, 1).unwrap();
        state
            .observe_working_directory_error(agent, Some("launch directory was removed".into()))
            .unwrap();
        assert_eq!(
            state.agent(agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
        assert!(state.expire_stalled_queued_requests(10 + QUEUED_STALL_MS).unwrap().is_empty(),
            "{status:?}: presentation availability cannot expire work queued behind native activity");
        assert_eq!(state.queued_requests(agent), &[request]);
        assert_eq!(state.request(request).unwrap().phase, RequestPhase::Queued);
    }
}

#[test]
fn restart_stale_idle_owner_never_expires_new_messages_as_prior_request_active() {
    for status in [
        RuntimeStatus::Idle,
        RuntimeStatus::Working,
        RuntimeStatus::Blocked,
        RuntimeStatus::Unavailable,
    ] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state
            .observe_status(agent, RuntimeStatus::Idle, 1_000)
            .unwrap();
        let old = submit_text(&mut state, room, agent, "old request");
        state.begin_submission(old, "launch-codex", 0).unwrap();
        state
            .record_submission(
                old,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("session".into()),
                    provider_turn_id: None,
                },
            )
            .unwrap();
        state.observe_status(agent, status, 1_001).unwrap();
        let first = submit_text(&mut state, room, agent, "resume work");
        let second = submit_text(&mut state, room, agent, "follow up");
        let mut state: BusState =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        assert!(
            state
                .expire_stalled_queued_requests(24 * 60 * 60 * 1_000)
                .unwrap()
                .is_empty(),
            "{status:?}"
        );
        assert_eq!(state.queued_requests(agent), &[first, second]);
        for id in [first, second] {
            assert_eq!(state.request(id).unwrap().phase, RequestPhase::Queued);
            assert!(state.request(id).unwrap().failure_reason.is_none());
        }
    }
}
