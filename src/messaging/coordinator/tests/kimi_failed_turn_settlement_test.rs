use super::*;

fn captured(name: &str, payload: &str) -> Vec<Value> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/kimi-failed-turn/{name}.json"));
    let mut rows: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    rows[0]["input"][0]["text"] = json!(payload);
    rows
}

fn write_wire(path: &Path, rows: &[Value]) {
    std::fs::write(
        path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

fn failed_then_drain(restart: bool) {
    for name in ["rr-owner", "cc-owner"] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = home(&f);
        let lead = running(&mut f, "failed task", "2");
        let payload = f.worker.state.render_request_payload(lead).unwrap();
        let rows = captured(name, &payload);
        let pending_commit = rows
            .iter()
            .position(|row| row["type"] == "prompt.completed")
            .unwrap();
        write_wire(&wire, &rows[..pending_commit]);
        f.status(RuntimeStatus::Idle);
        let first = f.send("first after failure", true);
        let second = f.send("second after failure", true);
        if restart {
            reopen(&mut f);
            f.status(RuntimeStatus::Idle);
        }
        f.consume();
        assert!(
            !f.request(lead).settled(),
            "wait for the matching native prompt commit"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_none());
        write_wire(&wire, &rows);
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert!(f.request(lead).failure_reason.is_none());
        assert!(f.request(lead).turn_ended_at_ms.is_some());
        let hold = f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .clone()
            .expect("the committed quota failure must create a retry owner");
        assert_eq!(hold.lead, lead, "{name}, restart={restart}");
        assert!(hold.quota && !hold.queued);
        assert_eq!(hold.attempts, 1);
        assert!(hold.retry_at_ms > f.now);
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().shown_status(),
            RuntimeStatus::Working
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().activity_status(),
            RuntimeStatus::Idle
        );
        if restart {
            reopen(&mut f);
            f.status(RuntimeStatus::Idle);
            assert_eq!(
                f.worker
                    .state
                    .agent(f.agent)
                    .unwrap()
                    .provider_retry
                    .as_ref(),
                Some(&hold)
            );
        }
        f.now = hold.retry_at_ms - 1;
        assert!(
            f.now
                .saturating_sub(f.request(first).prompt.submitted_at_ms)
                > crate::messaging::model::QUEUED_STALL_MS
        );
        f.worker.expire_queued_requests(f.now).unwrap();
        for id in [first, second] {
            assert_eq!(f.request(id).phase, RequestPhase::Queued);
            assert!(f.request(id).failure_reason.is_none());
        }
        f.consume();
        f.deliver();
        assert_eq!(
            f.typed().len(),
            1,
            "no retry or follow-up before the hold is due"
        );
        assert_eq!(
            f.worker
                .state
                .agent(f.agent)
                .unwrap()
                .provider_retry
                .as_ref(),
            Some(&hold)
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );

        // Model the domain's authorized due retry, preserving the same request
        // before later queue entries. This is not a live availability probe.
        f.now = hold.retry_at_ms;
        f.status(RuntimeStatus::Idle);
        let mut state = f.worker.state.clone();
        assert!(state.retry_provider_requests(f.now));
        assert_eq!(state.queued_requests(f.agent), &[lead, first, second]);
        f.worker.save(state).unwrap();
        f.worker.own_turns.remove(&f.agent);
        f.deliver();
        assert_eq!(f.typed().len(), 2, "retry the same lead once");
        let retry_payload = f.typed()[1].0.clone();
        assert!(retry_payload.contains("Continue the existing task"));
        f.started("4", &retry_payload);
        f.consume();
        assert!(f.request(lead).trusted_start_bound);
        assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("4"));
        let retried = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>()
            + &native_turn(4, &retry_payload, &[], "retried task completed");
        std::fs::write(&wire, &retried).unwrap();
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert_eq!(f.reply(lead).as_deref(), Some("retried task completed"));
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_none());
        assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
        f.worker.own_turns.remove(&f.agent);
        f.deliver();
        assert_eq!(
            f.typed().len(),
            3,
            "the first queued message must get one delivery"
        );
        assert!(f.typed()[2].0.contains("first after failure"));
        assert_eq!(f.request(first).phase, RequestPhase::Active);
        assert_eq!(f.request(second).phase, RequestPhase::Queued);
        f.deliver();
        assert_eq!(
            f.typed().len(),
            3,
            "no retry or replay of the accepted follow-up"
        );
        let first_payload = f.typed()[2].0.clone();
        f.started("5", &first_payload);
        f.consume();
        f.status(RuntimeStatus::Working);
        f.now += crate::messaging::model::QUEUED_STALL_MS + 1;
        f.worker.expire_queued_requests(f.now).unwrap();
        assert_eq!(f.request(second).phase, RequestPhase::Queued);
        let first_done = retried + &native_turn(5, &first_payload, &[], "first completed");
        std::fs::write(&wire, &first_done).unwrap();
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert_eq!(f.reply(first).as_deref(), Some("first completed"));
        f.worker.own_turns.remove(&f.agent);
        f.deliver();
        assert_eq!(f.typed().len(), 4);
        assert!(f.typed()[3].0.contains("second after failure"));
        assert_eq!(f.request(second).phase, RequestPhase::Active);
        let second_payload = f.typed()[3].0.clone();
        f.started("6", &second_payload);
        f.consume();
        std::fs::write(
            &wire,
            first_done + &native_turn(6, &second_payload, &[], "second completed"),
        )
        .unwrap();
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert_eq!(f.reply(second).as_deref(), Some("second completed"));
        assert_eq!(
            f.typed().len(),
            4,
            "one original attempt, one retry, and FIFO follow-ups"
        );
    }
}

#[test]
fn kimi_failed_turn_quota_hold_preserves_queue_until_retry_succeeds() {
    failed_then_drain(false);
}

#[test]
fn kimi_failed_turn_pending_records_resume_into_a_durable_retry_hold() {
    failed_then_drain(true);
}

#[test]
fn kimi_failed_turn_interleaved_native_queue_keeps_its_own_turn() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = home(&f);
    let lead = running(&mut f, "failed task", "2");
    let next = f.send("native queued follow-up", false);
    f.deliver();
    let next_payload = f.typed()[1].0.clone();
    let payload = f.worker.state.render_request_payload(lead).unwrap();
    let mut rows = captured("cc-owner", &payload);
    rows[1]["promptId"] = json!("queued-native-prompt");
    rows[1]["input"][0]["text"] = json!(next_payload);
    rows[4]["errorMessage"] = json!("owned turn failed");
    rows[5]["errorMessage"] = json!("another turn failed");
    rows[2]["error"]["message"] = json!("owned turn failed");
    rows[6]["error"]["message"] = json!("another turn failed");
    // The next prompt is queued natively, not attested as guidance consumed by turn 2.
    write_wire(&wire, &rows);
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .provider_retry
        .is_none());
    let reason = f.request(lead).failure_reason.as_deref().unwrap();
    assert!(
        reason.starts_with("provider_error:") && reason.contains("owned turn failed"),
        "{reason}"
    );
    assert!(!reason.contains("another turn failed"));
    assert!(!f.request(next).trusted_start_bound);
    assert_ne!(f.request(next).phase, RequestPhase::Completed);
    assert_eq!(
        f.typed().len(),
        2,
        "do not replay an already accepted native input"
    );
}
