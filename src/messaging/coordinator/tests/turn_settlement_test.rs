use super::*;

#[path = "idle_backstop_test.rs"]
mod idle_backstop_tests;

// The callback spool gets a bounded grace period after the first settled poll.
const CALLBACK_GRACE_MS: u64 = crate::messaging::model::TURN_SETTLE_MS;

fn reload(fixture: &mut Fixture) {
    let placeholder = Worker::open(
        fixture.dir.join("restart-placeholder"),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut fixture.worker, placeholder));
    fixture.worker = Worker::open(
        fixture.dir.clone(),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
}

fn restart(fixture: &mut Fixture) {
    reload(fixture);
    fixture.worker.serving_generation = 2;
}

fn server_restart(fixture: &mut Fixture) {
    let record = crate::utils::restart_record::RestartLog {
        at_ms: fixture.now,
        who: "user:restart-test".into(),
        old_sha: "old".into(),
        new_sha: "new".into(),
        old_binary: "/old".into(),
        new_binary: "/new".into(),
        outcome: "restarted".into(),
        reason: String::new(),
    };
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::outcome_path(&fixture.dir),
        &record,
    )
    .unwrap();
    fixture.worker.announce_recorded_restart().unwrap();
    fixture.worker.serving_generation = 2;
}

#[cfg(unix)]
fn end_restarted_turn(
    fixture: &mut Fixture,
    turn: &str,
    text: &str,
    kimi_wire: Option<&std::path::Path>,
) {
    fixture.status(RuntimeStatus::Idle);
    match fixture.provider {
        Provider::Kimi => {
            let lead = fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .current_request
                .unwrap();
            let prompt = fixture.worker.state.render_request_payload(lead).unwrap();
            append(
                kimi_wire.expect("isolated Kimi wire"),
                &super::kimi_steering_tests::native_turn(turn.parse().unwrap(), &prompt, &[], text),
            );
            fixture.record(json!({"hook_event_name":"Stop","session_id":"session"}));
        }
        Provider::Grok => {
            let directory = fixture.dir.join("grok-native/session");
            std::fs::create_dir_all(&directory).unwrap();
            let transcript = directory.join("updates.jsonl");
            let rows = [
                json!({"method":"session/update","params":{"sessionId":"session","_meta":{"promptId":turn},"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}}),
                json!({"method":"session/update","params":{"sessionId":"session","update":{"sessionUpdate":"turn_completed","prompt_id":turn,"stop_reason":"end_turn"}}}),
            ];
            std::fs::write(
                &transcript,
                rows.iter()
                    .map(|row| format!("{row}\n"))
                    .collect::<String>(),
            )
            .unwrap();
            fixture.record(json!({"hookEventName":"stop","sessionId":"session","promptId":turn,"reason":"end_turn","transcriptPath":transcript}));
        }
        _ => fixture.stopped(turn, text),
    }
    fixture.consume();
}

struct RestartLifecycleNative {
    methods: Arc<Mutex<Vec<Method>>>,
    info: schema::AgentInfo,
}

impl Transport for RestartLifecycleNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(&method, Method::AgentListWithDetectionContext(_)) {
            self.methods.lock().unwrap().push(method);
            return Ok(ResponseResult::AgentList {
                agents: vec![self.info.clone()],
            });
        }
        Native(self.methods.clone()).request(method)
    }
}

fn restart_lifecycle_transport(fixture: &Fixture, rebind: bool) -> Box<dyn Transport> {
    let kind = crate::messaging::identity::provider_kind(fixture.provider).label();
    let info = serde_json::from_value(json!({
        "generation": if rebind { 2 } else { fixture.worker.serving_generation },
        "terminal_id": if rebind { "restored-terminal" } else { "terminal" },
        "pane_id": if rebind { "restored-pane" } else { "pane" },
        "name": crate::messaging::identity::managed_name(fixture.room, fixture.agent),
        "agent": kind, "agent_status": "idle", "interactive_ready": true,
        "workspace_id": "workspace", "tab_id": "tab", "focused": false, "revision": 1,
        "agent_session": {"source": crate::messaging::identity::managed_source(fixture.provider),
            "agent": kind, "kind": "id", "value": "session"}
    }))
    .unwrap();
    Box::new(RestartLifecycleNative {
        methods: fixture.native.clone(),
        info,
    })
}

fn restart_lifecycle_reopen(fixture: &mut Fixture, rebind: bool) {
    let placeholder = Worker::open(
        fixture.dir.join("lifecycle-placeholder"),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut fixture.worker, placeholder));
    fixture.worker = Worker::open(
        fixture.dir.clone(),
        restart_lifecycle_transport(fixture, rebind),
    )
    .unwrap();
}

fn restart_lifecycle_receipt(fixture: &Fixture, outcome: &str) {
    let record = crate::utils::restart_record::RestartLog {
        at_ms: fixture.now,
        who: "user:lifecycle".into(),
        old_sha: "old".into(),
        new_sha: "new".into(),
        old_binary: "/old".into(),
        new_binary: "/new".into(),
        outcome: outcome.into(),
        reason: String::new(),
    };
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::outcome_path(&fixture.dir),
        &record,
    )
    .unwrap();
}

#[test]
fn restart_lifecycle_late_outcome_keeps_the_request_delivered_after_native_rebind() {
    let mut fixture = Fixture::new(Provider::Codex);
    let interrupted = fixture.send("in flight before whole-session restart", false);
    fixture.deliver();
    let queued = fixture.send(
        "queued before restart, delivered by the resumed coordinator",
        true,
    );
    restart_lifecycle_reopen(&mut fixture, true);
    fixture.worker.tick().unwrap();
    assert_eq!(fixture.request(interrupted).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        Some(queued)
    );
    assert_eq!(fixture.request(queued).phase, RequestPhase::Active);
    restart_lifecycle_receipt(&fixture, "restarted");
    fixture.worker.tick().unwrap();
    assert_eq!(
        fixture.request(queued).phase,
        RequestPhase::Active,
        "the health receipt describes the already-reconciled interruption, not work delivered by the replacement coordinator; reason={:?}",
        fixture.request(queued).failure_reason
    );
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        Some(queued)
    );
}

#[test]
fn restart_lifecycle_late_outcome_keeps_queued_work_without_a_loaded_current_request() {
    let mut fixture = Fixture::new(Provider::Codex);
    let queued = fixture.send(
        "queued work delivered after resume with no old current request",
        true,
    );
    restart_lifecycle_reopen(&mut fixture, true);
    fixture.worker.tick().unwrap();
    assert_eq!(fixture.request(queued).phase, RequestPhase::Active);
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        Some(queued)
    );
    restart_lifecycle_receipt(&fixture, "restarted");
    fixture.worker.tick().unwrap();
    assert_eq!(
        fixture.request(queued).phase,
        RequestPhase::Active,
        "an empty reopen request set contains no interrupted work; a late receipt must not capture newly delivered work; reason={:?}",
        fixture.request(queued).failure_reason
    );
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        Some(queued)
    );
}

