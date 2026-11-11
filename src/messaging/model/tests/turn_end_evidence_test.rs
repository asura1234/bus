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
fn an_older_busy_observation_cannot_revoke_a_newer_idle_end() {
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
    state
        .observe_status(agent, RuntimeStatus::Working, 250)
        .unwrap();
    assert_eq!(state.request(lead).unwrap().turn_ended_at_ms, Some(300));
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
