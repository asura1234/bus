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
        // Synthetic permanent failure: the durable recovery log still covers closure.
        3 => rows[1]["error"] = json!({"name":"AuthenticationError", "message":"401 unauthorized"}),
        _ => {}
    }
    rows.into_iter().map(|row| format!("{row}\n")).collect()
}

fn assert_quota_hold(f: &Fixture, lead: RequestId) {
    let request = f.request(lead);
    assert_eq!(request.phase, RequestPhase::Active);
    assert!(!request.settled());
    assert!(request.failure_reason.is_none());
    assert_eq!(request.provider_turn_id.as_deref(), Some("0"));
    assert!(
        request.turn_ended_at_ms.is_some(),
        "native failure must end the turn"
    );
    let hold = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .provider_retry
        .as_ref()
        .unwrap();
    assert_eq!(hold.lead, lead);
    assert!(hold.quota);
}

#[test]
fn kimi_41980_committed_failure_holds_delivery_despite_stale_status() {
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
                assert_quota_hold(&f, lead);
                f.deliver();
                assert_eq!(f.typed().len(), 1, "no write into a failed native turn");
                assert_eq!(f.request(next).phase, RequestPhase::Queued);
                assert_eq!(f.request(next).group, None);
                f.status(RuntimeStatus::Idle);
                f.deliver();
                assert_eq!(
                    f.typed().len(),
                    1,
                    "Idle alone does not bypass the retry deadline"
                );
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
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_none());
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
fn kimi_41980_uncertain_steering_is_never_replayed_by_a_provider_retry() {
    for restart in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let member = uncertain_member(&mut f);
        let waiting = f.send("next queued work", false);
        std::fs::write(wire, failure_wire(&mut f, lead, 0)).unwrap();
        f.consume();
        assert_quota_hold(&f, lead);
        if restart {
            super::kimi_steering_tests::reopen(&mut f);
        }
        f.status(RuntimeStatus::Idle);
        f.worker.transport = Box::new(Native(f.native.clone()));
        f.now = f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms
            + crate::messaging::model::QUEUED_STALL_MS * 2;
        f.expire_after_reconciliation(f.now);
        f.deliver();
        assert_eq!(f.request(member).phase, RequestPhase::Submitting);
        assert!(!f.request(member).trusted_start_bound);
        assert!(f.request(member).uncertain_outcome);
        assert!(f.request(member).submission_error.is_some());
        assert!(f.reply(member).is_none());
        assert_eq!(f.request(waiting).phase, RequestPhase::Queued);
        assert_eq!(
            f.typed().len(),
            2,
            "a deadline never proves an unconfirmed input ran"
        );
        let status = f
            .worker
            .control_message(f.request(member).prompt.id)
            .unwrap();
        assert_eq!(status["requests"][0]["stage"], "submitting");
        assert_eq!(status["complete"], false);
    }
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
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_none());
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
    }
}

#[test]
fn kimi_committed_failure_settles_even_with_a_stale_dialog_observation() {
    for stale_dialog in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let next = f.send("next independent work", true);
        std::fs::write(wire, failure_wire(&mut f, lead, 0)).unwrap();
        f.status(RuntimeStatus::Blocked);
        let mut state = f.worker.state.clone();
        state.observe_dialog(f.agent, stale_dialog, f.now).unwrap();
        f.worker.save(state).unwrap();
        f.consume();
        assert_quota_hold(&f, lead);
        assert_eq!(f.request(next).phase, RequestPhase::Queued);
        assert_eq!(f.typed().len(), 1);
    }
}

#[test]
fn kimi_failed_lead_is_not_rebound_when_queued_input_starts_the_next_turn() {
    for restart in [false, true] {
        for stop in [false, true] {
            let mut f = Fixture::new(Provider::Kimi);
            let wire = super::kimi_steering_tests::home(&f);
            let lead = running(&mut f, "original task", "0");
            let next = uncertain_member(&mut f);
            let next_payload = f.worker.state.render_request_payload(next).unwrap();
            let records = failure_wire(&mut f, lead, 0)
                + &super::kimi_steering_tests::native_turn(
                    1,
                    &next_payload,
                    &[],
                    "independent answer",
                );
            std::fs::write(wire, records).unwrap();
            if stop {
                f.record(json!({"hook_event_name":"Stop","session_id":"session"}));
            }
            f.started("1", incident()["followup"].as_str().unwrap());
            if restart {
                super::kimi_steering_tests::reopen(&mut f);
            }
            f.consume();
            assert_quota_hold(&f, lead);
            assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("0"));
            assert!(f.reply(lead).is_none());
            assert_eq!(f.request(next).provider_turn_id.as_deref(), Some("1"));
            assert_eq!(f.reply(next).as_deref(), Some("independent answer"));
            assert_eq!(f.typed().len(), 2, "neither input was replayed");
        }
    }
}