#[cfg(unix)]
#[test]
fn restart_request_continues_bound_and_closes_unbound_requests_before_queued_work() {
    let mut failures = Vec::new();
    for provider in [
        Provider::ClaudeCode,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        for server_only in [false, true] {
            for start_bound in [false, true] {
                let mut fixture = Fixture::new(provider);
                let wire = (provider == Provider::Kimi)
                    .then(|| super::kimi_steering_tests::home(&fixture));
                let interrupted = if start_bound {
                    running(&mut fixture, "interrupted delivered request", "1")
                } else {
                    let request = fixture.send("interrupted awaiting-start request", false);
                    fixture.deliver();
                    request
                };
                let queued = fixture.send("queued behind the restart", true);
                if server_only {
                    server_restart(&mut fixture);
                } else {
                    restart(&mut fixture);
                }
                fixture.status(RuntimeStatus::Idle);
                fixture.worker.settle_requests().unwrap();
                let request = fixture.request(interrupted);
                if start_bound {
                    assert!(!request.settled(), "{provider:?}: {request:?}");
                    assert_eq!(
                        fixture
                            .worker
                            .state
                            .agent(fixture.agent)
                            .unwrap()
                            .current_request,
                        Some(interrupted)
                    );
                    assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
                    fixture.deliver();
                    let notice = fixture.typed().last().unwrap().0.clone();
                    assert!(notice.contains("Bus restarted"), "{notice}");
                    fixture.started("2", &notice);
                    fixture.consume();
                    end_restarted_turn(
                        &mut fixture,
                        "2",
                        "continued task completed",
                        wire.as_deref(),
                    );
                    assert_completed_without_reply(&fixture, interrupted);
                    fixture.status(RuntimeStatus::Idle);
                } else if request.phase != RequestPhase::Abandoned
                    || request.failure_reason.as_deref() != Some("reply_not_captured")
                    || fixture
                        .worker
                        .state
                        .agent(fixture.agent)
                        .unwrap()
                        .current_request
                        .is_some()
                {
                    failures.push(format!(
                        "{provider:?}/server_only={server_only}/start_bound={start_bound}: old request remained {:?}/{:?}",
                        request.phase, request.failure_reason
                    ));
                }
                fixture.deliver();
                if fixture.request(queued).phase == RequestPhase::Queued {
                    failures.push(format!(
                        "{provider:?}/server_only={server_only}/start_bound={start_bound}: queued message stayed blocked"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn restart_request_plain_coordinator_reopen_keeps_a_live_turn() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(
        &mut fixture,
        "continue after a plain coordinator reopen",
        "turn-1",
    );
    reload(&mut fixture);
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.stopped("turn-1", "completed after the coordinator reopened");
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "completed after the coordinator reopened");
}

#[cfg(unix)]
#[test]
fn restart_request_continues_mid_steer_group_without_losing_an_already_spooled_final() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "lead before restart", "turn-1");
        let steering = fixture.send("steering before restart", false);
        fixture.deliver();
        let queued = fixture.send("after the interrupted group", true);
        restart(&mut fixture);
        fixture.status(RuntimeStatus::Idle);
        fixture.worker.settle_requests().unwrap();
        for request in [lead, steering] {
            assert!(!fixture.request(request).settled());
            assert!(fixture.request(request).failure_reason.is_none());
        }
        assert_eq!(fixture.request(steering).group, Some(lead));
        assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
        fixture.deliver();
        let notice = fixture.typed().last().unwrap().0.clone();
        fixture.started("turn-2", &notice);
        fixture.consume();
        fixture.stopped("turn-2", "resumed group completed");
        fixture.consume();
        assert_completed_without_reply(&fixture, lead);
        assert_completed_without_reply(&fixture, steering);
        fixture.status(RuntimeStatus::Idle);
        fixture.deliver();
        assert_ne!(fixture.request(queued).phase, RequestPhase::Queued);

        let mut completed = Fixture::new(provider);
        let answered = running(&mut completed, "finish just before restart", "turn-1");
        completed.stopped("turn-1", "finished before the stop");
        completed.consume();
        let next = completed.send("next after the completed turn", true);
        restart(&mut completed);
        completed.status(RuntimeStatus::Idle);
        completed.worker.settle_requests().unwrap();
        assert_completed_without_reply(&completed, answered);
        assert_reply_text_absent(&completed, "finished before the stop");
        completed.deliver();
        assert_ne!(completed.request(next).phase, RequestPhase::Queued);
    }
}

#[test]
fn restart_request_recovers_a_completed_codex_turn_from_its_rollout_before_releasing_the_queue() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-completion.jsonl");
    let lead = running(&mut fixture, "finish without a Stop hook", "turn-1");
    let typed = fixture.typed().last().unwrap().0.clone();
    append(
        &rollout,
        &codex_turn(
            &rollout_at(&fixture, 1),
            "turn-1",
            &typed,
            Some("recovered after restart"),
        ),
    );
    let queued = fixture.send("queued after recovered final", true);
    restart(&mut fixture);
    fixture.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"resume","transcript_path":rollout}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "recovered after restart");
    fixture.deliver();
    assert_ne!(fixture.request(queued).phase, RequestPhase::Queued);
}

#[test]
fn restart_request_recovers_codex_final_from_the_rollout_learned_before_stop() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-before-stop.jsonl");
    let lead = fixture.send("finish before Bus stops without a Stop hook", false);
    fixture.deliver();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"finish before Bus stops without a Stop hook",
        "transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let typed = fixture.typed().last().unwrap().0.clone();
    append(
        &rollout,
        &codex_turn(
            &rollout_at(&fixture, 1),
            "turn-1",
            &typed,
            Some("committed before the stop"),
        ),
    );
    let queued = fixture.send("next after the committed answer", true);
    restart(&mut fixture);
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "committed before the stop");
    fixture.deliver();
    assert_ne!(fixture.request(queued).phase, RequestPhase::Queued);
}

#[cfg(unix)]
#[test]
fn restart_request_recovers_a_completed_kimi_turn_from_its_wire_before_releasing_the_queue() {
    let mut fixture = Fixture::new(Provider::Kimi);
    let wire = super::kimi_steering_tests::home(&fixture);
    let lead = running(&mut fixture, "finish on the committed Kimi wire", "1");
    let prompt = fixture.worker.state.render_request_payload(lead).unwrap();
    let rows = [
        json!({"type":"turn.prompt","agentId":"main","turnId":1,"promptId":"p1","input":[{"type":"text","text":prompt}]}),
        json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"text","text":"Kimi final recovered after restart"}]}}}),
        json!({"type":"agent.turn.ended","turnId":1,"outcome":"done"}),
        json!({"type":"turn.ended","agentId":"main","turnId":1,"reason":"completed"}),
        json!({"type":"prompt.completed","agentId":"main","promptId":"p1","reason":"completed"}),
    ];
    std::fs::write(
        wire,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let queued = fixture.send("queued after the Kimi final", true);
    restart(&mut fixture);
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "Kimi final recovered after restart");
    fixture.deliver();
    assert_ne!(fixture.request(queued).phase, RequestPhase::Queued);
}

#[test]
fn reply_capture_diagnostics_identify_every_abandoned_group_member() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut fixture = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut fixture, "PRIVATE_PROMPT", "turn-1");
        let joined = fixture.send("PRIVATE_STEERING", false);
        fixture.deliver();
        fixture.started("turn-1", "PRIVATE_STEERING");
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        let log = capture.text();
        for id in [lead, joined] {
            assert_completed_without_reply(&fixture, id);
        }
        assert!(fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request
            .is_none());
        assert!(
            !log.contains("reply_not_captured") && !log.contains("bus.message.no_reply"),
            "{log}"
        );
        assert!(!log.contains("PRIVATE"), "{log}");
    });
}

#[test]
fn reply_capture_diagnostics_record_a_settlement_once_after_a_failed_save_is_retried() {
    for reason in ["reply_not_captured", "agent_unavailable", "provider_error"] {
        let mut fixture = Fixture::new(Provider::Codex);
        let lead = running(&mut fixture, "PRIVATE_PROMPT", "turn-1");
        let follower = fixture.send("PRIVATE_STEERING", false);
        fixture.deliver();
        fixture.started("turn-1", "PRIVATE_STEERING");
        fixture.consume();
        if reason == "provider_error" {
            let rollout = fixture.dir.join("rollout-error.jsonl");
            std::fs::write(
                &rollout,
                json!({"type":"event_msg","payload":{
                    "type":"task_complete","turn_id":"turn-1","error":{"message":"PRIVATE_ERROR"}
                }})
                .to_string(),
            )
            .unwrap();
            fixture.record(
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
                "turn_id":"turn-1","prompt":"PRIVATE_STEERING","transcript_path":rollout}),
            );
            fixture.consume();
        }
        fixture.status(RuntimeStatus::Idle);
        let status = if reason == "agent_unavailable" {
            RuntimeStatus::Unavailable
        } else {
            RuntimeStatus::Idle
        };
        fixture.status(status);
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(status);
        fixture
            .worker
            .store
            .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            assert!(fixture.worker.settle_requests().is_err());
            assert!(!capture.text().contains("bus.message.recovered"));
            assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
            fixture.worker.settle_requests().unwrap();
            fixture.worker.settle_requests().unwrap();
        });
        let log = capture.text();
        if reason == "reply_not_captured" {
            // Idle grace with no recorded end completes the claimed turn.
            // It is not an abandoned capture, and a failed save must not log one.
            for id in [lead, follower] {
                assert_completed_without_reply(&fixture, id);
            }
            assert!(!log.contains("reason=\"reply_not_captured\""), "{log}");
        } else {
            for id in [lead, follower] {
                let rows: Vec<_> = log
                    .lines()
                    .filter(|line| {
                        line.contains("bus.message.recovered")
                            && line.contains(&format!("request_id={} ", id.0))
                    })
                    .collect();
                assert_eq!(rows.len(), 1, "{reason}: {log}");
                assert!(
                    rows[0].contains(&format!("reason=\"{reason}\"")),
                    "{reason}: {}",
                    rows[0]
                );
                assert_eq!(fixture.request(id).phase, RequestPhase::Abandoned);
            }
        }
        assert!(
            !log.contains("bus.message.no_reply") && !log.contains("PRIVATE"),
            "{log}"
        );
    }
}

