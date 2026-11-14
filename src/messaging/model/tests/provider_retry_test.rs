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
fn provider_flake_codex_explicit_native_quota_categories_keep_the_delivery_open() {
    // Synthetic native errors: categories are present in installed Codex 0.164.0
    // schema/binary evidence. No real quota rollout was observed for these codes.
    for code in [
        "usage_limit_reached",
        "usage_limit_exceeded",
        "usageLimitExceeded",
        "rateLimitExceeded",
    ] {
        let error = format!("Provider quota exhausted ({code})");
        let (mut state, _, agent, request) = failed_turn(Provider::Codex, &error);
        assert!(!state.request(request).unwrap().settled(), "{code}");
        let hold = state.agent(agent).unwrap().provider_retry.as_ref().unwrap();
        assert!(hold.quota, "{code}");
        assert!(
            hold.retry_at_ms >= 300_100,
            "unknown reset uses a bounded quota probe"
        );
        assert!(!state.retry_provider_requests(1000), "{code}");
    }
}

#[test]
fn provider_flake_explicit_native_http_server_errors_are_held_without_guessing_from_numbers() {
    // Synthetic HTTP shapes cover the approved 5xx policy, not a recorded incident.
    for provider in [
        Provider::ClaudeCode,
        Provider::Codex,
        Provider::Grok,
        Provider::Kimi,
    ] {
        for error in [
            "HTTP 500 Internal Server Error",
            "API Error: 500 api_error",
            "HTTP status 507 service failure",
        ] {
            let (state, _, agent, request) = failed_turn(provider, error);
            assert!(
                !state.request(request).unwrap().settled(),
                "{provider:?}: {error}"
            );
            assert_eq!(
                state.agent(agent).unwrap().shown_status(),
                RuntimeStatus::Working
            );
            assert!(
                !state
                    .agent(agent)
                    .unwrap()
                    .provider_retry
                    .as_ref()
                    .unwrap()
                    .quota
            );
        }
    }
    for error in [
        "Context window exceeded: 512 tokens",
        "HTTP 600 unknown status",
        "HTTP 500 invalid model",
    ] {
        let (state, _, _, request) = failed_turn(Provider::Codex, error);
        assert!(state.request(request).unwrap().settled(), "{error}");
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
fn provider_flake_cursor_monthly_quota_keeps_its_terminal_contract() {
    // Synthetic structured native error; monthly Cursor limits are outside this hold policy.
    let (state, _, agent, request) =
        failed_turn(Provider::Cursor, "Monthly usage limit reached (rate_limit)");
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert!(state.request(request).unwrap().failure_reason.is_some());
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
}

#[test]
fn provider_flake_kimi_connection_abort_is_not_a_user_cancellation() {
    // Recorded Kimi 2.4.1 OAuth connection failure from the private proxy503 acceptance.
    let error = "OAuthConnectionError: OAuth request to https://auth.kimi.com/api/oauth/token failed: fetch failed: Request was cancelled.: Proxy response (503) !== 200 when HTTP Tunneling";
    for error in [
        error,
        // Synthetic control for the other already-supported native connection category.
        "APIConnectionError: Connection attempt was cancelled: HTTP503",
    ] {
        let (state, _, agent, request) = failed_turn(Provider::Kimi, error);
        assert!(
            !state.request(request).unwrap().settled(),
            "a transport abort must retain the delivery"
        );
        assert!(state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .is_some_and(|hold| !hold.quota));
    }
    for error in [
        "OAuthConnectionError: HTTP401 unauthorized: Request was cancelled.",
        "OAuthConnectionError: HTTP403 permission denied: Request was cancelled.",
        "OAuthConnectionError: Request cancelled by user",
        "Kimi turn interrupted",
        "Request was cancelled.",
    ] {
        let (state, _, agent, request) = failed_turn(Provider::Kimi, error);
        assert_eq!(
            state.request(request).unwrap().phase,
            RequestPhase::Abandoned,
            "{error}"
        );
        assert!(state.agent(agent).unwrap().provider_retry.is_none());
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
    let settled = state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(settled.reply().is_none());
    assert!(settled.failure_reason.is_none());
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
    let settled = state.request(member).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(settled.reply().is_none());
    assert!(settled.failure_reason.is_none());
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

#[test]
fn provider_flake_review_past_quota_reset_uses_growing_capped_backoff() {
    let now = 1_793_545_200_000;
    let error = "You've hit your weekly limit · resets Oct 25 at 2am (Asia/Singapore) (rate_limit)";
    for (previous, minimum, maximum) in [
        (0, 5000, 6000),
        (1, 10000, 11000),
        (2, 20000, 21000),
        (100, 300000, 301000),
    ] {
        let (mut state, _, agent, request) = failed_turn_at(Provider::ClaudeCode, error, now);
        state.agents.get_mut(&agent).unwrap().provider_retry = None;
        state
            .requests
            .get_mut(&request)
            .unwrap()
            .provider_retry_attempts = previous;
        assert!(state.hold_provider_error(request, error, now));
        let due = state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms;
        assert!(
            (now + minimum..now + maximum).contains(&due),
            "attempt {previous}: {due}"
        );
    }
}

#[test]
fn provider_flake_review_minute_rounded_reset_does_not_delay_an_entire_day() {
    for (now, clock) in [(1_793_549_430_000, "12:10am"), (1_793_556_030_000, "2am")] {
        let error =
            format!("You've hit your 5-hour limit · resets {clock} (Asia/Singapore) (rate_limit)");
        let (state, _, agent, request) = failed_turn_at(Provider::ClaudeCode, &error, now);
        let due = state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms;
        assert!((now + 5000..now + 6000).contains(&due), "{clock}: {due}");
        assert!(!state.request(request).unwrap().settled());
    }
}

fn review_native_input(agent: AgentId, turn: &str, text: &str, sequence: u64) -> ProviderCallback {
    ProviderCallback {
        callback_id: format!("review-start-{turn}"),
        sequence,
        occurred_at_ms: sequence,
        agent_id: agent,
        launch_id: "launch-codex".into(),
        provider_session_id: Some("provider-session".into()),
        provider_turn_id: Some(turn.into()),
        provider_prompt_id: None,
        prompt_payload: Some(text.into()),
        kind: CallbackEventKind::PromptStarted,
    }
}

#[test]
fn provider_flake_review_interrupt_closes_the_independent_hold_before_and_after_native_final() {
    for finished in [false, true] {
        let (mut state, agent, lead, member, queued) = steering_failure(false, false);
        assert_eq!(
            state.accept_callback(review_native_input(agent, "native-2", "second task", 24)),
            CallbackDisposition::AcceptedSteering
        );
        if finished {
            state.accept_callback(ProviderCallback::final_event(
                "review-native-final",
                25,
                agent,
                "launch-codex",
                "provider-session",
                "native-2",
                "second task",
                "second done",
            ));
        }
        state.close_interrupted(agent, 200).unwrap();
        assert_eq!(
            state.request(lead).unwrap().failure_reason.as_deref(),
            Some("interrupted")
        );
        assert!(state.request(lead).unwrap().settled());
        let waiting = state.request(queued).unwrap();
        assert_eq!(waiting.phase, RequestPhase::Queued, "{waiting:?}");
        assert!(waiting.failure_reason.is_none(), "{waiting:?}");
        assert_eq!(state.queued_requests(agent), &[queued]);
        if finished {
            let settled = state.request(member).unwrap();
            assert_eq!(settled.phase, RequestPhase::Completed);
            assert!(settled.reply().is_none());
            assert!(settled.failure_reason.is_none());
        } else {
            assert_eq!(
                state.request(member).unwrap().failure_reason.as_deref(),
                Some("interrupted")
            );
        }
        assert!(!state.retry_provider_requests(1_000_000));
    }
}

#[test]
fn provider_flake_review_native_member_background_continuation_keeps_its_own_reply() {
    let (mut state, agent, lead, member, queued) = steering_failure(false, false);
    assert_eq!(
        state.accept_callback(review_native_input(agent, "native-2", "second task", 24)),
        CallbackDisposition::AcceptedSteering
    );
    let mut background = review_native_input(agent, "native-2", "second task", 25);
    background.callback_id = "review-background".into();
    background.prompt_payload = None;
    background.kind = CallbackEventKind::BackgroundPending;
    assert_eq!(
        state.accept_callback(background),
        CallbackDisposition::AcceptedProgress
    );
    assert_eq!(
        state.accept_callback(review_native_input(
            agent,
            "native-3",
            "<task-notification>finished</task-notification>",
            26
        )),
        CallbackDisposition::AcceptedContinuation
    );
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "review-continued-final",
            27,
            agent,
            "launch-codex",
            "provider-session",
            "native-3",
            "second task",
            "second done"
        )),
        CallbackDisposition::AcceptedCompleted
    );
    let settled = state.request(member).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert!(settled.reply().is_none());
    assert!(settled.failure_reason.is_none());
    assert!(!state.request(lead).unwrap().settled());
    assert!(state.retry_provider_requests(1_000_000));
    assert_eq!(state.queued_requests(agent), &[lead, queued]);
}

#[test]
fn provider_flake_review_explicit_native_connection_and_server_failures_are_retryable() {
    for provider in [Provider::Grok, Provider::Kimi] {
        for error in [
            "HTTP 503 Service Unavailable",
            "Connection lost (connection_error)",
            "Request timeout (timeout)",
        ] {
            let (state, _, agent, request) = failed_turn(provider, error);
            assert!(
                !state.request(request).unwrap().settled(),
                "{provider:?}: {error}"
            );
            assert_eq!(
                state.agent(agent).unwrap().shown_status(),
                RuntimeStatus::Working
            );
        }
    }
}

#[test]
fn grok_native_connection_failure_keeps_the_delivery_open() {
    let native = serde_json::json!({
        "hookEventName": "stop_failure", "sessionId": "provider-session", "promptId": "turn-1",
        "error": "server_error", "errorDetails": "API Error: Connection lost mid-response (HTTP 503)"
    });
    let crate::agents::providers::spool::Parsed::Failure {
        error: Some(error), ..
    } = crate::agents::providers::grok::hooks::parse(&native).unwrap()
    else {
        panic!("the native failure must retain its error category");
    };
    let (state, _, agent, request) = failed_turn(Provider::Grok, &error);
    assert!(
        !state.request(request).unwrap().settled(),
        "native error was abandoned: {error}"
    );
    assert!(state.agent(agent).unwrap().provider_retry.is_some());
}

#[test]
fn kimi_native_connection_failure_keeps_the_delivery_open() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = Provider::Kimi;
    state
        .agents
        .get_mut(&agent)
        .unwrap()
        .runtime_identity
        .session_id = Some("provider-session".into());
    let request = submit_text(&mut state, room, agent, "finish the owned task");
    state.begin_submission(request, "launch-codex", 10).unwrap();
    let payload = state.render_request_payload(request).unwrap();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "native-start-seven".into(),
            sequence: 11,
            occurred_at_ms: 20,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("7".into()),
            provider_prompt_id: None,
            prompt_payload: Some(payload.clone()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    let wire = [
        serde_json::json!({"type":"turn.prompt","agentId":"main","turnId":7,"promptId":"p7","input":[{"type":"text","text":payload}]}),
        serde_json::json!({"type":"agent.turn.ended","turnId":7,"outcome":"failed","errorMessage":"APIConnectionError: Connection lost mid-response (HTTP 503)"}),
        serde_json::json!({"type":"turn.ended","agentId":"main","turnId":7,"reason":"failed"}),
        serde_json::json!({"type":"prompt.completed","agentId":"main","promptId":"p7","reason":"failed"}),
    ].into_iter().map(|v| v.to_string()).collect::<Vec<_>>().join("\n");
    let Some(crate::agents::providers::kimi::wire::Outcome::Failed(error)) =
        crate::agents::providers::kimi::wire::committed_outcome(&wire, 7, &payload)
    else {
        panic!("the owned native failure must have a committed error");
    };
    state
        .observe_status(agent, RuntimeStatus::Working, 21)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(agent, 101, |_| Some(RecordedTurnEnd::Error(error))),
        Some(request)
    );
    assert!(
        !state.request(request).unwrap().settled(),
        "the native connection failure abandoned the owned delivery"
    );
    assert!(state.agent(agent).unwrap().provider_retry.is_some());
}

