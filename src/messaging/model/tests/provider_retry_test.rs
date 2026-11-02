use super::*;

fn failed_turn(provider: Provider, message: &str) -> (BusState, RoomId, AgentId, RequestId) {
    failed_turn_at(provider, message, 100)
}

fn failed_turn_at(
    provider: Provider,
    message: &str,
    now: u64,
) -> (BusState, RoomId, AgentId, RequestId) {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = provider;
    state
        .agents
        .get_mut(&agent)
        .unwrap()
        .runtime_identity
        .session_id = Some("provider-session".into());
    let request = submit_text(&mut state, room, agent, "finish the owned task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Idle, now)
        .unwrap();
    state.accept_callback(ProviderCallback {
        callback_id: "native-failure".into(),
        sequence: 20,
        occurred_at_ms: now,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: None,
        kind: CallbackEventKind::Error {
            message: "Provider turn failed".into(),
            provider_error: Some(message.into()),
        },
    });
    (state, room, agent, request)
}

#[test]
fn provider_flake_holds_recorded_real_errors_open_and_working_across_restart() {
    // Removing transient classification or falling through to abandonment loses a delivery.
    for (provider, error) in [
        (Provider::Codex, "Selected model is at capacity. Please try a different model. (server_overloaded)"),
        (Provider::ClaudeCode, "You've hit your 5-hour limit · resets 12:10am (Asia/Singapore) (rate_limit)"),
        (Provider::ClaudeCode, "You've hit your weekly limit · resets 2am (Asia/Singapore) (rate_limit)"),
        (Provider::ClaudeCode, "API Error: Connection lost mid-response. The response above may be incomplete. (server_error)"),
    ] {
        let (state, room, agent, request) = failed_turn(provider, error);
        assert!(!state.request(request).unwrap().settled(), "{error}");
        assert_eq!(state.agent(agent).unwrap().current_request, Some(request));
        assert_eq!(state.agent(agent).unwrap().shown_status(), RuntimeStatus::Working);
        assert_eq!(state.agent(agent).unwrap().activity_status(), RuntimeStatus::Idle);
        assert!(state.agent(agent).unwrap().actionable_error.is_none());
        assert!(state.recovery_reason(state.request(request).unwrap(), u64::MAX).is_none());
        assert!(state.room(room).unwrap().latest_replies.is_empty());
        let restarted: BusState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(restarted, state);
        assert_eq!(restarted.agent(agent).unwrap().shown_status(), RuntimeStatus::Working);
    }
}

#[test]
fn provider_flake_does_not_reclassify_permanent_or_cancelled_errors() {
    // Treating these as transient would silently retry invalid work or undo cancellation.
    for (provider, error) in [
        (Provider::Codex, "Invalid model (invalid_request)"),
        (
            Provider::ClaudeCode,
            "API Error: 401 Authentication failed (authentication_error)",
        ),
        (Provider::ClaudeCode, "Permission denied (permission_error)"),
        (Provider::Grok, "user_cancelled"),
        (Provider::Kimi, "Kimi turn interrupted"),
        (Provider::Cursor, "Cursor turn error; inspect its terminal."),
        (Provider::Codex, "unclassified provider failure"),
    ] {
        let (state, _, _, request) = failed_turn(provider, error);
        assert!(state.request(request).unwrap().settled(), "{error}");
    }
}

#[test]
fn provider_flake_explicit_overload_and_429_are_held_for_grok_and_kimi() {
    for provider in [Provider::Grok, Provider::Kimi] {
        for error in [
            "API Error: 429 Too Many Requests",
            "Model overloaded (server_overloaded)",
        ] {
            let (state, _, agent, request) = failed_turn(provider, error);
            assert!(!state.request(request).unwrap().settled());
            assert_eq!(
                state.agent(agent).unwrap().shown_status(),
                RuntimeStatus::Working
            );
        }
    }
}

#[test]
fn provider_flake_retries_the_original_request_only_after_backoff_and_rejects_the_old_turn() {
    let (mut state, _, agent, request) =
        failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
    let due = state
        .agent(agent)
        .unwrap()
        .provider_retry
        .as_ref()
        .unwrap()
        .retry_at_ms;
    assert!((5_100..6_100).contains(&due));
    assert!(!state.retry_provider_requests(due - 1));
    assert!(state.retry_provider_requests(due));
    assert_eq!(state.next_queued_request(agent), Some(request));
    assert!(!state.request(request).unwrap().trusted_start_bound);
    assert!(state.request(request).unwrap().provider_turn_id.is_none());
    let payload = state.render_request_payload(request).unwrap();
    assert!(payload.contains("finish the owned task"));
    assert!(payload.contains("check completed effects"));
    state.begin_submission(request, "launch-codex", 30).unwrap();
    let mut started = ProviderCallback {
        callback_id: "stale-start-with-new-id".into(),
        sequence: 31,
        occurred_at_ms: due + 1,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: Some(payload.clone()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert!(matches!(
        state.accept_callback(started.clone()),
        CallbackDisposition::Rejected(_)
    ));
    assert!(!state.request(request).unwrap().trusted_start_bound);
    started.callback_id = "new-native-start".into();
    started.sequence = 32;
    started.provider_turn_id = Some("turn-2".into());
    assert_eq!(
        state.accept_callback(started),
        CallbackDisposition::AcceptedBinding
    );
    let final_event = ProviderCallback::final_event(
        "success",
        33,
        agent,
        "launch-codex",
        "provider-session",
        "turn-2",
        &payload,
        "completed once",
    );
    assert_eq!(
        state.accept_callback(final_event),
        CallbackDisposition::AcceptedCompleted
    );
    assert_eq!(
        state.request(request).unwrap().reply().unwrap().text,
        "completed once"
    );
}

#[test]
fn provider_flake_native_activity_dialog_and_identity_guards_survive_deadlines() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Unavailable] {
        let (mut state, _, agent, request) =
            failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
        state.observe_status(agent, status, 200).unwrap();
        assert!(!state.retry_provider_requests(1_000_000));
        assert_eq!(
            state.agent(agent).unwrap().shown_status(),
            RuntimeStatus::Working
        );
        assert_eq!(state.agent(agent).unwrap().current_request, Some(request));
    }
    for guard in ["dialog", "identity", "delete"] {
        let (mut state, _, agent, request) =
            failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
        match guard {
            "dialog" => state.observe_dialog(agent, true, 200).unwrap(),
            "identity" => {
                state
                    .agents
                    .get_mut(&agent)
                    .unwrap()
                    .runtime_identity
                    .launch_id = Some("replacement".into())
            }
            _ => state.prepare_delete_agent(agent).unwrap(),
        }
        assert!(!state.retry_provider_requests(1_000_000), "{guard}");
        assert_eq!(state.agent(agent).unwrap().current_request, Some(request));
    }
}

#[test]
fn provider_flake_interrupt_and_delete_cancel_a_hold_without_resurrection() {
    for status in [RuntimeStatus::Idle, RuntimeStatus::Unavailable] {
        let (mut state, _, agent, request) =
            failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
        state.observe_status(agent, status, 200).unwrap();
        state.close_interrupted(agent, 201).unwrap();
        assert!(state.agent(agent).unwrap().provider_retry.is_none());
        assert_eq!(
            state.request(request).unwrap().failure_reason.as_deref(),
            Some("interrupted")
        );
        assert!(!state.retry_provider_requests(1_000_000));
    }
    let (mut state, _, agent, request) =
        failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
    state.delete_agent(agent, 200).unwrap();
    assert!(!state.retry_provider_requests(1_000_000));
    assert!(state.request(request).unwrap().settled());
}

fn steering_failure(
    bound: bool,
    replied_lead: bool,
) -> (BusState, AgentId, RequestId, RequestId, RequestId) {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .agents
        .get_mut(&agent)
        .unwrap()
        .runtime_identity
        .session_id = Some("provider-session".into());
    let lead = submit_text(&mut state, room, agent, "first task");
    start_request(&mut state, lead, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Working, 20)
        .unwrap();
    let member = submit_text(&mut state, room, agent, "second task");
    state.begin_steering(member, lead).unwrap();
    state
        .record_steering(
            member,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .unwrap();
    let started = ProviderCallback {
        callback_id: "steered-start".into(),
        sequence: 21,
        occurred_at_ms: 21,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: Some("second task".into()),
        kind: CallbackEventKind::PromptStarted,
    };
    if bound {
        assert_eq!(
            state.accept_callback(started),
            CallbackDisposition::AcceptedSteering
        );
    }
    let queued = submit_text(&mut state, room, agent, "later task");
    if replied_lead {
        state
            .answer_message(
                state.request(lead).unwrap().prompt.id,
                agent,
                "already done".into(),
                22,
            )
            .unwrap();
    }
    state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "group-native-failure".into(),
            sequence: 23,
            occurred_at_ms: 100,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: None,
            kind: CallbackEventKind::Error {
                message: "Provider failed".into(),
                provider_error: Some("Model at capacity (server_overloaded)".into())
            },
        }),
        CallbackDisposition::AcceptedError
    );
    (state, agent, lead, member, queued)
}

