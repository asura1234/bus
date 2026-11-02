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