#[test]
fn accept_callback_stamps_provider_start_on_the_rebound_member() {
    let (mut state, agent, lead, member, _) = steering_failure(false, false);
    let lead_started = state.request(lead).unwrap().latency.provider_started_at_ms;
    assert_eq!(
        state.accept_callback(ProviderCallback {
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
        }),
        CallbackDisposition::AcceptedSteering
    );
    assert_eq!(state.agent(agent).unwrap().current_request, Some(member));
    assert_eq!(
        state
            .request(member)
            .unwrap()
            .latency
            .provider_started_at_ms,
        Some(200),
        "the steered member owns the start that rebound it"
    );
    assert_eq!(
        state.request(lead).unwrap().latency.provider_started_at_ms,
        lead_started,
        "rebinding must not give the previous lead the member's provider start"
    );
}

#[test]
fn restart_settling_an_independent_input_keeps_the_provider_retry_hold() {
    let (mut state, agent, lead, member, _queued) = steering_failure(false, false);
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
    assert_eq!(
        state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .map(|hold| hold.lead),
        Some(lead)
    );
    for id in [lead, member] {
        state
            .stamp_delivery_epoch(
                id,
                DeliveryEpoch {
                    generation: 1,
                    terminal_id: "terminal".into(),
                    pane_id: "pane".into(),
                },
            )
            .unwrap();
    }
    assert_eq!(
        state.settle_generation_request(agent, member, 2, 200, |_| None),
        Some(member)
    );
    assert_eq!(
        state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .map(|hold| hold.lead),
        Some(lead),
        "settling the independent interrupted input must keep the failed delivery's retry hold"
    );
    assert_eq!(
        state.settle_generation_request(agent, lead, 2, 201, |_| None),
        None
    );
    assert!(
        !state.request(lead).unwrap().settled(),
        "the held delivery stays open for provider retry"
    );
}