#[test]
fn failed_provider_turn_closes_its_group_and_releases_the_queue() {
    for provider in [Provider::Cursor, Provider::ClaudeCode] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "review this change", "turn-1");
        let joined = fixture.send("check the tests too", false);
        fixture.deliver();
        fixture.started("turn-1", "check the tests too");
        fixture.consume();
        let next = fixture.send("review the next change", true);
        let failed = match provider {
            Provider::Cursor => {
                json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-1","status":"error"})
            }
            _ => {
                json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1"})
            }
        };
        fixture.record(failed);
        fixture.consume();
        for id in [lead, joined] {
            assert_eq!(
                fixture.request(id).phase,
                RequestPhase::Abandoned,
                "{provider:?}"
            );
            assert!(fixture.request(id).completed_at_ms.is_some());
            assert!(fixture.reply(id).is_none());
        }
        assert!(fixture.error().is_some());
        assert_eq!(
            fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .current_request,
            None
        );
        fixture.status(RuntimeStatus::Idle);
        fixture.deliver();
        assert_eq!(fixture.request(next).group, None);
        assert_eq!(
            fixture.typed_content().last().unwrap(),
            &("review the next change".into(), false)
        );
    }
}

#[test]
fn a_turn_end_without_visible_reply_text_unblocks_the_next_request() {
    for provider in [Provider::Cursor, Provider::ClaudeCode] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "review this change", "turn-1");
        let next = fixture.send("review the next change", true);
        if provider == Provider::Cursor {
            let transcript = fixture.dir.join("incomplete.jsonl");
            std::fs::write(&transcript, concat!(
                "{\"role\":\"user\"}\n",
                "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Checking.\"},{\"type\":\"tool_use\"}]}}\n"
            )).unwrap();
            fixture.record(json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn-1","text":"Checking.Ready","transcript_path":transcript}));
            fixture.record(json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-1","status":"completed"}));
        } else {
            // A completed Claude hook with no assistant text is an empty final.
            fixture.record(
                json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1"}),
            );
        }
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(
            fixture.request(lead).phase,
            if provider == Provider::ClaudeCode {
                RequestPhase::Completed
            } else {
                RequestPhase::Active
            }
        );
        fixture.now += CALLBACK_GRACE_MS;
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        assert_completed_without_reply(&fixture, lead);
        assert!(fixture
            .worker
            .state
            .room(fixture.room)
            .unwrap()
            .latest_replies
            .is_empty());
        fixture.deliver();
        assert_eq!(fixture.request(next).group, None);
        assert_eq!(
            fixture.typed_content().last().unwrap(),
            &("review the next change".into(), false)
        );
        let persisted = fixture.worker.store.load().unwrap().unwrap();
        assert_eq!(
            persisted.request(lead).unwrap().phase,
            RequestPhase::Completed
        );
        assert!(persisted.request(lead).unwrap().reply().is_none());
    }
}

#[test]
fn completed_codex_turn_closes_existing_joins_and_cannot_collect_new_joins() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    let mut joined = Vec::new();
    for index in 0..8 {
        let text = format!("correction {index}");
        joined.push(fixture.send(&text, false));
        fixture.deliver();
        fixture.started("turn-1", &text);
        fixture.consume();
    }
    fixture.status(RuntimeStatus::Idle);
    // A trusted final distinguishes unrelated later activity from a stale
    // Idle observation in the same still-running turn.
    fixture.stopped("turn-1", "implemented");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let next = fixture.send("an independent task", false);
    assert_eq!(
        fixture.worker.state.next_steering(fixture.agent),
        None,
        "a finished request must not take another join"
    );
    assert_eq!(fixture.request(next).phase, RequestPhase::Queued);
    assert_eq!(fixture.request(next).group, None);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for id in std::iter::once(lead).chain(joined) {
        assert_completed_without_reply(&fixture, id);
        assert!(fixture.request(id).completed_at_ms.is_some());
    }
    fixture.deliver();
    assert_eq!(
        fixture.typed_content().last().unwrap(),
        &("an independent task".into(), false)
    );
}

#[test]
fn a_final_arriving_during_the_idle_grace_period_still_completes() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS - 2;
    fixture.status(RuntimeStatus::Idle);
    fixture.stopped("turn-1", "implemented");
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "implemented");
}

#[test]
fn a_visible_choice_panel_does_not_close_an_idle_codex_request() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    let mut state = fixture.worker.state.clone();
    state
        .observe_dialog(
            fixture.agent,
            true,
            state.agent(fixture.agent).unwrap().observed_at_ms,
        )
        .unwrap();
    fixture.worker.save(state).unwrap();
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
}

#[test]
fn explicit_background_work_can_resume_after_the_callback_grace_period() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1","background_tasks":[{"type":"agent"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS * 2;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.started(
        "turn-2",
        "<task-notification>background agent finished</task-notification>",
    );
    fixture.stopped("turn-2", "implemented");
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_eq!(
        fixture.request(lead).provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_reply_text_absent(&fixture, "implemented");
}

#[test]
fn a_final_spooled_at_the_deadline_is_read_before_the_request_closes() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let lead = running(&mut fixture, "review the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.stopped("turn-1", "Ready");
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "Ready");
}

#[test]
fn a_failure_from_another_turn_does_not_release_the_current_owner() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let lead = running(&mut fixture, "review the change", "turn-1");
    fixture.record(json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"old-turn","status":"error"}));
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .current_request,
        Some(lead)
    );
    assert_eq!(fixture.error(), None);
}

#[test]
fn a_turn_with_status_evidence_but_no_start_or_reply_hook_also_closes() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = fixture.send("implement the change", false);
    fixture.deliver();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert!(!fixture.request(lead).uncertain_outcome);
    assert!(!fixture.request(lead).trusted_start_bound);
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    // The deadline is durable, including when a coordinator reloads its state.
    fixture.worker.state = fixture.worker.store.load().unwrap().unwrap();
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert!(fixture
        .worker
        .state
        .room(fixture.room)
        .unwrap()
        .latest_replies
        .is_empty());
}

#[test]
fn a_codex_turn_that_ended_on_a_provider_error_reports_that_error() {
    // Live: Codex fires no Stop hook when "Selected model is at capacity"
    // ends a turn; only its rollout's task_complete event carries the error.
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-12T00-00-00-abc.jsonl");
    std::fs::write(
        &rollout,
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1","last_agent_message":null,
            "error":{"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"}}})
        .to_string() + "\n",
    )
    .unwrap();
    let lead = fixture.send("implement the change", false);
    fixture.deliver();
    fixture.record(json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn-1","prompt":"implement the change","transcript_path":rollout}));
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let request = fixture.request(lead);
    assert!(!request.settled());
    assert!(request.failure_reason.is_none());
    assert_eq!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .shown_status(),
        RuntimeStatus::Working
    );
    assert!(fixture.error().is_none());
}

/// Live (2026-10-13, Claude Code 2.1.296): "API Error: Connection lost
/// mid-response" ended a worker's turn with a StopFailure hook, and Bus closed
/// the request with no reason. The payload names the error, as measured.
#[test]
fn a_claude_turn_ended_by_an_api_error_reports_that_error() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "run the e2e", "turn-1");
    let joined = fixture.send("check the tests too", false);
    fixture.deliver();
    fixture.started("turn-1", "check the tests too");
    fixture.consume();
    fixture.record(json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1",
        "error":"server_error",
        "last_assistant_message":"API Error: Connection lost mid-response. The response above may be incomplete."}));
    fixture.consume();
    for id in [lead, joined] {
        assert!(!fixture.request(id).settled());
        assert!(fixture.request(id).failure_reason.is_none());
    }
    assert!(fixture.error().is_none());
    let message = fixture.request(joined).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["complete"], false);
    assert_eq!(status["requests"][0]["reason"], Value::Null);
    assert_eq!(status["requests"][0]["status"], "working");
}