#[test]
fn kimi_dialog_without_an_owned_commit_retains_the_turn() {
    for case in [1, 2] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let next = f.send("queued work", true);
        std::fs::write(wire, failure_wire(&mut f, lead, case)).unwrap();
        f.status(RuntimeStatus::Blocked);
        let mut state = f.worker.state.clone();
        state.observe_dialog(f.agent, true, f.now).unwrap();
        f.worker.save(state).unwrap();
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_none());
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        assert_eq!(f.request(next).phase, RequestPhase::Queued);
        assert_eq!(f.typed().len(), 1);
    }
}

#[test]
fn kimi_failure_does_not_promote_a_member_from_a_foreign_started_callback() {
    for foreign_session in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let next = uncertain_member(&mut f);
        std::fs::write(wire, failure_wire(&mut f, lead, 0)).unwrap();
        let incident = incident();
        f.record(json!({"hook_event_name":"TurnStarted","origin_kind":"user","turn_id":1,
            "session_id": if foreign_session { "foreign-session" } else { "session" },
            "prompt": if foreign_session { incident["followup"].as_str().unwrap() } else { "foreign input" },
        }));
        f.consume();
        assert_quota_hold(&f, lead);
        assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("0"));
        assert_eq!(f.request(next).phase, RequestPhase::Submitting);
        assert!(!f.request(next).trusted_start_bound);
        assert!(f.request(next).submission_error.is_some());
        assert!(f.reply(next).is_none());
        assert_ne!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(next)
        );
        assert_eq!(f.typed().len(), 2);
    }
}

#[test]
fn kimi_followup_after_failed_lead_rejects_callbacks_before_submission() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = super::kimi_steering_tests::home(&f);
    let lead = running(&mut f, "original task", "0");
    let next = uncertain_member(&mut f);
    std::fs::write(wire, failure_wire(&mut f, lead, 0)).unwrap();
    f.started("1", incident()["followup"].as_str().unwrap());
    f.consume();
    assert_quota_hold(&f, lead);
    assert_eq!(f.request(next).phase, RequestPhase::Active);
    let callback = ProviderCallback {
        callback_id: "stale-start".into(),
        sequence: f.request(lead).submission_boundary.unwrap(),
        occurred_at_ms: f.now,
        agent_id: f.agent,
        launch_id: "launch".into(),
        provider_session_id: Some("session".into()),
        provider_turn_id: Some("1".into()),
        provider_prompt_id: None,
        prompt_payload: f.worker.state.render_request_payload(next),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(
        f.worker.state.accept_callback(callback),
        CallbackDisposition::Rejected(CallbackRejection::BeforeSubmissionBoundary)
    );
}