#[test]
fn interrupting_a_prepared_retry_closes_every_prior_turn_member() {
    let (mut state, agent, lead, member, queued) = steering_failure(true, false);
    assert!(state.retry_provider_requests(1_000_000));
    assert_eq!(state.queued_requests(agent), &[lead, member, queued]);
    assert_eq!(state.request(member).unwrap().group, None);
    assert!(state.request(member).unwrap().provider_retry_attempts > 0);
    state.close_interrupted(agent, 1_000_001).unwrap();
    assert!(state.request(lead).unwrap().settled());
    assert_eq!(
        state.request(member).unwrap().phase,
        RequestPhase::Abandoned,
        "a prepared retry member belongs to the stopped turn, not fresh queued work"
    );
    assert_eq!(
        state.request(member).unwrap().failure_reason.as_deref(),
        Some("interrupted")
    );
    assert_eq!(state.request(queued).unwrap().phase, RequestPhase::Queued);
    assert_eq!(state.queued_requests(agent), &[queued]);
}

#[test]
fn interrupting_a_rejected_prepared_retry_closes_every_prior_turn_member() {
    let (mut state, agent, lead, member, queued) = steering_failure(true, false);
    assert!(state.retry_provider_requests(1_000_000));
    assert_eq!(state.queued_requests(agent), &[lead, member, queued]);
    assert!(state.coalesce_queue(lead).is_some());
    state.begin_submission(lead, "launch-codex", 30).unwrap();
    state
        .record_submission(lead, SubmissionOutcome::ComposerBusy)
        .unwrap();
    assert!(
        state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .queued
    );
    assert_eq!(state.queued_requests(agent), &[lead, member, queued]);
    state.close_interrupted(agent, 1_000_001).unwrap();
    assert!(state.request(lead).unwrap().settled());
    assert_eq!(
        state.request(member).unwrap().phase,
        RequestPhase::Abandoned,
        "a definite retry refusal must preserve cancellation ownership of prior turn members"
    );
    assert_eq!(
        state.request(member).unwrap().failure_reason.as_deref(),
        Some("interrupted")
    );
    assert_eq!(state.request(queued).unwrap().phase, RequestPhase::Queued);
    assert_eq!(state.queued_requests(agent), &[queued]);
}

