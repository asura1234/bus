use super::*;

fn incident() -> Value {
    serde_json::from_str(include_str!("fixtures/kimi-41980.json")).unwrap()
}

fn failure_wire(f: &mut Fixture, lead: RequestId, case: usize) -> String {
    let mut rows = incident()["wire"].as_array().unwrap().clone();
    rows[0]["input"][0]["text"] = f.worker.state.render_request_payload(lead).unwrap().into();
    match case {
        1 => {
            rows.remove(2); // The owned prompt has not committed yet.
        }
        2 => rows[1]["turnId"] = 99.into(),
        _ => {}
    }
    rows.into_iter().map(|row| format!("{row}\n")).collect()
}

#[test]
fn kimi_41980_committed_failure_revokes_working_and_blocked_turn() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Blocked] {
        for restart in [false, true] {
            for stop in [false, true] {
                let mut f = Fixture::new(Provider::Kimi);
                let wire = super::kimi_steering_tests::home(&f);
                let lead = running(&mut f, "original task", "0");
                let next = f.send(incident()["followup"].as_str().unwrap(), false);
                std::fs::write(&wire, failure_wire(&mut f, lead, 0)).unwrap();
                if restart {
                    super::kimi_steering_tests::reopen(&mut f);
                }
                f.status(status);
                if stop {
                    f.record(json!({"hook_event_name":"Stop","session_id":"session"}));
                }
                f.consume();
                assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
                assert!(f
                    .request(lead)
                    .failure_reason
                    .as_deref()
                    .unwrap()
                    .starts_with("provider_error: APIProviderQuotaExhaustedError: 429"));
                assert!(f
                    .worker
                    .state
                    .agent(f.agent)
                    .unwrap()
                    .current_request
                    .is_none());
                f.deliver();
                assert_eq!(f.typed().len(), 1, "no write into a failed native turn");
                assert_eq!(f.request(next).phase, RequestPhase::Queued);
                assert_eq!(f.request(next).group, None);
                f.status(RuntimeStatus::Idle);
                f.deliver();
                assert_eq!(f.typed().len(), 2);
                assert!(!f.typed()[1].1, "the next message gets an independent turn");
            }
        }
    }
}

#[test]
fn kimi_41980_incomplete_or_foreign_failure_keeps_the_owned_turn() {
    for case in [1, 2] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        std::fs::write(&wire, failure_wire(&mut f, lead, case)).unwrap();
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
    }
}

struct TimedOutSteering(Arc<Mutex<Vec<Method>>>);

impl Transport for TimedOutSteering {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        Native(self.0.clone()).request(method)?;
        Err(TransportError {
            code: Some("submission_unconfirmed".into()),
            message: incident()["source"]["detail"].as_str().unwrap().into(),
            definitely_rejected: false,
        })
    }
}

fn uncertain_member(f: &mut Fixture) -> RequestId {
    f.worker.transport = Box::new(TimedOutSteering(f.native.clone()));
    let member = f.send(incident()["followup"].as_str().unwrap(), false);
    f.deliver();
    member
}

#[test]
fn kimi_41980_uncertain_steering_holds_followups_until_a_native_receipt() {
    for legacy in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let member = uncertain_member(&mut f);
        if legacy {
            // The live 41981 snapshot persisted Active + unbound uncertainty.
            let mut state = serde_json::to_value(&f.worker.state).unwrap();
            state["requests"][member.0.to_string()]["phase"] = "active".into();
            f.worker
                .save(serde_json::from_value(state).unwrap())
                .unwrap();
        }
        let waiting = f.send("next queued work", false);
        super::kimi_steering_tests::reopen(&mut f);
        f.status(RuntimeStatus::Working);
        let status = f
            .worker
            .control_message(f.request(member).prompt.id)
            .unwrap();
        assert_eq!(status["requests"][0]["stage"], "submitting");
        assert_eq!(status["requests"][0]["start_bound"], false);
        assert_eq!(status["requests"][0]["uncertain_outcome"], true);
        assert_eq!(status["complete"], false);
        assert!(status["requests"][0]["reply"].is_null());
        assert!(status["requests"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("Kimi submission may have run; inspect the transcript before resending"));
        f.now += crate::messaging::model::QUEUED_STALL_MS * 2;
        f.expire_after_reconciliation(f.now);
        f.deliver();
        assert_eq!(
            f.typed().len(),
            2,
            "neither retry nor another composer write"
        );
        assert_eq!(f.request(waiting).phase, RequestPhase::Queued);
        assert_eq!(f.request(member).group, Some(lead));
        f.started("0", incident()["followup"].as_str().unwrap());
        f.consume();
        let status = f
            .worker
            .control_message(f.request(member).prompt.id)
            .unwrap();
        assert_eq!(status["requests"][0]["stage"], "joined");
        assert_eq!(status["requests"][0]["start_bound"], true);
        assert_eq!(status["requests"][0]["uncertain_outcome"], false);
        f.deliver();
        assert_eq!(f.typed().len(), 3, "receipt releases the following message");
        assert!(f.typed()[2].1);
    }
}

#[test]
fn kimi_41980_uncertain_steering_ends_with_provider_failure_without_retry() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = super::kimi_steering_tests::home(&f);
    let lead = running(&mut f, "original task", "0");
    let member = uncertain_member(&mut f);
    let waiting = f.send("next queued work", false);
    std::fs::write(wire, failure_wire(&mut f, lead, 0)).unwrap();
    f.consume();
    assert_eq!(f.request(member).phase, RequestPhase::Abandoned);
    assert!(!f.request(member).trusted_start_bound);
    assert!(f.reply(member).is_none());
    assert_eq!(f.request(waiting).phase, RequestPhase::Queued);
    f.status(RuntimeStatus::Idle);
    f.worker.transport = Box::new(Native(f.native.clone()));
    f.deliver();
    assert_eq!(f.typed().len(), 3);
    assert_eq!(f.typed_content()[2], ("next queued work".into(), false));
}

#[test]
fn kimi_41980_restart_before_write_acknowledgment_holds_the_next_message() {
    let mut f = Fixture::new(Provider::Kimi);
    super::kimi_steering_tests::home(&f);
    let lead = running(&mut f, "original task", "0");
    let pending = f.send(incident()["followup"].as_str().unwrap(), false);
    let mut state = f.worker.state.clone();
    state.render_request_payload(pending).unwrap();
    state.begin_steering(pending, lead).unwrap();
    f.worker.save(state).unwrap();
    let waiting = f.send("next queued work", false);
    super::kimi_steering_tests::reopen(&mut f);
    f.status(RuntimeStatus::Working);
    f.deliver();
    assert_eq!(
        f.typed().len(),
        1,
        "the lost outcome does not free the composer"
    );
    assert_eq!(f.request(waiting).phase, RequestPhase::Queued);
    f.started("0", incident()["followup"].as_str().unwrap());
    f.consume();
    f.deliver();
    assert_eq!(f.typed().len(), 2, "a native receipt releases the queue");
}

#[test]
fn kimi_41980_partial_busy_wire_is_not_a_committed_provider_failure() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Blocked] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let open = failure_wire(&mut f, lead, 0)
            .lines()
            .next()
            .unwrap()
            .to_owned();
        std::fs::write(wire, format!("{open}\n{{\"type\":")).unwrap();
        f.status(status);
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
    }
}