#[test]
fn a_turn_closed_without_a_reply_or_provider_error_says_so_in_its_reason() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
}

pub(super) fn claude_stop(turn: &str, text: &str, shells: &[&str]) -> Value {
    // Shape measured from Claude Code 2.1.296: each running background shell
    // is listed, and each one that finishes wakes a turn whose Stop drops it.
    let tasks: Vec<_> = shells
        .iter()
        .map(|id| json!({"id":id,"type":"shell","status":"running","description":"Run","command":"sleep 45"}))
        .collect();
    json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,"last_assistant_message":text,"background_tasks":tasks,"session_crons":[]})
}

pub(super) fn claude_startup() -> Value {
    json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"})
}

/// A shell left running before the request (say a hung `find /`) is listed by
/// every later Stop and must not hold this or any later reply.
#[test]
fn a_claude_shell_left_running_before_the_request_does_not_hold_its_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let first = running(&mut fixture, "look around", "turn-1");
    fixture.record(claude_stop("turn-1", "Looked around.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(first).phase, RequestPhase::Completed);
    // A turn of the agent's own leaves a shell running.
    fixture.record(claude_stop("own-turn", "Started a search.", &["stale"]));
    fixture.consume();
    let lead = running(&mut fixture, "review this change", "turn-2");
    fixture.record(claude_stop("turn-2", "Reviewed.", &["stale"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "Reviewed.");
}

/// After a Bus restart the shells running before the request are unknown, so
/// none of them holds a reply.
#[test]
fn a_claude_reply_is_not_held_by_shells_of_unknown_age() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "review this change", "turn-1");
    fixture.record(claude_stop("turn-1", "Reviewed.", &["unknown"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
}

#[test]
fn a_joined_codex_request_reports_the_provider_error_that_ended_its_turn() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    let joined = fixture.send("check the tests too", false);
    fixture.deliver();
    let rollout = fixture.dir.join("rollout-joined.jsonl");
    std::fs::write(&rollout, json!({"type":"event_msg","payload":{
        "type":"task_complete","turn_id":"turn-1","error":{"message":"Selected model is at capacity."}
    }}).to_string()).unwrap();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"check the tests too","transcript_path":rollout}),
    );
    fixture.consume();
    assert_eq!(fixture.request(joined).group, Some(lead));
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(joined).phase, RequestPhase::Abandoned);
    let message = fixture.request(joined).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(
        status["requests"][0]["reason"],
        "provider_error: Selected model is at capacity."
    );
}

/// A follower polling message status between a reply-less turn end and its
/// settlement must not take the turn as done: Bus attaches the provider error
/// (or `reply_not_captured`) only after the settle window. Live e2e: `send
/// --async` exited 0 at Idle, 5 s before the provider error was attached.
#[test]
fn a_reply_less_ended_turn_reports_settling_until_its_reason_is_attached() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    let joined = fixture.send("check the tests too", false);
    fixture.deliver();
    let rollout = fixture.dir.join("rollout-settling.jsonl");
    std::fs::write(&rollout, json!({"type":"event_msg","payload":{
        "type":"task_complete","turn_id":"turn-1","error":{"message":"Selected model is at capacity."}
    }}).to_string()).unwrap();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"check the tests too","transcript_path":rollout}),
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for request in [lead, joined] {
        let message = fixture.request(request).prompt.id;
        let status = fixture
            .worker
            .control_message_at(message, fixture.now)
            .unwrap();
        assert_eq!(status["requests"][0]["turn_ended"], true, "{status}");
        assert_eq!(status["requests"][0]["settling"], true, "{status}");
    }
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for request in [lead, joined] {
        let message = fixture.request(request).prompt.id;
        let status = fixture
            .worker
            .control_message_at(message, fixture.now)
            .unwrap();
        assert_eq!(status["requests"][0]["stage"], "abandoned", "{status}");
        assert_eq!(status["requests"][0]["settling"], false, "{status}");
    }
}

/// An ended turn whose agent then went unavailable is settling: async
/// waiters, which finish only on a complete message, wait for its reason.
#[test]
fn an_ended_turn_whose_agent_is_unavailable_is_settling() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.status(RuntimeStatus::Unavailable);
    let message = fixture.request(lead).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["requests"][0]["turn_ended"], true, "{status}");
    assert_eq!(status["requests"][0]["settling"], true, "{status}");
}

/// Review of #128: a provider that exits after its turn ended left the
/// request open forever, so `send --async` / `wait --async` never exited.
/// Once the settle window passes from when the agent went unavailable, the
/// request closes as `agent_unavailable` and the waiters exit 1.
#[test]
fn an_ended_turn_on_an_unavailable_agent_eventually_releases_async_waiters() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    // Ended long ago; the window counts from going unavailable.
    fixture.now += CALLBACK_GRACE_MS * 10;
    fixture.status(RuntimeStatus::Unavailable);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Unavailable);
    fixture.consume();
    let message = fixture.request(lead).prompt.id;
    let status = fixture
        .worker
        .control_message_at(message, fixture.now)
        .unwrap();
    assert_eq!(status["complete"], true, "{status}");
    assert_eq!(status["requests"][0]["stage"], "abandoned", "{status}");
    assert_eq!(
        status["requests"][0]["reason"], "agent_unavailable",
        "{status}"
    );
    assert_eq!(status["requests"][0]["settling"], false, "{status}");
}

/// An agent that is unavailable only while it reattaches (a Bus restart)
/// and is Idle again within the window settles as an idle agent would.
#[test]
fn an_ended_turn_whose_agent_reattaches_within_the_window_settles_as_idle() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "implement the change", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.status(RuntimeStatus::Unavailable);
    fixture.now += CALLBACK_GRACE_MS - 10;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
}

/// Going unavailable cannot prove which turn accepted unclaimed steering.
#[test]
fn unavailable_unclaimed_steering_surfaces_uncertainty_without_borrowing_a_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Unavailable);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "parser written");
    assert!(fixture.reply(correction).is_none());
    assert_eq!(fixture.request(correction).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(correction).failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(fixture.request(correction).submission_notice().is_some());
}

fn answered_with_a_running_shell(fixture: &mut Fixture) -> RequestId {
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(fixture, "run the build", "turn-1");
    fixture.record(claude_stop("turn-1", "Waiting on the build.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_completed_without_reply(fixture, lead);
    assert_eq!(
        fixture.request(lead).provider_turn_id.as_deref(),
        Some("turn-1")
    );
    assert_reply_text_absent(fixture, "Waiting on the build.");
    lead
}

/// A turn that ended without a `Stop` and never held a reply completes with
/// no stored reply and no failure reason.
#[test]
fn a_turn_without_a_held_reply_still_closes_as_reply_not_captured() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "build it", "turn-1");
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += CALLBACK_GRACE_MS + 1;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
}

/// Live (2026-10-16): the orchestrator's replies were held behind its own
/// `bus send --async` waits on workers. Those are not work the reply waits
/// on, so a Stop listing only Bus waits is the final reply at once.
#[test]
fn a_claude_reply_is_not_held_by_the_agents_own_bus_waits() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(claude_startup());
    fixture.consume();
    let lead = running(&mut fixture, "dispatch the review", "turn-1");
    fixture.record(json!({
        "hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"Review dispatched; I'll report when it lands.",
        "background_tasks":[
            {"id":"w1","type":"shell","status":"running","description":"Wait","command":"bus send --async --room qa --as orch --to rev --text 'review #1'"},
            {"id":"w2","type":"shell","status":"running","description":"Wait","command":"/opt/bus/bin/bus --dev wait 12"}
        ],
        "session_crons":[]
    }));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "Review dispatched; I'll report when it lands.");
}

