use super::*;

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
    assert_eq!(f.reply(lead).as_deref(), Some("first answer"));
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
    assert_eq!(f.reply(next).as_deref(), Some("second answer"));
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .current_request
        .is_none());
}
