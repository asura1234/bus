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

#[test]
fn a_bound_turn_that_never_worked_has_a_bounded_uncertain_idle_backstop() {
    for provider in providers() {
        let (mut state, room, agent, other) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "lost final callback");
        start_request(&mut state, lead, "launch-codex", 100);
        let concurrent = submit_text(&mut state, room, other, "independent");
        start_request(&mut state, concurrent, "launch-claude", 100);
        state
            .observe_status(agent, RuntimeStatus::Idle, 283)
            .unwrap();
        let queued = submit_text(&mut state, room, agent, "next request");
        assert!(state
            .settle_ended_request(agent, 30_100, |_| None)
            .is_none());
        // The first post-Started Idle observation starts a non-renewable 30s
        // window. Reload and repeated Idle polls must not reset its deadline.
        let mut state: BusState =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        state
            .observe_status(agent, RuntimeStatus::Idle, 30_282)
            .unwrap();
        assert!(state
            .settle_ended_request(agent, 30_282, |_| None)
            .is_none());
        state
            .observe_status(agent, RuntimeStatus::Idle, 30_283)
            .unwrap();
        assert_eq!(
            state.settle_ended_request(agent, 30_283, |_| None),
            Some(lead),
            "{provider:?}"
        );
        let closed = state.request(lead).unwrap();
        assert_eq!(closed.phase, RequestPhase::Abandoned);
        assert!(closed.uncertain_outcome);
        assert_eq!(
            closed.failure_reason.as_deref(),
            Some("turn_end_unconfirmed")
        );
        assert!(
            closed.turn_ended_at_ms.is_none(),
            "timeout is not end evidence"
        );
        assert_eq!(state.agent(agent).unwrap().current_request, None);
        assert_eq!(state.next_queued_request(agent), Some(queued));
        assert_eq!(
            state.request(concurrent).unwrap().phase,
            RequestPhase::Active
        );
    }
}

#[test]
fn idle_backstop_reconciles_owned_provider_records_before_uncertain_release() {
    for provider in providers() {
        for final_answer in [false, true] {
            let (mut state, room, agent, _) = state_with_room_and_agents();
            state.agents.get_mut(&agent).unwrap().provider = provider;
            state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
            let lead = submit_text(&mut state, room, agent, "lost terminal hook");
            start_request(&mut state, lead, "launch-codex", 100);
            state
                .observe_status(agent, RuntimeStatus::Idle, 283)
                .unwrap();
            state
                .observe_status(agent, RuntimeStatus::Idle, 30_283)
                .unwrap();
            let payload = state.request(lead).unwrap().prompt.rendered_payload();
            let record = if final_answer {
                RecordedTurnEnd::Final {
                    turn_id: "turn-1".into(),
                    text: "native result".into(),
                    prompts: vec![payload],
                }
            } else {
                RecordedTurnEnd::NoReply
            };
            assert_eq!(
                state.settle_ended_request(agent, 30_283, |r| {
                    assert_eq!(r.id, lead);
                    Some(record)
                }),
                Some(lead),
                "{provider:?}"
            );
            let request = state.request(lead).unwrap();
            assert_eq!(request.phase, RequestPhase::Completed);
            assert!(!request.uncertain_outcome);
            assert!(request.failure_reason.is_none());
            // Provider terminal text still is not an explicit Bus reply.
            assert!(request.reply().is_none());
            assert_eq!(state.agent(agent).unwrap().current_request, None);
        }
    }
}

#[test]
fn idle_backstop_preserves_an_explicit_reply_and_marks_only_unanswered_group_members() {
    for provider in providers() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "answered without a final");
        let joined = submit_text(&mut state, room, agent, "coalesced");
        state.requests.get_mut(&joined).unwrap().group = Some(lead);
        state.requests.get_mut(&joined).unwrap().phase = RequestPhase::Active;
        start_request(&mut state, lead, "launch-codex", 100);
        state
            .answer_message(
                state.request(lead).unwrap().prompt.id,
                agent,
                "receipt".into(),
                150,
            )
            .unwrap();
        let reply = state.request(lead).unwrap().completed_reply.clone();
        state
            .observe_status(agent, RuntimeStatus::Idle, 283)
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Idle, 30_283)
            .unwrap();
        assert_eq!(
            state.settle_ended_request(agent, 30_283, |_| None),
            Some(lead)
        );
        assert_eq!(state.request(lead).unwrap().completed_reply, reply);
        assert!(!state.request(lead).unwrap().uncertain_outcome);
        assert!(state.request(lead).unwrap().failure_reason.is_none());
        assert_eq!(
            state.request(joined).unwrap().phase,
            RequestPhase::Abandoned
        );
        assert!(state.request(joined).unwrap().uncertain_outcome);
        assert_eq!(
            state.request(joined).unwrap().failure_reason.as_deref(),
            Some("turn_end_unconfirmed")
        );
        assert_eq!(state.agent(agent).unwrap().current_request, None);
    }
}