/// Live MASTER regression: five messages were answered by five distinct
/// Claude turns while the same background shell kept every Stop held.
#[test]
fn five_shell_held_message_turns_keep_their_own_replies() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let first = answered_with_a_running_shell(&mut fixture);
    let mut messages = vec![(first, "Waiting on the build.".to_owned())];
    for n in 2..=5 {
        let prompt = format!("question {n}");
        let answer = format!("answer {n}");
        let request = fixture.send(&prompt, false);
        fixture.deliver();
        fixture.started(&format!("turn-{n}"), &prompt);
        fixture.consume();
        fixture.status(RuntimeStatus::Working);
        fixture.record(claude_stop(&format!("turn-{n}"), &answer, &["b1"]));
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        fixture.consume();
        messages.push((request, answer));
    }
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for (index, (id, answer)) in messages.iter().enumerate() {
        let turn = format!("turn-{}", index + 1);
        assert_completed_without_reply(&fixture, *id);
        assert_reply_text_absent(&fixture, answer);
        assert_eq!(fixture.request(*id).group, None);
        assert_eq!(
            fixture.request(*id).provider_turn_id.as_deref(),
            Some(turn.as_str()),
            "message {} lost its own turn",
            index + 1
        );
        let status = fixture
            .worker
            .control_message_at(fixture.request(*id).prompt.id, fixture.now)
            .unwrap();
        assert!(status["requests"][0]["reply"].is_null(), "{status}");
        assert_eq!(status["requests"][0]["stage"], "ended");
    }
}

/// A message answered in its own turn while an earlier request's shell still
/// runs keeps that answer when the shell later wakes a turn of its own: the
/// wake turn reports on the earlier request's work, not this message.
#[test]
fn a_message_answered_in_its_own_turn_keeps_its_reply_when_an_earlier_shell_wakes() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = answered_with_a_running_shell(&mut fixture);
    let question = fixture.send("did the docs pr merge?", false);
    fixture.deliver();
    fixture.started("turn-2", "did the docs pr merge?");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-2", "Yes, the docs pr merged.", &["b1"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.started(
        "turn-3",
        "<task-notification>b1 completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-3", "Build green; pr 1 is ready.", &[]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_completed_without_reply(&fixture, question);
    assert_eq!(
        fixture.request(lead).provider_turn_id.as_deref(),
        Some("turn-1")
    );
    assert_eq!(
        fixture.request(question).provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_eq!(fixture.request(question).group, None);
    assert_reply_text_absent(&fixture, "Waiting on the build.");
    assert_reply_text_absent(&fixture, "Yes, the docs pr merged.");
    assert_reply_text_absent(&fixture, "Build green; pr 1 is ready.");
}

/// A second input without turn proof cannot borrow the new turn's answer;
/// the original shell-starting request keeps its already captured answer.
#[test]
fn an_unclaimed_input_after_an_answered_turn_cannot_borrow_the_next_answer() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = answered_with_a_running_shell(&mut fixture);
    let first = fixture.send("check the docs pr", false);
    fixture.deliver();
    fixture.started("turn-2", "check the docs pr");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let folded = fixture.send("and the changelog", false);
    fixture.deliver();
    fixture.record(claude_stop(
        "turn-2",
        "Docs pr and changelog are fine.",
        &[],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now = crate::messaging::storage::io::now_ms() + 5_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_completed_without_reply(&fixture, first);
    assert_eq!(
        fixture.request(lead).provider_turn_id.as_deref(),
        Some("turn-1")
    );
    assert_eq!(
        fixture.request(first).provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_reply_text_absent(&fixture, "Waiting on the build.");
    assert_reply_text_absent(&fixture, "Docs pr and changelog are fine.");
    assert!(fixture.reply(folded).is_none());
    assert_eq!(fixture.request(folded).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(folded).failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
}

/// An older request's shell can wake while the new request still owns a
/// different running shell. Its report must not replace the new held answer.
#[test]
fn an_inherited_shell_wake_cannot_replace_the_current_requests_held_answer() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = answered_with_a_running_shell(&mut fixture);
    let question = fixture.send("review the docs", false);
    fixture.deliver();
    fixture.started("turn-2", "review the docs");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "Docs reviewed; tests still run.",
        &["b1", "b2"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.started(
        "turn-3",
        "<task-notification><task-id>b1</task-id>completed</task-notification>",
    );
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop("turn-3", "Earlier build completed.", &["b2"]));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_completed_without_reply(&fixture, question);
    assert_eq!(
        fixture.request(lead).provider_turn_id.as_deref(),
        Some("turn-1")
    );
    assert_eq!(
        fixture.request(question).provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_reply_text_absent(&fixture, "Waiting on the build.");
    assert_reply_text_absent(&fixture, "Docs reviewed; tests still run.");
    assert_reply_text_absent(&fixture, "Earlier build completed.");
}

/// An actual Bus message can contain the same text as a task notification.
/// Its matching submit callback still owns the new turn and its reply.
#[test]
fn a_bus_message_matching_an_inherited_task_notification_keeps_its_reply() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = answered_with_a_running_shell(&mut fixture);
    let prompt = "<task-notification><task-id>b1</task-id>completed</task-notification>";
    let question = fixture.send(prompt, false);
    fixture.deliver();
    fixture.started("turn-2", prompt);
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.record(claude_stop(
        "turn-2",
        "That is a task notification.",
        &["b1"],
    ));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_completed_without_reply(&fixture, question);
    assert_eq!(
        fixture.request(lead).provider_turn_id.as_deref(),
        Some("turn-1")
    );
    assert_eq!(
        fixture.request(question).provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_eq!(fixture.request(question).group, None);
    assert_reply_text_absent(&fixture, "Waiting on the build.");
    assert_reply_text_absent(&fixture, "That is a task notification.");
}

/// A rollout timestamp `offset_ms` after the fixture's clock.
fn rollout_at(fixture: &Fixture, offset_ms: i64) -> String {
    let ms = i128::from(fixture.now) + i128::from(offset_ms);
    time::OffsetDateTime::from_unix_timestamp_nanos(ms * 1_000_000)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap()
}

/// Codex 0.163.0 rollout lines for one turn (shapes measured 2026-10-20).
fn codex_turn(at: &str, turn: &str, prompt: &str, answer: Option<&str>) -> String {
    let mut lines = vec![
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
        json!({"timestamp":at,"type":"event_msg","payload":{"type":"item_completed","turn_id":turn,
            "item":{"type":"UserMessage","content":[{"type":"text","text":prompt}]}}}),
    ];
    if let Some(answer) = answer {
        lines.push(
            json!({"timestamp":at,"type":"event_msg","payload":{"type":"task_complete",
            "turn_id":turn,"last_agent_message":answer}}),
        );
    }
    lines.iter().map(|line| format!("{line}\n")).collect()
}

fn append(path: &std::path::Path, lines: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
}

/// Runs the agent's turn to Idle with no `Stop` and waits out the settle window.
fn end_without_stop(fixture: &mut Fixture) {
    fixture.status(RuntimeStatus::Working);
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
}

fn assert_completed_without_reply(fixture: &Fixture, id: RequestId) {
    let request = fixture.request(id);
    assert_eq!(request.phase, RequestPhase::Completed, "{id:?}");
    assert!(
        request.failure_reason.is_none(),
        "{id:?} {:?}",
        request.failure_reason
    );
    assert!(
        fixture.reply(id).is_none(),
        "{id:?} {:?}",
        fixture.reply(id)
    );
}

fn assert_reply_text_absent(fixture: &Fixture, text: &str) {
    assert!(
        fixture
            .worker
            .state
            .requests()
            .all(|request| { request.reply().is_none_or(|reply| reply.text != text) }),
        "{text} stored on a request"
    );
    assert!(
        fixture
            .worker
            .state
            .room(fixture.room)
            .unwrap()
            .latest_replies
            .values()
            .all(|reply| reply.text != text),
        "{text} published"
    );
}

/// Live (2026-10-20, Codex 0.163.0): a worker removed its own worktree, its
/// cwd, during the turn. Codex then ran no `Stop` hook, though the turn
/// finished and its rollout recorded the answer. That record settles the
/// request without storing the answer.
#[test]
fn a_codex_turn_ending_without_stop_delivers_the_answer_its_rollout_recorded() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    let lead = fixture.send("merge the PR", false);
    fixture.deliver();
    fixture.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
    );
    fixture.consume();
    assert!(fixture.request(lead).trusted_start_bound);
    let at = rollout_at(&fixture, 1);
    append(
        &rollout,
        &codex_turn(&at, "turn-1", "merge the PR", Some("Merged #1627.")),
    );
    end_without_stop(&mut fixture);
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "Merged #1627.");
    assert!(fixture.error().is_none());
    assert!(fixture
        .worker
        .state
        .room(fixture.room)
        .unwrap()
        .latest_replies
        .is_empty());
    // A late Stop for the same turn is not a second reply.
    fixture.stopped("turn-1", "Merged #1627.");
    fixture.consume();
    assert_completed_without_reply(&fixture, lead);
    assert_reply_text_absent(&fixture, "Merged #1627.");
    assert!(fixture
        .worker
        .state
        .room(fixture.room)
        .unwrap()
        .latest_replies
        .is_empty());
}

