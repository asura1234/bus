use super::*;

#[test]
fn idle_backstop_recovers_codex_rollout_or_waits_on_missing_evidence_including_a_resumed_start() {
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
                assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
                assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
                fixture.now = first_idle + 299_998;
                fixture.status(RuntimeStatus::Idle);
                fixture.worker.settle_requests().unwrap();
                assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
                fixture.now = first_idle + 299_999;
                fixture.status(RuntimeStatus::Idle);
                fixture.worker.settle_requests().unwrap();
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

fn idle_codex_turn() -> (Fixture, RequestId, std::path::PathBuf, String) {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-silent-turn.jsonl");
    let lead = fixture.send("run a long silent tool", false);
    fixture.deliver();
    let typed = fixture.typed().last().unwrap().0.clone();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":typed,"transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert!(fixture.request(lead).trusted_start_bound);
    (fixture, lead, rollout, typed)
}

#[test]
fn idle_backstop_keeps_a_silent_codex_tool_turn_until_its_owned_final() {
    let (mut fixture, lead, rollout, typed) = idle_codex_turn();
    let first_idle = fixture.now;
    append(
        &rollout,
        &codex_turn(&rollout_at(&fixture, 0), "turn-1", &typed, None),
    );
    let started = json!({"timestamp":rollout_at(&fixture, 20_000),"type":"response_item",
        "payload":{"type":"function_call","name":"exec_command","call_id":"tool-1","arguments":"{}"}});
    append(&rollout, &format!("{started}\n"));
    let queued = fixture.send("do not release this into the live turn", true);
    for elapsed in [31_800, 264_999, 301_000] {
        fixture.now = first_idle + elapsed - 1;
        fixture.status(RuntimeStatus::Idle);
        fixture.worker.settle_requests().unwrap();
        fixture.deliver();
        assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
        assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
    }
    let complete = json!({"timestamp":rollout_at(&fixture, 1),"type":"event_msg",
        "payload":{"type":"task_complete","turn_id":"turn-1","last_agent_message":"owned final"}});
    append(&rollout, &format!("{complete}\n"));
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_completed_without_reply(&fixture, lead);
    assert_eq!(fixture.reply(lead).as_deref(), Some("owned final"));
    let closed_at = fixture.request(lead).turn_ended_at_ms;
    fixture.stopped("turn-1", "late duplicate must not replace final");
    fixture.consume();
    fixture.worker.settle_requests().unwrap();
    assert_eq!(fixture.request(lead).turn_ended_at_ms, closed_at);
    assert_eq!(fixture.reply(lead).as_deref(), Some("owned final"));
    fixture.deliver();
    assert_ne!(fixture.request(queued).phase, RequestPhase::Queued);
}

#[test]
fn idle_backstop_settles_a_textless_codex_completion_at_thirty_seconds() {
    let (mut fixture, lead, rollout, typed) = idle_codex_turn();
    append(
        &rollout,
        &codex_turn(&rollout_at(&fixture, 0), "turn-1", &typed, None),
    );
    let complete = json!({"timestamp":rollout_at(&fixture, 1),"type":"event_msg",
        "payload":{"type":"task_complete","turn_id":"turn-1"}});
    append(&rollout, &format!("{complete}\n"));
    fixture.now += 29_999;
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_completed_without_reply(&fixture, lead);
    assert!(fixture.reply(lead).is_none());
}

#[test]
fn idle_backstop_still_exempts_agy() {
    let mut fixture = Fixture::new(Provider::Agy);
    let lead = fixture.send("run without a Working observation", false);
    fixture.deliver();
    fixture.started("0", &fixture.typed().last().unwrap().0);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert!(fixture.request(lead).trusted_start_bound);
    fixture.now += 300_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
}
