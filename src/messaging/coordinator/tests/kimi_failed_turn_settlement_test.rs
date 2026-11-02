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
        write_wire(&wire, &rows);
        f.consume();
        assert_eq!(
            f.request(lead).phase,
            RequestPhase::Abandoned,
            "{name}, restart={restart}"
        );
        assert!(f
            .request(lead)
            .failure_reason
            .as_deref()
            .unwrap()
            .starts_with("provider_error:"));
        assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
        f.worker.own_turns.remove(&f.agent);
        f.deliver();
        assert_eq!(
            f.typed().len(),
            2,
            "the first queued message must get one delivery"
        );
        assert!(f.typed()[1].0.contains("first after failure"));
        assert_eq!(f.request(first).phase, RequestPhase::Active);
        assert_eq!(f.request(second).phase, RequestPhase::Queued);
        f.deliver();
        assert_eq!(
            f.typed().len(),
            2,
            "no retry or replay of the accepted follow-up"
        );
        let first_payload = f.typed()[1].0.clone();
        f.started("4", &first_payload);
        f.consume();
        f.status(RuntimeStatus::Working);
        f.now += crate::messaging::model::QUEUED_STALL_MS + 1;
        f.worker.expire_queued_requests(f.now).unwrap();
        assert_eq!(f.request(second).phase, RequestPhase::Queued);
        std::fs::write(
            &wire,
            format!(
                "{}{}",
                rows.iter()
                    .map(|row| format!("{row}\n"))
                    .collect::<String>(),
                native_turn(4, &first_payload, &[], "first completed")
            ),
        )
        .unwrap();
        f.status(RuntimeStatus::Idle);
        f.consume();
        assert_eq!(f.reply(first).as_deref(), Some("first completed"));
        f.worker.own_turns.remove(&f.agent);
        f.deliver();
        assert_eq!(f.typed().len(), 3);
        assert!(f.typed()[2].0.contains("second after failure"));
        assert_eq!(f.request(second).phase, RequestPhase::Active);
    }
}

#[test]
fn kimi_failed_turn_closure_releases_the_queue_in_order() {
    failed_then_drain(false);
}

#[test]
fn kimi_failed_turn_pending_records_survive_restart_and_release_the_queue() {
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