/// Live (2026-10-20): the follow-up to the turn above was never bound, since
/// Codex ran no hook at all once its cwd was gone. Its rollout turn is the one
/// that ran the typed prompt after Bus sent it.
#[test]
fn an_unbound_codex_turn_delivers_the_answer_of_the_rollout_turn_that_ran_its_prompt() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session",
        "source":"startup","transcript_path":rollout}),
    );
    fixture.consume();
    // An identical turn older than the 60 s clock margin is not a candidate.
    let earlier = rollout_at(&fixture, -120_000);
    append(
        &rollout,
        &codex_turn(&earlier, "turn-0", "close the worktree", Some("old")),
    );
    let lead = fixture.send("close the worktree", false);
    fixture.deliver();
    assert!(!fixture.request(lead).trusted_start_bound);
    let typed = fixture.typed().last().unwrap().0.clone();
    let at = rollout_at(&fixture, 1);
    append(
        &rollout,
        &codex_turn(&at, "turn-2", &typed, Some("Copied 374 files.")),
    );
    end_without_stop(&mut fixture);
    assert_completed_without_reply(&fixture, lead);
    assert!(
        fixture.request(lead).provider_turn_id.is_none(),
        "the older turn's identity was borrowed"
    );
    assert_reply_text_absent(&fixture, "Copied 374 files.");
    assert_reply_text_absent(&fixture, "old");
}

/// An unbound request whose prompt no rollout turn ran since it was sent
/// still ends once the agent is idle: a turn the agent ran on its own, or an
/// older turn with the same text, is not borrowed, and the close is success
/// without a reply.
#[test]
fn an_unbound_codex_turn_without_its_prompt_in_the_rollout_stays_reply_not_captured() {
    for own_turn in [true, false] {
        let mut fixture = Fixture::new(Provider::Codex);
        let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
        fixture.record(
            json!({"hook_event_name":"SessionStart","session_id":"session",
            "source":"startup","transcript_path":rollout}),
        );
        fixture.consume();
        let lead = fixture.send("close the worktree", false);
        fixture.deliver();
        let typed = fixture.typed().last().unwrap().0.clone();
        let (at, prompt) = if own_turn {
            (rollout_at(&fixture, 1), "something else".to_owned())
        } else {
            (rollout_at(&fixture, -1), typed)
        };
        append(
            &rollout,
            &codex_turn(&at, "turn-2", &prompt, Some("not this")),
        );
        end_without_stop(&mut fixture);
        assert_completed_without_reply(&fixture, lead);
        assert!(fixture.request(lead).provider_turn_id.is_none());
        assert_reply_text_absent(&fixture, "not this");
    }
}

/// A bound turn whose rollout records no answer for it (the turn's
/// `task_complete` is missing, names another turn, or carries no message)
/// still closes once the agent is idle, without inventing a reply.
#[test]
fn a_codex_turn_whose_rollout_has_no_answer_for_it_stays_reply_not_captured() {
    for recorded in [
        codex_turn("2026-10-20T05:06:51.361Z", "turn-1", "merge the PR", None),
        codex_turn(
            "2026-10-20T05:06:51.361Z",
            "turn-other",
            "merge the PR",
            Some("other"),
        ),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1",
            "last_agent_message":null}})
        .to_string()
            + "\n",
    ] {
        let mut fixture = Fixture::new(Provider::Codex);
        let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
        append(&rollout, &recorded);
        let lead = fixture.send("merge the PR", false);
        fixture.deliver();
        fixture.record(
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
            "turn_id":"turn-1","prompt":"merge the PR","transcript_path":rollout}),
        );
        fixture.consume();
        end_without_stop(&mut fixture);
        assert_completed_without_reply(&fixture, lead);
        assert_eq!(
            fixture.request(lead).provider_turn_id.as_deref(),
            Some("turn-1")
        );
        assert_reply_text_absent(&fixture, "other");
    }
}

/// Verifier regression (2026-10-20, temp/bug-reply-not-captured section 6):
/// the earlier of two turns that ran the same prompt since it was sent sat
/// outside the rollout tail Bus read, so the later turn's answer was
/// delivered to the earlier request. Every turn since the send must be seen;
/// two matching turns are ambiguous and settle with no stored answer.
#[test]
fn truncated_tail_must_not_assign_a_later_identical_turn_to_an_earlier_request() {
    let mut fixture = Fixture::new(Provider::Codex);
    let rollout = fixture.dir.join("rollout-2026-10-20T13-06-43-abc.jsonl");
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session",
        "source":"startup","transcript_path":rollout}),
    );
    fixture.consume();
    let id = fixture.send("status?", false);
    fixture.deliver();
    let typed = fixture.typed().last().unwrap().0.clone();
    append(
        &rollout,
        &codex_turn(&rollout_at(&fixture, 1), "first", &typed, None),
    );
    // Real rollout reasoning and tool output often exceed 256 KiB.
    let reasoning = json!({"timestamp":rollout_at(&fixture, 2),"type":"response_item",
        "payload":{"type":"reasoning","encrypted_content":"x".repeat(300 * 1024)}});
    let complete = json!({"timestamp":rollout_at(&fixture, 60_000),"type":"event_msg",
        "payload":{"type":"task_complete","turn_id":"first","last_agent_message":"FIRST"}});
    append(&rollout, &format!("{reasoning}\n{complete}\n"));
    let at = rollout_at(&fixture, 60_001);
    append(&rollout, &codex_turn(&at, "later", &typed, Some("LATER")));
    fixture.now += 60_002;
    end_without_stop(&mut fixture);
    assert_completed_without_reply(&fixture, id);
    assert!(fixture.request(id).provider_turn_id.is_none());
    assert_reply_text_absent(&fixture, "LATER");
    assert_reply_text_absent(&fixture, "FIRST");
}

#[test]
fn reply_capture_recovered_log_is_not_written_for_an_unsaved_settlement() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "prompt", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    fixture.now += CALLBACK_GRACE_MS;
    fixture.status(RuntimeStatus::Idle);
    fixture
        .worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        assert!(fixture.worker.settle_requests().is_err());
        assert!(!capture.text().contains("bus.message.recovered"));
        assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
        fixture.worker.settle_requests().unwrap();
    });
    let log = capture.text();
    let rows = log
        .lines()
        .filter(|line| {
            line.contains("bus.message.recovered")
                && line.contains("reply_not_captured")
                && line.contains(&format!("request_id={} ", lead.0))
        })
        .count();
    assert_eq!(rows, 0, "{log}");
    assert!(!log.contains("bus.message.no_reply"), "{log}");
    assert_completed_without_reply(&fixture, lead);
}

