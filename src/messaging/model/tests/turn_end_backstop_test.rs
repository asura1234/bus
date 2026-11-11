use super::*;

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

#[test]
fn idle_backstop_does_not_expire_immediately_after_accepted_same_turn_direct_input() {
    for restart in [false, true] {
        let (mut state, room, agent, _) = state_with_room_and_agents();
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        let lead = submit_text(&mut state, room, agent, "original task");
        start_request(&mut state, lead, "launch-codex", 100);
        state
            .observe_status(agent, RuntimeStatus::Idle, 283)
            .unwrap();
        let queued = submit_text(&mut state, room, agent, "queued independent task");

        // This is a fresh native input in the already bound turn, not a
        // duplicate of the Bus prompt or a replay of its original callback.
        // The unchanged native Idle frame may still be visible after Started.
        let disposition = state.accept_callback(ProviderCallback {
            callback_id: "fresh-native-input-near-idle-deadline".into(),
            sequence: 102,
            occurred_at_ms: 30_282,
            agent_id: agent,
            launch_id: "launch-codex".into(),
            provider_session_id: Some("provider-session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("Use the corrected deployment token in this task".into()),
            kind: CallbackEventKind::PromptStarted,
        });
        assert_eq!(disposition, CallbackDisposition::AcceptedProgress);
        assert_eq!(state.request(lead).unwrap().progress_at_ms, Some(30_282));
        assert!(
            !state
                .request(lead)
                .unwrap()
                .bound_turn_activity
                .as_ref()
                .unwrap()
                .working_seen
        );
        if restart {
            state = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
        }
        state
            .observe_status(agent, RuntimeStatus::Idle, 30_283)
            .unwrap();
        assert!(state.request(lead).unwrap().turn_ended_at_ms.is_none());
        assert_eq!(
            state.settle_ended_request(agent, 30_283, |_| None),
            None,
            "accepted native input must have a fresh grace before an uncertain release; restart={restart}"
        );
        assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Active);
        assert!(!state.request(lead).unwrap().uncertain_outcome);
        assert_eq!(state.agent(agent).unwrap().current_request, Some(lead));
        assert_eq!(state.request(queued).unwrap().phase, RequestPhase::Queued);
    }
}

#[test]
fn fresh_native_started_renews_grace_but_idle_polls_and_rejected_replay_do_not() {
    for provider in providers() {
        for restart in [false, true] {
            let (mut state, room, agent, _) = state_with_room_and_agents();
            state.agents.get_mut(&agent).unwrap().provider = provider;
            state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
            let lead = submit_text(&mut state, room, agent, "original task");
            start_request(&mut state, lead, "launch-codex", 100);
            state
                .observe_status(agent, RuntimeStatus::Idle, 283)
                .unwrap();
            let queued = submit_text(&mut state, room, agent, "independent queued task");
            let fresh = ProviderCallback {
                callback_id: "genuine-native-input-near-deadline".into(),
                sequence: 102,
                occurred_at_ms: 30_282,
                agent_id: agent,
                launch_id: "launch-codex".into(),
                provider_session_id: Some("provider-session".into()),
                provider_turn_id: Some("turn-1".into()),
                provider_prompt_id: None,
                prompt_payload: Some("use the corrected native input".into()),
                kind: CallbackEventKind::PromptStarted,
            };
            assert_eq!(
                state.accept_callback(fresh.clone()),
                CallbackDisposition::AcceptedProgress,
                "{provider:?}"
            );
            state
                .observe_status(agent, RuntimeStatus::Idle, 30_283)
                .unwrap();
            assert!(state
                .settle_ended_request(agent, 30_283, |_| None)
                .is_none());
            if restart {
                state = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
            }
            for now in [40_000, 50_000, 60_282] {
                state
                    .observe_status(agent, RuntimeStatus::Idle, now)
                    .unwrap();
                assert!(state.settle_ended_request(agent, now, |_| None).is_none());
            }
            let mut replay = fresh;
            replay.occurred_at_ms = 60_282;
            assert_eq!(
                state.accept_callback(replay),
                CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback)
            );
            state
                .observe_status(agent, RuntimeStatus::Idle, 60_283)
                .unwrap();
            assert_eq!(
                state.settle_ended_request(agent, 60_283, |_| None),
                Some(lead),
                "{provider:?}; restart={restart}"
            );
            assert_eq!(state.request(lead).unwrap().phase, RequestPhase::Abandoned);
            assert!(state.request(lead).unwrap().uncertain_outcome);
            assert_eq!(
                state.request(lead).unwrap().failure_reason.as_deref(),
                Some("turn_end_unconfirmed")
            );
            assert!(state.request(lead).unwrap().turn_ended_at_ms.is_none());
            assert_eq!(state.agent(agent).unwrap().current_request, None);
            assert_eq!(state.next_queued_request(agent), Some(queued));
        }
    }
}
