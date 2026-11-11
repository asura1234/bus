use super::*;

fn providers() -> [Provider; 6] {
    [
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ]
}

#[test]
fn a_legacy_false_idle_marker_cannot_settle_a_reloaded_active_turn() {
    // Kimi's old implementation already required wire evidence for this
    // marker; its committed end must keep that separate trust contract.
    for provider in providers().into_iter().filter(|p| *p != Provider::Kimi) {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "running across reload");
        start_request(&mut state, lead, "launch-codex", 100);
        state.requests.get_mut(&lead).unwrap().turn_ended_at_ms = Some(283);
        let mut state: BusState =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        assert!(
            !state.turn_ended(state.request(lead).unwrap()),
            "legacy stale Idle marker: {provider:?}"
        );
        assert!(!state.settling(state.request(lead).unwrap()));
        assert!(state
            .settle_ended_request(agent, 100_000, |_| None)
            .is_none());
        state
            .observe_status(agent, RuntimeStatus::Idle, 100_001)
            .unwrap();
        assert!(state.request(lead).unwrap().turn_ended_at_ms.is_none());
        assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Active);
        assert_eq!(state.agent(agent).unwrap().current_request, Some(lead));
    }
}

#[test]
fn codex_resumed_started_turn_cannot_settle_from_the_pre_resume_idle_frame() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    state
        .observe_status(agent, RuntimeStatus::Unavailable, 2_000)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 2_239)
        .unwrap();
    // Reloaded native identity is ready for delivery, but that idle frame is
    // not proof that the subsequently accepted model turn has finished.
    let mut state: BusState = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
    let lead = state
        .submit_message_with(
            room,
            Draft {
                text: "reply with the unique resume token".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            15_206,
            false,
        )
        .unwrap()[0];
    submit_request(&mut state, lead, "launch-codex", 15);
    let payload = state.request(lead).unwrap().prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback {
            callback_id: "resumed-start".into(),
            sequence: 17,
            occurred_at_ms: 16_871,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some(payload.clone()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    // Reproduce request80: unchanged Idle 538 ms after genuine Started.
    state
        .observe_status(agent, RuntimeStatus::Idle, 17_409)
        .unwrap();
    assert!(
        state.request(lead).unwrap().turn_ended_at_ms.is_none(),
        "resumed Started cannot end from unchanged Idle"
    );
    let mut state: BusState = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, 22_900)
        .unwrap();
    assert!(state
        .settle_ended_request(agent, 22_900, |_| None)
        .is_none());
    assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Active);
    assert_eq!(state.agent(agent).unwrap().current_request, Some(lead));
    // A delayed genuine final remains attributable instead of becoming a
    // detached result after a false no-reply completion.
    let mut final_event = ProviderCallback::final_event(
        "resumed-final",
        18,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        &payload,
        "resume token",
    );
    final_event.occurred_at_ms = 24_000;
    assert_eq!(
        state.accept_callback(final_event),
        CallbackDisposition::AcceptedCompleted
    );
    assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Completed);
}

#[test]
fn started_on_an_idle_frame_does_not_end_the_turn_or_disable_steering() {
    for provider in providers() {
        for restart in [false, true] {
            let (mut state, room, agent, _) = state_with_room_and_agents();
            state.agents.get_mut(&agent).unwrap().provider = provider;
            state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
            let lead = submit_text(&mut state, room, agent, "run a forty second command");
            start_request(&mut state, lead, "launch-codex", 100);
            // A fresh poll can still contain the native frame from before Started.
            state
                .observe_status(agent, RuntimeStatus::Idle, 283)
                .unwrap();
            assert!(
                state.request(lead).unwrap().turn_ended_at_ms.is_none(),
                "{provider:?}"
            );
            assert!(
                !state.turn_ended(state.request(lead).unwrap()),
                "{provider:?}"
            );
            if restart {
                state = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
            }
            state
                .observe_status(agent, RuntimeStatus::Working, 851)
                .unwrap();
            let correction = submit_text(&mut state, room, agent, "use the corrected token");
            let expected = (provider != Provider::Grok).then_some((correction, lead));
            assert_eq!(
                state.next_steering(agent),
                expected,
                "{provider:?}, restart={restart}"
            );
            if expected.is_some() {
                state.begin_steering(correction, lead).unwrap();
                assert_eq!(state.request(correction).unwrap().group, Some(lead));
            }
            assert!(state
                .settle_ended_request(agent, 100_000, |_| None)
                .is_none());
        }
    }
}

#[test]
fn a_claude_orchestrators_developer_correction_joins_the_original_master_turn() {
    let (mut state, work, _, _) = state_with_room_and_agents();
    let master = state.ensure_master_room();
    let agent = state
        .create_agent(
            master,
            "orchestrator",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("orchestrator-terminal".into()),
                pane_id: Some("orchestrator-pane".into()),
                session_id: Some("provider-session".into()),
            },
        )
        .unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let lead = submit_text(&mut state, master, agent, "run a forty second command");
    start_request(&mut state, lead, "launch", 100);
    state
        .observe_status(agent, RuntimeStatus::Idle, 283)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Working, 851)
        .unwrap();
    let correction = submit_text(&mut state, master, agent, "use corrected token");
    assert_eq!(state.next_steering(agent), Some((correction, lead)));
    state.begin_steering(correction, lead).unwrap();
    assert_eq!(state.request(correction).unwrap().group, Some(lead));
}

