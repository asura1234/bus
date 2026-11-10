use super::*;

#[test]
fn kimi_quota_exhaustion_closes_fresh_and_restored_requests_without_retry() {
    for native in [
        include_str!("../../../agents/providers/tests/fixtures/kimi-monthly-quota-error.jsonl"),
        include_str!("../../../agents/providers/tests/fixtures/kimi-quota-error.jsonl"),
    ] {
        for held in [false, true] {
            for restart in [false, true] {
                let mut f = Fixture::new(Provider::Kimi);
                let wire = super::kimi_steering_tests::home(&f);
                let lead = running(&mut f, "quota task", "0");
                let mut rows: Vec<Value> = native
                    .lines()
                    .map(|line| serde_json::from_str(line).unwrap())
                    .collect();
                rows[0]["input"][0]["text"] =
                    f.worker.state.render_request_payload(lead).unwrap().into();
                let error = rows.iter().find(|row| row["type"] == "turn.ended").unwrap()["error"]
                    ["message"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                std::fs::write(
                    &wire,
                    rows.iter()
                        .map(|row| format!("{row}\n"))
                        .collect::<String>(),
                )
                .unwrap();
                if held {
                    let mut saved = serde_json::to_value(&f.worker.state).unwrap();
                    saved["agents"][f.agent.0.to_string()]["provider_retry"] = json!({"lead":lead,"attempts":1,"retry_at_ms":f.now + 300_000,"quota":true,"queued":false});
                    saved["requests"][lead.0.to_string()]["turn_ended_at_ms"] = json!(f.now);
                    f.worker
                        .save(serde_json::from_value(saved).unwrap())
                        .unwrap();
                }
                if restart {
                    super::kimi_steering_tests::reopen(&mut f);
                }
                f.record(json!({"hook_event_name":"StopFailure","session_id":"session"}));
                f.consume();
                assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
                let status = f.worker.control_message(f.request(lead).prompt.id).unwrap();
                assert_eq!(status["complete"], true);
                assert_eq!(status["requests"][0]["turn_ended"], true);
                assert_eq!(
                    status["requests"][0]["reason"],
                    format!("provider_error: APIProviderQuotaExhaustedError: {error}")
                );
                assert!(status["requests"][0]["reply"].is_null());
                assert!(f.error().unwrap().contains(&error));
                let owner = f.worker.state.agent(f.agent).unwrap();
                assert!(owner.current_request.is_none());
                assert!(owner.provider_retry.is_none());
                f.status(RuntimeStatus::Idle);
                assert_eq!(
                    f.worker.state.agent(f.agent).unwrap().shown_status(),
                    RuntimeStatus::Idle
                );
                assert!(!f.worker.state.retry_provider_requests(f.now + 3_600_000));
                assert_eq!(
                    f.typed().len(),
                    1,
                    "the exhausted task must not be replayed"
                );
                super::kimi_steering_tests::reopen(&mut f);
                assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
                assert!(f.error().unwrap().contains(&error));
            }
        }
    }
}

#[test]
fn kimi_error_queue_preserves_a_busy_turns_followup_across_idle_and_restart() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = super::kimi_steering_tests::home(&f);
    let lead = running(&mut f, "foreground work", "0");
    let prompt = f.worker.state.render_request_payload(lead).unwrap();
    let open = json!({"type":"turn.prompt", "agentId":"main", "turnId":0, "promptId":"p0", "input":[{"type":"text","text":prompt}]}).to_string() + "\n";
    std::fs::write(&wire, &open).unwrap();
    let next = f.send("later queued work", true);
    f.status(RuntimeStatus::Idle);
    f.now += crate::messaging::model::QUEUED_STALL_MS * 2;
    f.worker.expire_queued_requests(f.now).unwrap();
    assert_eq!(
        f.request(next).phase,
        RequestPhase::Queued,
        "an owned active turn is not a failed delivery opportunity"
    );
    assert!(f.request(next).failure_reason.is_none());
    super::kimi_steering_tests::reopen(&mut f);
    f.status(RuntimeStatus::Idle);
    f.worker
        .expire_queued_requests(f.now + crate::messaging::model::QUEUED_STALL_MS)
        .unwrap();
    assert_eq!(f.request(next).phase, RequestPhase::Queued);
    std::fs::write(
        &wire,
        super::kimi_steering_tests::native_turn(0, &prompt, &[], "first answer"),
    )
    .unwrap();
    f.consume();
    assert!(f.reply(lead).is_none());
    assert_eq!(f.request(lead).delivery_stage(), "ended");
    f.deliver();
    assert_eq!(
        f.typed_content(),
        [
            ("foreground work".into(), false),
            ("later queued work".into(), false)
        ]
    );
    f.started("1", "later queued work");
    f.consume();
    let next_prompt = f.worker.state.render_request_payload(next).unwrap();
    let finished = super::kimi_steering_tests::native_turn(0, &prompt, &[], "first answer")
        + &super::kimi_steering_tests::native_turn(1, &next_prompt, &[], "second answer");
    std::fs::write(&wire, finished).unwrap();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert!(f.reply(next).is_none());
    assert_eq!(f.request(next).delivery_stage(), "ended");
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
}