#[test]
fn unbound_codex_0164_final_is_captured_despite_a_missing_directory_warning() {
    for answer in ["Recovered exact final.", ""] {
        let mut fixture = Fixture::new(Provider::Codex);
        let rollout = fixture.dir.join("rollout-0164.jsonl");
        fixture.record(
            json!({"hook_event_name":"SessionStart","session_id":"session",
            "source":"startup","transcript_path":rollout}),
        );
        fixture.consume();
        let lead = fixture.send("finish after hook loss", false);
        fixture.deliver();
        let typed = fixture.typed().last().unwrap().0.clone();
        assert!(!fixture.request(lead).trusted_start_bound);
        let at = rollout_at(&fixture, 1);
        append(&rollout, &codex_turn(&at, "turn-0164", &typed, None));
        // Preserve the recorded 0.164.0 envelope; replace only identity and text.
        let recorded = include_str!(
            "../../../agents/providers/codex/tests/fixtures/final-answer-0.164.0.jsonl"
        );
        for line in recorded.lines().skip(1).take(3) {
            let mut event: Value = serde_json::from_str(line).unwrap();
            let payload = &mut event["payload"];
            if payload["type"] == "message" {
                payload["internal_chat_message_metadata_passthrough"]["turn_id"] =
                    json!("turn-0164");
                payload["content"][0]["text"] = json!(answer);
                event["metadata"]["retained_source"]["id"]["turn_id"] = json!("turn-0164");
            } else {
                payload["turn_id"] = json!("turn-0164");
                if payload["type"] == "item_completed" {
                    payload["item"]["content"][0]["text"] = json!(answer);
                }
            }
            append(&rollout, &format!("{event}\n"));
        }
        fixture
            .worker
            .state
            .observe_working_directory_error(fixture.agent, Some("Launch directory removed".into()))
            .unwrap();
        end_without_stop(&mut fixture);
        assert_completed_without_reply(&fixture, lead);
        assert!(fixture.request(lead).provider_turn_id.is_none());
        if !answer.is_empty() {
            assert_reply_text_absent(&fixture, answer);
        }
        assert!(fixture
            .worker
            .state
            .room(fixture.room)
            .unwrap()
            .latest_replies
            .is_empty());
        fixture.stopped(
            "turn-0164",
            "late duplicate must not replace the recovered final",
        );
        fixture.consume();
        assert_completed_without_reply(&fixture, lead);
        assert_reply_text_absent(
            &fixture,
            "late duplicate must not replace the recovered final",
        );
        assert!(fixture
            .worker
            .state
            .room(fixture.room)
            .unwrap()
            .latest_replies
            .is_empty());
        assert_eq!(
            fixture
                .worker
                .state
                .requests()
                .filter(|request| request.reply().is_some())
                .count(),
            0
        );
    }
}

#[test]
fn provider_flake_uses_only_fresh_owned_exhausted_quota_metadata_for_the_reset() {
    use crate::agents::providers::usage::{UsageWindow, UsageWindows};
    use crate::messaging::coordinator::usage::UsageSnapshot;
    for valid in [true, false] {
        let mut f = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut f, "retry after quota", "turn-1");
        let now = io::now_ms();
        let reset = now / 1000 + 3600;
        f.worker.usage.claude = Some(UsageSnapshot {
            windows: UsageWindows {
                five_hour: Some(UsageWindow {
                    used_percent: 100.0,
                    resets_at: Some(reset),
                    window_minutes: 300,
                }),
                weekly: None,
            },
            read_at_ms: now,
            observed_by_agent: if valid { f.agent } else { AgentId(999999) },
        });
        f.record(json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1","error":"rate_limit"}));
        f.consume();
        f.worker.submit_ready().unwrap();
        let held = f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .unwrap();
        if valid {
            assert!(held.retry_at_ms >= reset * 1000 + 2000);
        } else {
            assert!(held.retry_at_ms < now + 301_000);
        }
        assert!(!f.request(lead).settled());
    }
}

#[test]
fn provider_flake_reset_diagnostics_follow_successful_persistence_once() {
    use crate::agents::providers::usage::{UsageWindow, UsageWindows};
    use crate::messaging::coordinator::usage::UsageSnapshot;
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "retry after quota", "turn-1");
    f.record(
        json!({"hook_event_name":"StopFailure","session_id":"session",
        "prompt_id":"turn-1","error":"rate_limit"}),
    );
    f.consume();
    let before = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .provider_retry
        .clone()
        .unwrap();
    let now = io::now_ms();
    let reset = now / 1000 + 3600;
    f.worker.usage.claude = Some(UsageSnapshot {
        windows: UsageWindows {
            five_hour: Some(UsageWindow {
                used_percent: 100.0,
                resets_at: Some(reset),
                window_minutes: 300,
            }),
            weekly: None,
        },
        read_at_ms: now,
        observed_by_agent: f.agent,
    });
    f.worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        assert!(f.worker.submit_ready().is_err());
        assert_eq!(
            f.worker
                .state
                .agent(f.agent)
                .unwrap()
                .provider_retry
                .as_ref(),
            Some(&before)
        );
        assert!(
            !capture.text().contains("bus.provider.reset"),
            "{}",
            capture.text()
        );
        f.worker.submit_ready().unwrap();
        f.worker.submit_ready().unwrap();
    });
    let log = capture.text();
    let rows: Vec<_> = log
        .lines()
        .filter(|line| line.contains("bus.provider.reset"))
        .collect();
    assert_eq!(rows.len(), 1, "{log}");
    for field in [
        format!("agent_id={}", f.agent.0),
        format!("request_id={}", lead.0),
        format!("retry_at_ms={}", reset * 1000 + 2000),
        "provider=ClaudeCode".into(),
    ] {
        assert!(rows[0].contains(&field), "missing {field}: {}", rows[0]);
    }
}

#[cfg(unix)]
#[path = "restart_epoch_test.rs"]
mod restart_epoch_tests;

#[test]
fn restart_published_recovers_distinct_committed_steering_final() {
    for restored in [false, true] {
        let mut f = Fixture::new(Provider::Codex);
        let rollout = f.dir.join("rollout-finished-steering.jsonl");
        let lead = f.send("finish the first task", false);
        f.deliver();
        let lead_payload = f.typed().last().unwrap().0.clone();
        f.record(
            json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
            "turn_id":"turn-1","prompt":"finish the first task","transcript_path":rollout}),
        );
        f.consume();
        f.status(RuntimeStatus::Working);
        let member = f.send("answer the independent followup", false);
        f.deliver();
        let member_payload = f.typed().last().unwrap().0.clone();
        assert!(f.request(member).steered && !f.request(member).trusted_start_bound);
        append(
            &rollout,
            &codex_turn(
                &rollout_at(&f, 1),
                "turn-1",
                &lead_payload,
                Some("first committed answer"),
            ),
        );
        append(
            &rollout,
            &codex_turn(
                &rollout_at(&f, 2),
                "turn-2",
                &member_payload,
                Some("second committed answer"),
            ),
        );
        f.record(json!({"hook_event_name":"Stop","session_id":"session",
            "turn_id":"turn-1","last_assistant_message":"first committed answer","transcript_path":rollout}));
        f.consume();
        assert!(f.reply(lead).is_none());
        assert!(
            f.request(lead).pending_final.is_some(),
            "the completed lead stays an anchor while the followup is unproven"
        );
        assert_reply_text_absent(&f, "first committed answer");
        if restored {
            restart_lifecycle_reopen(&mut f, true);
            f.worker.tick().unwrap();
        } else {
            f.status(RuntimeStatus::Idle);
            f.worker.settle_requests().unwrap();
        }
        let outcomes: Vec<_> = [lead, member]
            .into_iter()
            .map(|id| {
                (
                    f.request(id).phase,
                    f.reply(id),
                    f.request(id).failure_reason.clone(),
                )
            })
            .collect();
        assert_eq!(
            outcomes,
            vec![
                (RequestPhase::Completed, None, None),
                (RequestPhase::Completed, None, None),
            ],
            "restored={restored}: every committed turn settles without a stored reply"
        );
        assert_reply_text_absent(&f, "first committed answer");
        assert_reply_text_absent(&f, "second committed answer");
    }
}

#[test]
fn restart_published_continues_the_pre_stop_started_successor() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "first task before restart", "turn-1");
    let member = f.send("followup started before restart", false);
    f.deliver();
    assert!(f.request(member).steered && !f.request(member).trusted_start_bound);
    let queued = f.send("fresh queued task", true);
    f.stopped("turn-1", "first task completed");
    f.started("turn-2", "followup started before restart");
    restart_lifecycle_reopen(&mut f, true);
    f.worker.tick().unwrap();
    assert_completed_without_reply(&f, lead);
    assert_reply_text_absent(&f, "first task completed");
    assert!(!f.request(member).settled());
    assert!(f.request(member).provider_turn_id.is_none());
    assert!(f.request(member).failure_reason.is_none());
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(member)
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    let notice = f.typed().last().unwrap().0.clone();
    f.stopped("turn-2", "late end from the interrupted execution");
    f.consume();
    assert!(!f.request(member).settled());
    f.started("turn-3", &notice);
    f.consume();
    f.stopped("turn-3", "resumed successor completed");
    f.consume();
    assert_completed_without_reply(&f, member);
    f.status(RuntimeStatus::Idle);
    f.deliver();
    assert_ne!(f.request(queued).phase, RequestPhase::Queued);
}

