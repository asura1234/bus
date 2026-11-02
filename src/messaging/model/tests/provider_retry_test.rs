use super::*;

fn failed_turn(provider: Provider, message: &str) -> (BusState, RoomId, AgentId, RequestId) {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state.agents.get_mut(&agent).unwrap().provider = provider;
    let request = submit_text(&mut state, room, agent, "finish the owned task");
    start_request(&mut state, request, "launch-codex", 10);
    state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    state.accept_callback(ProviderCallback {
        callback_id: "native-failure".into(),
        sequence: 20,
        occurred_at_ms: 100,
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