#[test]
fn interrupting_a_definitely_rejected_prepared_retry_closes_every_prior_turn_member() {
    let (mut state, agent, lead, member, queued) = steering_failure(true, false);
    assert!(state.retry_provider_requests(1_000_000));
    assert_eq!(state.queued_requests(agent), &[lead, member, queued]);
    assert!(state.coalesce_queue(lead).is_some());
    state.begin_submission(lead, "launch-codex", 30).unwrap();
    state
        .record_submission(
            lead,
            SubmissionOutcome::DefinitelyRejected {
                message: "agent is not ready".into(),
            },
        )
        .unwrap();
    assert!(
        state
            .agent(agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .queued
    );
    assert_eq!(state.queued_requests(agent), &[lead, member, queued]);
    state.close_interrupted(agent, 1_000_001).unwrap();
    assert!(state.request(lead).unwrap().settled());
    assert_eq!(
        state.request(member).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(
        state.request(member).unwrap().failure_reason.as_deref(),
        Some("interrupted")
    );
    assert_eq!(state.request(queued).unwrap().phase, RequestPhase::Queued);
    assert_eq!(state.queued_requests(agent), &[queued]);
}

#[test]
fn a_confirmed_prepared_retry_releases_the_parked_hold() {
    let (mut state, agent, lead, member, queued) = steering_failure(true, false);
    assert!(state.retry_provider_requests(1_000_000));
    assert!(state.coalesce_queue(lead).is_some());
    state.begin_submission(lead, "launch-codex", 30).unwrap();
    state
        .record_submission(
            lead,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("retry-turn".into()),
            },
        )
        .unwrap();
    assert!(state.agent(agent).unwrap().provider_retry.is_none());
    assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Active);
    assert!(!state.request(member).unwrap().settled());
    assert!(!state.request(queued).unwrap().settled());
}