#[test]
fn provider_flake_preserves_group_order_and_never_replays_an_explicit_reply() {
    for replied in [false, true] {
        let (mut state, agent, lead, member, queued) = steering_failure(true, replied);
        assert!(state.retry_provider_requests(1_000_000));
        let expected = if replied {
            vec![member, queued]
        } else {
            vec![lead, member, queued]
        };
        assert_eq!(state.queued_requests(agent), expected);
        if replied {
            assert_eq!(
                state.request(lead).unwrap().reply().unwrap().text,
                "already done"
            );
        }
        assert!(state.request(member).unwrap().group.is_none());
    }
}

#[test]
fn provider_flake_retains_unattributed_steering_until_native_binding_arrives() {
    let (mut state, agent, lead, member, queued) = steering_failure(false, false);
    assert!(!state.retry_provider_requests(1_000_000));
    assert!(!state.settle_unclaimed_steering(agent, 1_000_000, |_| None));
    assert_eq!(state.agent(agent).unwrap().current_request, Some(lead));
    assert!(!state.request(member).unwrap().settled());
    assert!(state
        .recovery_reason(state.request(queued).unwrap(), u64::MAX)
        .is_none());
    let start = ProviderCallback {
        callback_id: "delayed-steered-start".into(),
        sequence: 24,
        occurred_at_ms: 1_000_001,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: None,
        prompt_payload: Some("second task".into()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        state.accept_callback(start),
        CallbackDisposition::AcceptedSteering
    );
    assert!(state.retry_provider_requests(1_000_002));
    assert_eq!(state.queued_requests(agent), &[lead, member, queued]);
}

#[test]
fn provider_flake_waits_for_recorded_quota_reset_times_in_the_named_zone() {
    for (now, message, reset) in [
        (
            1_793_545_200_000,
            "You've hit your 5-hour limit · resets 12:10am (Asia/Singapore) (rate_limit)",
            1_793_549_400_000,
        ),
        (
            1_793_545_200_000,
            "You've hit your weekly limit · resets 2am (Asia/Singapore) (rate_limit)",
            1_793_556_000_000,
        ),
        (
            1_792_458_000_000,
            "You've hit your weekly limit · resets Oct 25 at 2am (Asia/Singapore) (rate_limit)",
            1_792_864_800_000,
        ),
        (
            1_798_729_200_000,
            "Weekly limit · resets Jan 1 at 2am (Asia/Singapore) (rate_limit)",
            1_798_740_000_000,
        ),
        (
            1_793_545_200_000,
            "Quota · resets 12:10am (UTC+08:00) (rate_limit)",
            1_793_549_400_000,
        ),
    ] {
        let (mut state, _, agent, request) = failed_turn_at(Provider::ClaudeCode, message, now);
        let due = state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms;
        assert!(
            due >= reset + 2_000 && due < reset + 3_000,
            "{message}: {due}"
        );
        assert!(!state.retry_provider_requests(reset));
        assert!(!state.request(request).unwrap().settled());
        assert!(state.retry_provider_requests(due));
    }
}

#[test]
fn provider_flake_unknown_reset_uses_bounded_probes_and_backoff_has_no_attempt_limit() {
    for message in [
        "rate_limit",
        "resets 99:99am (Asia/Singapore) (rate_limit)",
        "resets 2am (Unrecognized/Zone) (rate_limit)",
    ] {
        let (state, _, agent, _) = failed_turn_at(Provider::ClaudeCode, message, 1_793_545_200_000);
        let due = state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms;
        assert!((1_793_545_500_000..1_793_545_501_000).contains(&due));
    }
    for (previous, min_delay, max_delay) in [
        (1, 10_000, 11_000),
        (2, 20_000, 21_000),
        (1000, 300_000, 301_000),
        (u32::MAX, 300_000, 301_000),
    ] {
        let (mut state, _, agent, request) =
            failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
        state.agents.get_mut(&agent).unwrap().provider_retry = None;
        state
            .requests
            .get_mut(&request)
            .unwrap()
            .provider_retry_attempts = previous;
        assert!(state.hold_provider_error(request, "Model at capacity (server_overloaded)", 1_000));
        let due = state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms;
        assert!((1_000 + min_delay..1_000 + max_delay).contains(&due));
        assert!(!state.request(request).unwrap().settled());
    }
}

#[test]
fn provider_flake_native_new_turn_can_claim_unbound_steering_without_replaying_it() {
    let (mut state, agent, lead, member, queued) = steering_failure(false, false);
    let start = ProviderCallback {
        callback_id: "native-queued-start".into(),
        sequence: 24,
        occurred_at_ms: 200,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("native-next-turn".into()),
        provider_prompt_id: None,
        prompt_payload: Some("second task".into()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        state.accept_callback(start),
        CallbackDisposition::AcceptedSteering
    );
    assert_eq!(state.agent(agent).unwrap().current_request, Some(member));
    assert!(!state.retry_provider_requests(1_000_000));
    let final_event = ProviderCallback::final_event(
        "native-queued-final",
        25,
        agent,
        "launch-codex",
        "provider-session",
        "native-next-turn",
        "second task",
        "second task done",
    );
    assert_eq!(
        state.accept_callback(final_event),
        CallbackDisposition::AcceptedCompleted
    );
    assert_eq!(
        state.request(member).unwrap().reply().unwrap().text,
        "second task done"
    );
    assert!(!state.request(lead).unwrap().settled());
    assert!(state.retry_provider_requests(1_000_001));
    assert_eq!(state.queued_requests(agent), &[lead, queued]);
}

#[test]
fn provider_flake_keeps_working_ownership_when_a_due_retry_waits_for_the_composer() {
    let (mut state, _, agent, request) =
        failed_turn(Provider::Codex, "Model at capacity (server_overloaded)");
    assert!(state.retry_provider_requests(1_000_000));
    assert_eq!(
        state.agent(agent).unwrap().shown_status(),
        RuntimeStatus::Working
    );
    state.begin_submission(request, "launch-codex", 30).unwrap();
    state
        .record_submission(request, SubmissionOutcome::ComposerBusy)
        .unwrap();
    assert_eq!(
        state.agent(agent).unwrap().shown_status(),
        RuntimeStatus::Working
    );
    assert!(!state.request(request).unwrap().settled());
    state.close_interrupted(agent, 1_000_001).unwrap();
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
    assert!(state.queued_requests(agent).is_empty());
    assert!(!state.retry_provider_requests(2_000_000));
}

#[test]
fn provider_flake_a_permanent_error_in_native_queued_input_cannot_orphan_the_held_request() {
    let (mut state, agent, lead, member, _) = steering_failure(false, false);
    let start = ProviderCallback {
        callback_id: "native-queued-start".into(),
        sequence: 24,
        occurred_at_ms: 200,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("native-next-turn".into()),
        provider_prompt_id: None,
        prompt_payload: Some("second task".into()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        state.accept_callback(start),
        CallbackDisposition::AcceptedSteering
    );
    state.accept_callback(ProviderCallback {
        callback_id: "permanent-native-error".into(),
        sequence: 25,
        occurred_at_ms: 201,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some("native-next-turn".into()),
        provider_prompt_id: None,
        prompt_payload: None,
        kind: CallbackEventKind::Error {
            message: "Invalid model".into(),
            provider_error: Some("Invalid model (invalid_request)".into()),
        },
    });
    for id in [lead, member] {
        assert!(state.request(id).unwrap().settled());
        assert!(state
            .request(id)
            .unwrap()
            .failure_reason
            .as_deref()
            .unwrap()
            .contains("invalid_request"));
    }
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
    assert!(!state.retry_provider_requests(1_000_000));
}