#[test]
fn busy_recovery_uses_observation_order_even_when_the_wall_clock_rolls_back() {
    for working_at in [250, 300, 301] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        let lead = submit_text(&mut state, room, agent, "command");
        start_request(&mut state, lead, "launch-codex", 100);
        state
            .observe_status(agent, RuntimeStatus::Working, 200)
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Idle, 300)
            .unwrap();
        assert_eq!(state.request(lead).unwrap().turn_ended_at_ms, Some(300));
        let revision = state.agent(agent).unwrap().status_revision;
        state
            .observe_status(agent, RuntimeStatus::Working, working_at)
            .unwrap();
        assert!(state.agent(agent).unwrap().status_revision > revision);
        assert!(
            state.request(lead).unwrap().turn_ended_at_ms.is_none(),
            "latest Working must revoke idle inference despite wall-clock ordering"
        );
        assert!(!state.turn_ended(state.request(lead).unwrap()));
    }
}

#[test]
fn a_trusted_provider_error_end_survives_idle_and_working_observations() {
    // Cursor completes errors rather than opening this native retry hold.
    for provider in providers().into_iter().filter(|p| *p != Provider::Cursor) {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "request held after native error");
        start_request(&mut state, lead, "launch-codex", 100);
        assert!(state.hold_provider_error(lead, "Model at capacity (server_overloaded)", 120));
        state
            .observe_status(agent, RuntimeStatus::Idle, 121)
            .unwrap();
        assert_eq!(
            state.request(lead).unwrap().turn_ended_at_ms,
            Some(120),
            "a provider-owned error is trusted end evidence: {provider:?}"
        );
        let mut state: BusState =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        state
            .observe_status(agent, RuntimeStatus::Working, 122)
            .unwrap();
        assert_eq!(state.request(lead).unwrap().turn_ended_at_ms, Some(120));
        assert!(state.turn_ended(state.request(lead).unwrap()));
        submit_text(&mut state, room, agent, "queued during provider retry");
        assert_eq!(state.next_steering(agent), None);
    }
}

#[test]
fn an_explicit_reply_does_not_make_a_provisional_idle_marker_permanent() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let lead = submit_text(&mut state, room, agent, "command");
    start_request(&mut state, lead, "launch-codex", 100);
    let message = state.request(lead).unwrap().prompt.id;
    state
        .answer_message(message, agent, "receipt".into(), 110)
        .unwrap();
    state.requests.get_mut(&lead).unwrap().turn_ended_at_ms = Some(120);
    state
        .observe_status(agent, RuntimeStatus::Working, 121)
        .unwrap();
    assert!(state.request(lead).unwrap().turn_ended_at_ms.is_none());
    assert_eq!(
        state.request(lead).unwrap().reply().unwrap().text,
        "receipt"
    );
    let correction = submit_text(&mut state, room, agent, "correction");
    assert_eq!(state.next_steering(agent), Some((correction, lead)));
}

#[test]
fn working_clears_only_the_current_unfinished_turns_idle_marker_after_reload() {
    for provider in providers() {
        let (mut state, room, agent, other) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        let lead = submit_text(&mut state, room, agent, "first");
        start_request(&mut state, lead, "launch-codex", 100);
        let concurrent = submit_text(&mut state, room, other, "independent turn");
        start_request(&mut state, concurrent, "launch-claude", 100);
        // Simulate a saved marker written by an older coordinator.
        state.requests.get_mut(&lead).unwrap().turn_ended_at_ms = Some(283);
        state
            .requests
            .get_mut(&concurrent)
            .unwrap()
            .turn_ended_at_ms = Some(283);
        let mut state: BusState =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        state
            .observe_status(agent, RuntimeStatus::Working, 851)
            .unwrap();
        assert!(
            state.request(lead).unwrap().turn_ended_at_ms.is_none(),
            "{provider:?}"
        );
        assert!(!state.turn_ended(state.request(lead).unwrap()));
        assert_eq!(
            state.request(concurrent).unwrap().turn_ended_at_ms,
            Some(283)
        );
    }
}

#[test]
fn idle_end_requires_a_transition_after_started_and_working_reopens_settlement() {
    for provider in providers() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "command");
        start_request(&mut state, lead, "launch-codex", 100);
        state
            .observe_status(agent, RuntimeStatus::Working, 102)
            .unwrap();
        // An observation at/before Started cannot close this bound turn.
        state
            .observe_status(agent, RuntimeStatus::Idle, 101)
            .unwrap();
        assert!(
            !state.turn_ended(state.request(lead).unwrap()),
            "{provider:?}"
        );
        state
            .observe_status(agent, RuntimeStatus::Working, 103)
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Idle, 104)
            .unwrap();
        assert_eq!(
            state.turn_ended(state.request(lead).unwrap()),
            provider != Provider::Kimi
        );
        state
            .observe_status(agent, RuntimeStatus::Working, 105)
            .unwrap();
        assert!(
            !state.turn_ended(state.request(lead).unwrap()),
            "{provider:?}"
        );
        assert!(state
            .settle_ended_request(agent, 100_000, |_| None)
            .is_none());
    }
}

#[test]
fn a_trusted_final_is_not_reopened_by_background_working() {
    for provider in providers() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "command");
        start_request(&mut state, lead, "launch-codex", 100);
        let payload = state.request(lead).unwrap().prompt.rendered_payload();
        let disposition = state.accept_callback(ProviderCallback::final_event(
            "final",
            110,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            &payload,
            "done",
        ));
        assert!(
            !matches!(disposition, CallbackDisposition::Rejected(_)),
            "{provider:?}: {disposition:?}"
        );
        state
            .observe_status(agent, RuntimeStatus::Working, 111)
            .unwrap();
        assert!(
            state.turn_ended(state.request(lead).unwrap()),
            "{provider:?}"
        );
        assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Completed);
    }
}