#[test]
fn idle_backstop_requires_continuous_idle_and_a_fresh_poll_including_after_legacy_reload() {
    for provider in providers() {
        for dialog in [false, true] {
            let (mut state, room, agent, _) = state_with_room_and_agents();
            state.agents.get_mut(&agent).unwrap().provider = provider;
            state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
            let lead = submit_text(
                &mut state,
                room,
                agent,
                "idle interrupted by a dialog or blocked frame",
            );
            start_request(&mut state, lead, "launch-codex", 100);
            // Serialized old states have no observation history. They must
            // bootstrap from a new poll rather than expire from an old clock.
            let mut saved = serde_json::to_value(state).unwrap();
            saved["requests"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .for_each(|r| {
                    r.as_object_mut().unwrap().remove("bound_turn_activity");
                });
            let mut state: BusState = serde_json::from_value(saved).unwrap();
            assert!(state
                .settle_ended_request(agent, 100_000, |_| None)
                .is_none());
            state
                .observe_status(agent, RuntimeStatus::Idle, 100_000)
                .unwrap();
            assert!(
                state
                    .settle_ended_request(agent, 130_000, |_| None)
                    .is_none(),
                "a cached pre-Started frame cannot expire the watch"
            );
            if dialog {
                state.observe_dialog(agent, true, 120_000).unwrap();
            } else {
                state
                    .observe_status(agent, RuntimeStatus::Blocked, 120_000)
                    .unwrap();
            }
            state
                .observe_status(
                    agent,
                    if dialog {
                        RuntimeStatus::Idle
                    } else {
                        RuntimeStatus::Blocked
                    },
                    150_000,
                )
                .unwrap();
            assert!(state
                .settle_ended_request(agent, 150_000, |_| None)
                .is_none());
            if dialog {
                state.observe_dialog(agent, false, 150_001).unwrap();
            }
            state
                .observe_status(agent, RuntimeStatus::Idle, 150_001)
                .unwrap();
            if provider == Provider::Kimi {
                assert!(
                    state
                        .settle_ended_request(agent, 180_001, |_| None)
                        .is_none(),
                    "a cached poll cannot expire the watch"
                );
                state
                    .observe_status(agent, RuntimeStatus::Idle, 180_000)
                    .unwrap();
                assert!(state
                    .settle_ended_request(agent, 180_000, |_| None)
                    .is_none());
            }
            state
                .observe_status(agent, RuntimeStatus::Idle, 180_001)
                .unwrap();
            // A fresh Idle transition also has normal trusted end evidence
            // for non-Kimi providers; both paths must release by this bound.
            assert_eq!(
                state.settle_ended_request(agent, 180_001, |_| None),
                Some(lead)
            );
        }
    }
}

#[test]
fn idle_backstop_is_disabled_by_working_even_with_clock_rollback_and_more_started_input() {
    for provider in providers() {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.agents.get_mut(&agent).unwrap().provider = provider;
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "long foreground command");
        start_request(&mut state, lead, "launch-codex", 100);
        state
            .observe_status(agent, RuntimeStatus::Idle, 283)
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Working, 100)
            .unwrap();
        let payload = state.request(lead).unwrap().prompt.rendered_payload();
        let disposition = state.accept_callback(ProviderCallback {
            callback_id: "same-turn-input".into(),
            sequence: 300,
            occurred_at_ms: 300,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: Some("next-prompt".into()),
            prompt_payload: Some(payload),
            kind: CallbackEventKind::PromptStarted,
        });
        assert!(
            !matches!(disposition, CallbackDisposition::Rejected(_)),
            "{provider:?}: {disposition:?}"
        );
        assert!(
            state
                .request(lead)
                .unwrap()
                .bound_turn_activity
                .as_ref()
                .unwrap()
                .working_seen
        );
        // Roll back to an Idle clock earlier than Started: no trustworthy
        // idle edge and, despite many seconds, no never-Working timeout.
        state
            .observe_status(agent, RuntimeStatus::Idle, 99)
            .unwrap();
        state
            .observe_status(agent, RuntimeStatus::Idle, 100_000)
            .unwrap();
        let mut state: BusState =
            serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        assert!(state
            .settle_ended_request(agent, 100_000, |_| None)
            .is_none());
        assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Active);
    }
}

#[test]
fn idle_backstop_leaves_background_waits_retry_holds_and_accepted_finals_authoritative() {
    for provider in providers() {
        for protected in 0..3 {
            if protected == 1 && provider == Provider::Cursor {
                continue;
            }
            let (mut state, room, agent, _) = state_with_room_and_agents();
            state.agents.get_mut(&agent).unwrap().provider = provider;
            state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
            let lead = submit_text(&mut state, room, agent, "protected turn");
            start_request(&mut state, lead, "launch-codex", 100);
            match protected {
                0 => state.requests.get_mut(&lead).unwrap().awaiting_background = true,
                1 => assert!(state.hold_provider_error(
                    lead,
                    "Model at capacity (server_overloaded)",
                    150
                )),
                _ => {
                    let payload = state.request(lead).unwrap().prompt.rendered_payload();
                    assert_eq!(
                        state.accept_callback(ProviderCallback::final_event(
                            "real-final",
                            150,
                            agent,
                            "launch-codex",
                            "provider-session",
                            "turn-1",
                            &payload,
                            "done"
                        )),
                        CallbackDisposition::AcceptedCompleted
                    );
                }
            }
            state
                .observe_status(agent, RuntimeStatus::Idle, 283)
                .unwrap();
            state
                .observe_status(agent, RuntimeStatus::Idle, 100_000)
                .unwrap();
            assert!(state
                .settle_ended_request(agent, 100_000, |_| None)
                .is_none());
            assert!(!state.request(lead).unwrap().uncertain_outcome);
            assert_eq!(
                state.request(lead).unwrap().phase,
                if protected == 2 {
                    RequestPhase::Completed
                } else {
                    RequestPhase::Active
                }
            );
        }
    }
}