#[cfg(unix)]
#[test]
fn restart_published_continues_independent_kimi_inputs_without_stranding_the_owner() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = super::kimi_steering_tests::home(&f);
    std::fs::write(&wire, "").unwrap();
    let first = running(&mut f, "older independent input", "0");
    let second = f.send("newer independent input", false);
    f.deliver();
    f.started("1", "newer independent input");
    f.consume();
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(second)
    );
    for id in [first, second] {
        assert_eq!(f.request(id).phase, RequestPhase::Active);
        assert!(f.request(id).trusted_start_bound);
        assert_eq!(f.request(id).group, None);
    }
    let queued = f.send("pending after both old inputs", true);
    let before = f.typed().len();
    restart_lifecycle_reopen(&mut f, true);
    f.worker.tick().unwrap();
    for id in [first, second] {
        assert!(!f.request(id).settled(), "{:?}", f.request(id));
        assert!(f.request(id).failure_reason.is_none());
        assert_eq!(f.request(id).group, None);
    }
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(second)
    );
    assert_eq!(
        f.typed().len(),
        before + 1,
        "the retained current owner must be deliverable"
    );
    for (id, turn) in [(second, "2"), (first, "3")] {
        let notice = f.typed().last().unwrap().0.clone();
        assert!(notice.contains(&format!("message {}", f.request(id).prompt.id.0)));
        f.started(turn, &notice);
        f.consume();
        end_restarted_turn(&mut f, turn, "resumed input completed", Some(&wire));
        assert_completed_without_reply(&f, id);
        f.status(RuntimeStatus::Idle);
        f.deliver();
    }
    assert_ne!(f.request(queued).phase, RequestPhase::Queued);
}

#[test]
fn restart_published_logs_only_durable_unbound_closures_once() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = f.send("unconfirmed lead interrupted by restore", false);
    f.deliver();
    f.status(RuntimeStatus::Working);
    let member = f.send("steering interrupted by restore", false);
    f.deliver();
    assert_eq!(f.request(member).group, Some(lead));
    restart(&mut f);
    f.status(RuntimeStatus::Idle);
    f.worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        assert!(f.worker.settle_requests().is_err());
        assert!(
            !capture.text().contains("bus.message.recovered"),
            "no closure before persistence"
        );
        assert!(!f.request(lead).settled());
        assert!(!f.request(member).settled());
        f.worker.settle_requests().unwrap();
        f.worker.settle_requests().unwrap();
    });
    let log = capture.text();
    let rows: Vec<_> = log
        .lines()
        .filter(|l| l.contains("bus.message.recovered") && l.contains("reply_not_captured"))
        .collect();
    assert_eq!(
        rows.len(),
        2,
        "each durable affected request logs once: {log}"
    );
    for id in [lead, member] {
        assert!(
            rows.iter()
                .any(|line| line.contains(&format!("request_id={}", id.0))
                    && line.contains(&format!("lead_id={}", lead.0))),
            "{log}"
        );
    }
}

#[test]
fn restart_published_retained_receipt_after_saved_ack_preserves_successor() {
    for boundary in ["same-worker", "consumed-reopen", "retained-reopen"] {
        let mut f = Fixture::new(Provider::Codex);
        let old = running(&mut f, "old interrupted execution", "turn-1");
        let successor = f.send("fresh successor after native replacement", true);
        f.worker.transport = restart_lifecycle_transport(&f, true);
        f.worker.tick().unwrap();
        assert!(!f.request(old).settled());
        assert_eq!(f.request(successor).phase, RequestPhase::Queued);
        let notice = f.typed().last().unwrap().0.clone();
        f.started("turn-2", &notice);
        f.consume();
        f.stopped("turn-2", "continued original completed");
        f.consume();
        assert_completed_without_reply(&f, old);
        f.status(RuntimeStatus::Idle);
        f.deliver();
        assert_eq!(f.request(successor).phase, RequestPhase::Active);

        f.now += 1;
        restart_lifecycle_receipt(&f, "restarted");
        let path = crate::utils::restart_record::outcome_path(&f.dir);
        let receipt = std::fs::read(&path).unwrap();
        f.worker.announce_recorded_restart().unwrap();
        assert!(!path.exists());
        assert_eq!(f.request(successor).phase, RequestPhase::Active);

        if boundary != "consumed-reopen" {
            // Reconstruct the exact reachable crash cut after the receipt's
            // durable state save but before its following outcome-file unlink.
            // Only the identical disk receipt is retained; no model field changes.
            std::fs::write(&path, &receipt).unwrap();
        }
        if boundary != "same-worker" {
            // The same restored terminal is still alive: this is coordinator
            // reconnect, not a second replacement that could mask replay.
            restart_lifecycle_reopen(&mut f, true);
        }
        f.worker.tick().unwrap();
        assert_eq!(
            (
                f.request(successor).phase,
                f.request(successor).failure_reason.as_deref()
            ),
            (RequestPhase::Active, None),
            "boundary={boundary}: an already acknowledged receipt must not interrupt its successor"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(successor)
        );
    }
}

// Portable behavioral proof: the generation is wire metadata, so this same
// test also compiles on an older server/client that ignores that JSON field.
#[cfg(unix)]
struct RestartGenerationProofNative {
    methods: Arc<Mutex<Vec<Method>>>,
    info: schema::AgentInfo,
}
#[cfg(unix)]
impl Transport for RestartGenerationProofNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(
            &method,
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_)
        ) {
            self.methods.lock().unwrap().push(method);
            return Ok(ResponseResult::AgentList {
                agents: vec![self.info.clone()],
            });
        }
        Native(self.methods.clone()).request(method)
    }
}
#[cfg(unix)]
fn restart_generation_proof_native(f: &Fixture, generation: u64) -> Box<dyn Transport> {
    let kind = crate::messaging::identity::provider_kind(f.provider).label();
    Box::new(RestartGenerationProofNative {
        methods: f.native.clone(),
        info: serde_json::from_value(json!({
            "generation":generation,"terminal_id":"terminal","pane_id":"pane",
            "name":crate::messaging::identity::managed_name(f.room,f.agent),
            "agent":kind,"agent_status":"idle","interactive_ready":true,
            "workspace_id":"workspace","tab_id":"tab","focused":false,"revision":1,
            "agent_session":{"source":crate::messaging::identity::managed_source(f.provider),
                "agent":kind,"kind":"id","value":"session"}
        }))
        .unwrap(),
    })
}
#[cfg(unix)]
#[test]
fn restart_generation_without_rebind_or_receipt_continues_bound_and_closes_unbound_work() {
    let mut failures = Vec::new();
    for provider in [
        Provider::ClaudeCode,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        for bound in [false, true] {
            let mut f = Fixture::new(provider);
            let wire = (provider == Provider::Kimi).then(|| super::kimi_steering_tests::home(&f));
            f.worker.transport = restart_generation_proof_native(&f, 1);
            f.worker.tick().unwrap();
            let old = if bound {
                running(&mut f, "interrupted delivery", "1")
            } else {
                let id = f.send("interrupted awaiting-start delivery", false);
                f.deliver();
                id
            };
            let queued = f.send("queued successor", true);
            assert_eq!(f.request(old).phase, RequestPhase::Active);
            assert_eq!(f.request(queued).phase, RequestPhase::Queued);
            f.worker.transport = restart_generation_proof_native(&f, 2);
            f.worker.tick().unwrap();
            if bound {
                assert!(
                    !f.request(old).settled(),
                    "{provider:?}: {:?}",
                    f.request(old)
                );
                assert_eq!(f.request(queued).phase, RequestPhase::Queued);
                let notice = f.typed().last().unwrap().0.clone();
                assert!(notice.contains("Bus restarted"), "{notice}");
                f.started("2", &notice);
                f.consume();
                end_restarted_turn(&mut f, "2", "continued task completed", wire.as_deref());
                assert_completed_without_reply(&f, old);
                f.status(RuntimeStatus::Idle);
                f.deliver();
                assert_ne!(f.request(queued).phase, RequestPhase::Queued);
            } else if f.request(old).phase != RequestPhase::Abandoned
                || f.request(old).failure_reason.as_deref() != Some("reply_not_captured")
                || f.request(queued).phase == RequestPhase::Queued
            {
                failures.push(format!(
                    "{provider:?}/bound={bound}: old={:?}/{:?}, queued={:?}",
                    f.request(old).phase,
                    f.request(old).failure_reason,
                    f.request(queued).phase
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(unix)]
#[path = "restart_hypotheses_test.rs"]
mod restart_hypotheses_tests;
