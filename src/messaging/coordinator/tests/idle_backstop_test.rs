use super::*;

#[test]
fn idle_backstop_recovers_codex_rollout_or_releases_uncertain_including_a_resumed_start() {
    for recorded in [false, true] {
        for restart in [false, true] {
            let mut fixture = Fixture::new(Provider::Codex);
            let rollout = fixture.dir.join("rollout-never-working.jsonl");
            let lead = fixture.send("finish without a status edge or Stop hook", false);
            fixture.deliver();
            if restart {
                reload(&mut fixture);
                // Native readiness is observed before the resumed Started.
                // Its repeated Idle frame cannot become a trusted turn end.
                fixture.status(RuntimeStatus::Idle);
            }
            let typed = fixture.typed().last().unwrap().0.clone();
            fixture.record(json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "turn_id":"turn-1", "prompt":typed, "transcript_path":rollout}));
            fixture.consume();
            fixture.status(RuntimeStatus::Idle);
            assert!(
                fixture.request(lead).trusted_start_bound,
                "{:?}",
                fixture.request(lead)
            );
            assert!(
                fixture.request(lead).bound_turn_activity.is_some(),
                "{:?}",
                fixture.request(lead)
            );
            let first_idle = fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .observed_at_ms;
            let queued = fixture.send("queued after never-Working turn", true);
            if recorded {
                append(
                    &rollout,
                    &codex_turn(
                        &rollout_at(&fixture, 1),
                        "turn-1",
                        &typed,
                        Some("owned terminal record"),
                    ),
                );
            }
            fixture.now = first_idle + 29_998;
            fixture.status(RuntimeStatus::Idle);
            fixture.worker.settle_requests().unwrap();
            assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
            assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
            fixture.now = first_idle + 30_000;
            fixture.status(RuntimeStatus::Idle);
            fixture.worker.settle_requests().unwrap();
            if recorded {
                assert_completed_without_reply(&fixture, lead);
            } else {
                assert_eq!(fixture.request(lead).phase, RequestPhase::Abandoned, "recorded={recorded} restart={restart}; request={:?}; agent={:?}; in_flight={:?}", fixture.request(lead), fixture.worker.state.agent(fixture.agent), fixture.worker.native_submissions.keys());
                assert_eq!(
                    fixture.request(lead).failure_reason.as_deref(),
                    Some("turn_end_unconfirmed")
                );
                assert!(fixture.request(lead).uncertain_outcome);
                assert_eq!(fixture.request(lead).delivery_stage(), "abandoned");
                assert!(fixture.request(lead).turn_ended_at_ms.is_none());
            }
            assert_eq!(
                fixture
                    .worker
                    .state
                    .agent(fixture.agent)
                    .unwrap()
                    .current_request,
                None
            );
            fixture.deliver();
            assert_ne!(fixture.request(queued).phase, RequestPhase::Queued);
        }
    }
}