#[test]
fn provider_flake_legacy_kimi_inputs_keep_their_turns_before_and_after_hold() {
    // The old retry callback path must not strand a second native input on the held lead.
    for (restart, settle_first) in [(false, true), (false, false), (true, true), (true, false)] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "original task", "0");
        let first = f.send("first independent followup", false);
        f.deliver();
        let second = f.send("second independent followup", false);
        f.deliver();

        let mut snapshot = serde_json::to_value(&f.worker.state).unwrap();
        for id in [first, second] {
            let member = &mut snapshot["requests"][id.0.to_string()];
            member["phase"] = json!("active");
            member["uncertain_outcome"] = json!(true);
            member["submission_error"] = json!("legacy unconfirmed write");
        }
        f.worker
            .save(serde_json::from_value(snapshot).unwrap())
            .unwrap();
        let failed_wire = failure_wire(&mut f, lead, 0);
        std::fs::write(&wire, &failed_wire).unwrap();
        if settle_first {
            f.consume();
        }

        f.started("1", "first independent followup");
        f.started("2", "second independent followup");
        f.consume();

        assert_eq!(f.request(lead).phase, RequestPhase::Active);
        assert_eq!(
            f.worker
                .state
                .agent(f.agent)
                .unwrap()
                .provider_retry
                .as_ref()
                .unwrap()
                .lead,
            lead,
        );
        assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("0"));
        assert!(f.request(lead).turn_ended_at_ms.is_some());
        assert_eq!(
            [
                (
                    f.request(first).phase,
                    f.request(first).provider_turn_id.as_deref(),
                ),
                (
                    f.request(second).phase,
                    f.request(second).provider_turn_id.as_deref(),
                ),
            ],
            [
                (RequestPhase::Active, Some("1")),
                (RequestPhase::Active, Some("2")),
            ]
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(second)
        );
        for id in [first, second] {
            assert!(
                f.request(id).submission_error.is_none(),
                "a native Started receipt must clear the old submission failure"
            );
        }
        assert_eq!(f.typed().len(), 3, "neither legacy input was replayed");
        if restart {
            super::kimi_steering_tests::reopen(&mut f);
            f.status(RuntimeStatus::Working);
        }
        let first_payload = f.worker.state.render_request_payload(first).unwrap();
        let second_payload = f.worker.state.render_request_payload(second).unwrap();
        std::fs::write(
            &wire,
            failed_wire
                + &super::kimi_steering_tests::native_turn(
                    1,
                    &first_payload,
                    &[],
                    "first independent answer",
                )
                + &super::kimi_steering_tests::native_turn(
                    2,
                    &second_payload,
                    &[],
                    "second independent answer",
                ),
        )
        .unwrap();
        f.consume();
        assert_eq!(f.reply(first).as_deref(), Some("first independent answer"));
        assert_eq!(
            f.reply(second).as_deref(),
            Some("second independent answer")
        );
        assert_eq!(f.request(first).provider_turn_id.as_deref(), Some("1"));
        assert_eq!(f.request(second).provider_turn_id.as_deref(), Some("2"));
        f.status(RuntimeStatus::Idle);
        let due = f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap()
            .retry_at_ms;
        assert!(f.worker.state.retry_provider_requests(due));
        assert_eq!(f.worker.state.queued_requests(f.agent), &[lead]);
        assert_eq!(f.reply(first).as_deref(), Some("first independent answer"));
        assert_eq!(
            f.reply(second).as_deref(),
            Some("second independent answer")
        );
        assert_eq!(
            f.typed().len(),
            3,
            "native receipts and replies never cause replay"
        );
    }
}

struct PolledKimi(Arc<Mutex<Vec<Method>>>);

impl Transport for PolledKimi {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(
            method,
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_)
        ) {
            return Ok(ResponseResult::AgentList {
                agents: vec![serde_json::from_value(json!({
                "terminal_id":"terminal", "pane_id":"pane", "agent":"kimi",
                "agent_status":"working", "interactive_ready":true,
                "workspace_id":"workspace", "tab_id":"tab", "focused":false, "revision":1,
                "agent_session":{"source":"bus", "agent":"kimi", "kind":"id", "value":"session"},
            })).unwrap()],
            });
        }
        Native(self.0.clone()).request(method)
    }
}

#[test]
fn kimi_pre_started_error_logs_recovery_once_after_durable_save() {
    for batched in [false, true] {
        let mut f = Fixture::new(Provider::Kimi);
        let wire = super::kimi_steering_tests::home(&f);
        let lead = running(&mut f, "PRIVATE_ORIGINAL", "0");
        let next = uncertain_member(&mut f);
        if batched {
            f.worker.transport = Box::new(PolledKimi(f.native.clone()));
            f.worker.tick().unwrap();
        }
        std::fs::write(wire, failure_wire(&mut f, lead, 3)).unwrap();
        f.started("1", incident()["followup"].as_str().unwrap());
        let spool = f.dir.join("callbacks/launch");
        f.worker
            .store
            .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            let failed = if batched {
                f.worker.tick()
            } else {
                f.worker.consume_callbacks(f.agent, &spool)
            };
            assert!(failed.is_err());
            assert!(!capture.text().contains("bus.message.recovered"));
            assert_eq!(f.request(lead).phase, RequestPhase::Active);
            let (events, _events) = std::sync::mpsc::channel();
            f.worker
                .retry_storage(std::time::Instant::now() + Duration::from_secs(2), &events);
            if batched {
                f.worker.tick().unwrap();
            } else {
                f.worker.consume_callbacks(f.agent, &spool).unwrap();
            }
            assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
            assert_eq!(f.request(next).provider_turn_id.as_deref(), Some("1"));
            let saved = f.worker.store.load().unwrap().unwrap();
            assert_eq!(saved.request(lead).unwrap().phase, RequestPhase::Abandoned);
            f.consume();
            f.consume();
        });
        let log = capture.text();
        let rows: Vec<_> = log
            .lines()
            .filter(|line| {
                line.contains("bus.message.recovered")
                    && line.contains(&format!("request_id={} ", lead.0))
            })
            .collect();
        assert_eq!(rows.len(), 1, "batched={batched}: {log}");
        assert!(rows[0].contains("reason=\"provider_error\""));
        assert!(rows[0].contains(&format!("lead_id={}", lead.0)));
        assert!(!log.contains("PRIVATE"), "{log}");
    }
}
