use super::*;

fn composer_busy() -> Result<ResponseResult, TransportError> {
    Err(TransportError {
        message: "Waiting for the developer's composer".into(),
        code: Some(crate::protocol::api::AGENT_COMPOSER_BUSY.into()),
        definitely_rejected: true,
    })
}

fn composer_available() -> Result<ResponseResult, TransportError> {
    Ok(ResponseResult::AgentPrompted {
        agent: native_info("pane", "codex", "session"),
    })
}

#[test]
fn occupied_composer_retains_fifo_without_failure_or_expiry_across_reload() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut worker, agent, room, dir, calls) = fixture(
            provider,
            vec![composer_busy(), composer_busy(), composer_available()],
        );
        let first = queue(&mut worker, room, agent, "first");
        let second = queue(&mut worker, room, agent, "second");
        worker.submit_ready().unwrap();
        assert_eq!(worker.state.queued_requests(agent), &[first, second]);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        assert_eq!(
            worker.state.agent(agent).unwrap().wait_reason(),
            Some("composer_busy")
        );
        let mut restored = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        let future = io::now_ms() + 10 * crate::messaging::model::QUEUED_STALL_MS;
        assert!(restored
            .expire_stalled_queued_requests(future)
            .unwrap()
            .is_empty());
        worker.save(restored).unwrap();
        // A repeated occupied observation can coalesce a waited queue, but
        // rollback must restore every member in its original order.
        worker.submit_ready().unwrap();
        assert_eq!(worker.state.queued_requests(agent), &[first, second]);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        for id in [first, second] {
            assert_eq!(
                worker.state.request(id).unwrap().phase,
                RequestPhase::Queued
            );
            assert!(worker.state.request(id).unwrap().failure_reason.is_none());
        }
        assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
        assert_eq!(worker.state.notices(room).count(), 0);
        worker.submit_ready().unwrap();
        assert_eq!(calls.lock().unwrap().len(), 3);
        assert!(worker.state.queued_requests(agent).is_empty());
        assert_eq!(
            worker.state.request(first).unwrap().phase,
            RequestPhase::Active
        );
        assert_eq!(worker.state.request(second).unwrap().group, Some(first));
        assert_eq!(worker.state.agent(agent).unwrap().delivery_rejection, None);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn occupied_composer_retries_steering_without_changing_the_lead_turn() {
    for (provider, lead_finishes) in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor]
        .into_iter()
        .flat_map(|provider| [false, true].map(|ends| (provider, ends)))
    {
        let (mut worker, agent, room, dir, calls) = fixture(
            provider,
            vec![composer_available(), composer_busy(), composer_available()],
        );
        let lead = queue(&mut worker, room, agent, "lead");
        worker.submit_ready().unwrap();
        let hook = match provider {
            Provider::ClaudeCode => {
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":"turn","prompt":"lead"})
            }
            Provider::Codex => {
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"lead"})
            }
            Provider::Cursor => {
                json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn","prompt":"lead"})
            }
        };
        record(&dir, provider, hook);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Working, io::now_ms())
            .unwrap();
        let follow = queue(&mut worker, room, agent, "follow");
        let revision = worker
            .state
            .request(lead)
            .unwrap()
            .submission_status_revision;
        worker.submit_ready().unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(lead)
        );
        assert_eq!(
            worker
                .state
                .request(lead)
                .unwrap()
                .submission_status_revision,
            revision
        );
        assert_eq!(worker.state.queued_requests(agent), &[follow]);
        assert_eq!(worker.state.request(follow).unwrap().group, None);
        assert!(!worker.state.request(follow).unwrap().steered);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        if lead_finishes {
            // A finished lead must settle; it cannot be revived by another
            // Working observation just to retry the held steering message.
            worker
                .state
                .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
                .unwrap();
            let future = io::now_ms() + 10 * crate::messaging::model::QUEUED_STALL_MS;
            worker.expire_queued_requests(future).unwrap();
            assert_eq!(worker.state.queued_requests(agent), &[follow]);
            assert_eq!(
                worker.state.settle_ended_request(agent, future, |_| {
                    Some(crate::messaging::model::RecordedTurnEnd::Final {
                        turn_id: "turn".into(),
                        text: "lead final".into(),
                        prompts: vec!["lead".into()],
                    })
                }),
                Some(lead)
            );
            assert_eq!(
                worker.state.request(lead).unwrap().phase,
                RequestPhase::Completed
            );
        }
        worker.submit_ready().unwrap();
        assert_eq!(calls.lock().unwrap().len(), 3);
        assert_eq!(
            worker.state.request(follow).unwrap().phase,
            RequestPhase::Active
        );
        assert_eq!(
            worker.state.request(follow).unwrap().group,
            (!lead_finishes).then_some(lead)
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(if lead_finishes { follow } else { lead })
        );
        assert_eq!(worker.state.agent(agent).unwrap().delivery_rejection, None);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delivery_logs_correlate_submission_callback_and_persisted_reply() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
        worker.submit_ready().unwrap(); // Fake transport's unknown response is deliberately uncertain.
        record(&dir, Provider::Codex, json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"PRIVATE_PROMPT"}));
        record(&dir, Provider::Codex, json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"PRIVATE_REPLY"}));
        worker.consume_callbacks(agent, &dir.join("callbacks/launch")).unwrap();
        let mut state = worker.state.clone();
        state.observe_status(agent, RuntimeStatus::Idle, 100).unwrap();
        worker.save(state).unwrap();
        assert_eq!(worker.state.request(request).unwrap().phase, RequestPhase::Completed);
        let logs = capture.text();
        for event in ["bus.delivery.start", "bus.delivery.result", "bus.callback.observed", "bus.callback.correlated", "bus.reply.persisted"] {
            assert!(logs.contains(event), "missing {event}: {logs}");
        }
        assert!(logs.contains("uncertain"), "{logs}");
        assert!(logs.contains("request_id=4"), "{logs}");
        assert!(!logs.contains("PRIVATE_PROMPT") && !logs.contains("PRIVATE_REPLY"), "{logs}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn provider_start_hooks_bind_when_terminal_trims_trailing_prompt_whitespace() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        let request = queue(&mut worker, room, agent, "round trip payload ");
        worker.submit_ready().unwrap();
        let callback = match provider {
            Provider::ClaudeCode => json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session",
                "prompt_id": "turn",
                "prompt": "round trip payload"
            }),
            Provider::Codex => json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "session",
                "turn_id": "turn",
                "prompt": "round trip payload"
            }),
            Provider::Cursor => json!({
                "hook_event_name": "beforeSubmitPrompt",
                "conversation_id": "session",
                "generation_id": "turn",
                "prompt": "round trip payload"
            }),
        };
        record(&dir, provider, callback);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();

        assert!(
            worker.state.request(request).unwrap().trusted_start_bound,
            "terminal-trimmed prompt must bind for {provider:?}"
        );
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delivery_logs_do_not_claim_reply_persisted_when_storage_fails() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"PRIVATE_PROMPT"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"PRIVATE_REPLY"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let mut state = worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    std::fs::rename(worker.store.path(), dir.join("original-state.json")).unwrap();
    std::fs::create_dir(worker.store.path()).unwrap();
    capture.run(|| {
        assert!(worker.save(state).is_err());
    });
    let logs = capture.text();
    assert!(logs.contains("bus.storage.failed"), "{logs}");
    assert!(!logs.contains("bus.reply.persisted"), "{logs}");
    assert!(!logs.contains("PRIVATE_"), "{logs}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_first_room_prompt_does_not_wait_for_its_deferred_session_start() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    fresh_idle_poll(&mut worker, agent);
    queue(&mut worker, room, agent, "first real prompt");
    worker.submit_ready().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["agent.prompt_if_unbound"]);
    // An uncertain first submit remains owned; it must not bootstrap twice.
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    let recovered_calls = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: recovered_calls.clone(),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    queue(&mut recovered, room, agent, "second must wait");
    recovered.submit_ready().unwrap();
    assert!(recovered_calls.lock().unwrap().is_empty());
    assert!(recovered
        .state
        .agent(agent)
        .unwrap()
        .current_request
        .is_some());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn codex_idle_under_a_visible_dialog_keeps_its_message_queued() {
    // A fresh Codex reports Idle under its startup update chooser; a message
    // typed there picks "Update now" and is lost with submission_unconfirmed.
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker
        .state
        .observe_dialog(
            agent,
            true,
            worker.state.agent(agent).unwrap().observed_at_ms,
        )
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "brief");
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Queued
    );
    worker
        .state
        .observe_dialog(
            agent,
            false,
            worker.state.agent(agent).unwrap().observed_at_ms,
        )
        .unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["agent.prompt_if_unbound"]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deferred_codex_start_and_final_survive_background_sessions_without_rebinding() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "first room prompt");
    worker.submit_ready().unwrap();
    for value in [
        json!({"hook_event_name":"SessionStart","session_id":"title-session","transcript_path":null}),
        json!({"hook_event_name":"SessionStart","session_id":"root-session"}),
        json!({"hook_event_name":"UserPromptSubmit","session_id":"root-session","turn_id":"root-turn","prompt":"first room prompt"}),
        json!({"hook_event_name":"Stop","session_id":"root-session","turn_id":"root-turn","last_assistant_message":"ROOT_FINAL"}),
        json!({"hook_event_name":"SessionStart","session_id":"memory-session","transcript_path":null}),
        json!({"hook_event_name":"Stop","session_id":"title-session","turn_id":"title-turn","last_assistant_message":"wrong title","transcript_path":null}),
    ] {
        record(&dir, Provider::Codex, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "ROOT_FINAL"
    );
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("root-session")
    );
    assert!(
        !worker
            .state
            .agent(agent)
            .unwrap()
            .session_binding_invalidated
    );
    worker.submit_ready().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["agent.prompt_if_unbound", "pane.report_agent_session"]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn session_start_reports_bind_through_native_provider_session_adapter() {
    struct NativeSessionTransport(
        Arc<Mutex<Option<crate::agents::resume::catalog::AgentSessionRef>>>,
    );
    impl Transport for NativeSessionTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::PaneReportAgentSession(params) = method {
                // The real API returns Ok even if this consumer rejects the source.
                // A transport-only mock would miss a silently discarded identity.
                *self.0.lock().unwrap() = crate::agents::resume::catalog::session_ref_from_report(
                    &params.source,
                    &params.agent,
                    params.agent_session_id,
                    params.agent_session_path,
                );
            }
            Ok(ResponseResult::Ok {})
        }
    }
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let (mut worker, agent, _, dir, _) = fixture(provider, vec![]);
        let native_session = Arc::new(Mutex::new(None));
        worker.transport = Box::new(NativeSessionTransport(native_session.clone()));
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        let callback = if provider == Provider::Cursor {
            json!({"hook_event_name":"sessionStart","conversation_id":"native-session"})
        } else {
            json!({"hook_event_name":"SessionStart","session_id":"native-session"})
        };
        record(&dir, provider, callback);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        assert_eq!(
            native_session
                .lock()
                .unwrap()
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("native-session"),
            "native session adapter must retain {provider:?} identity"
        );
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .runtime_identity
                .session_id
                .as_deref(),
            Some("native-session")
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// The fixture polled at 1 ms. Bus fails a write it could not confirm once
/// its agent sits idle past `DELIVERED_STALL_MS`, so typing starts on the real clock.
fn fresh_idle_poll(worker: &mut Worker, agent: AgentId) {
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
}

#[test]
fn an_unconfirmed_write_is_failed_at_its_bound_and_the_next_message_is_typed() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::ClaudeCode,
        vec![
            Err(TransportError {
                message: "Provider did not accept prompt before submission deadline".into(),
                code: Some("timeout".into()),
                definitely_rejected: false,
            }),
            Ok(ResponseResult::AgentPrompted {
                agent: serde_json::from_value(json!({
                    "terminal_id":"terminal", "pane_id":"pane", "agent":"claude",
                    "agent_status":"idle", "workspace_id":"workspace", "tab_id":"tab",
                    "focused":false, "revision":1
                }))
                .unwrap(),
            }),
        ],
    );
    fresh_idle_poll(&mut worker, agent);
    let lost = queue(&mut worker, room, agent, "worker final");
    let next = queue(&mut worker, room, agent, "blocked notice");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(lost).unwrap().uncertain_outcome);
    let bound = worker.state.request(lost).unwrap().progress_at_ms.unwrap()
        + crate::messaging::model::DELIVERED_STALL_MS;
    worker.expire_unconfirmed_submissions(bound).unwrap();
    let failed = worker.state.request(lost).unwrap();
    assert_eq!(failed.phase, RequestPhase::Abandoned);
    assert_eq!(
        failed.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 2);
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(next)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn operational_notice_unconfirmed_write_retains_unknown_outcome_without_chat() {
    let (mut worker, agent, room, dir, _calls) = fixture(
        Provider::ClaudeCode,
        vec![Err(TransportError {
            message: "Provider did not accept prompt before submission deadline".into(),
            code: Some("timeout".into()),
            definitely_rejected: false,
        })],
    );
    fresh_idle_poll(&mut worker, agent);
    let lost = queue(&mut worker, room, agent, "worker final");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(lost).unwrap().uncertain_outcome);
    let bound = worker.state.request(lost).unwrap().progress_at_ms.unwrap()
        + crate::messaging::model::DELIVERED_STALL_MS;
    let mut reported = worker.state.clone();
    reported.record_stalled_errors(bound).unwrap();
    assert!(reported
        .master_room()
        .is_none_or(|master| reported.notices(master.id).count() == 0));
    worker.expire_unconfirmed_submissions(bound - 1).unwrap();
    worker.expire_unconfirmed_submissions(bound).unwrap();
    let request = worker.state.request(lost).unwrap();
    assert_eq!(request.phase, RequestPhase::Abandoned);
    assert_eq!(
        request.failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(request.uncertain_outcome);
    let error = worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .unwrap();
    assert!(
        error.contains("outcome") && error.contains("unknown") && error.contains("will not retry"),
        "{error}"
    );
    assert!(worker.state.master_room().is_none_or(|master| worker
        .state
        .notices(master.id)
        .count()
        == 0));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_submission_survives_restart_and_never_retries() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "lost response".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    fresh_idle_poll(&mut worker, agent);
    let request = queue(&mut worker, room, agent, "same");
    let next = queue(&mut worker, room, agent, "next");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    let (transport_calls, no_replies) = (Arc::new(Mutex::new(Vec::new())), VecDeque::new());
    let fake = FakeTransport {
        replies: no_replies,
        calls: transport_calls.clone(),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 3)
        .unwrap();
    recovered.submit_ready().unwrap();
    assert!(transport_calls.lock().unwrap().is_empty());
    assert_eq!(recovered.state.queued_requests(agent), &[next]);
    assert_eq!(
        recovered.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_submission_rejects_wrong_session_start_binding() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "lost response".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"wrong-session","turn_id":"wrong-turn","prompt":"same"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"wrong-session","turn_id":"wrong-turn","last_assistant_message":"wrong reply"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(!worker.state.request(request).unwrap().trusted_start_bound);
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn changed_session_start_cannot_erase_and_rebind_known_identity() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "held after session changed");
    for _ in 0..2 {
        record(
            &dir,
            Provider::Codex,
            json!({"hook_event_name":"SessionStart","session_id":"other-session"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("session")
    );
    assert!(calls.lock().unwrap().is_empty());
    // A stale server identity/Idle observation must not reopen this launch.
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.state.confirm_hook_setup(agent).is_err());
    worker.save(worker.state.clone()).unwrap();
    let mut reloaded = worker.store.load().unwrap().unwrap();
    assert!(reloaded.agent(agent).unwrap().session_binding_invalidated);
    assert!(reloaded.confirm_hook_setup(agent).is_err());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cursor_stop_before_response_and_start_survives_detach_without_duplicate_reply() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Cursor,
        vec![Err(TransportError {
            message: "unknown".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    let transcript = dir.join("cursor-session.jsonl");
    let completed_transcript = concat!(
            "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"same\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"I will ask first.\"},{\"type\":\"tool_use\",\"name\":\"AskQuestion\"}]}}\n",
            "{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"real final\"}]}}\n",
            "{\"type\":\"turn_ended\",\"status\":\"success\"}\n",
    );
    // Cursor can publish hooks before its transcript writer finishes the turn.
    std::fs::write(
        &transcript,
        completed_transcript
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn","text":"I will ask first.real final","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_deref()
        .is_some_and(|message| message.contains("completed transcript")));
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        2
    );
    std::fs::write(&transcript, completed_transcript).unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn","prompt":"same"}),
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 4)
        .unwrap();
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, 5)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 6)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 7)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "real final"
    );
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    // Replayed raw callbacks cannot increment unread again after a crash/delete boundary.
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn","text":"I will ask first.real final","transcript_path":transcript}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    // A second request may legitimately produce exactly the same answer. Its
    // callback identity, not the uniqueness of its text, owns the room reply.
    let next = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    std::fs::write(
        &transcript,
        format!("{completed_transcript}{completed_transcript}"),
    )
    .unwrap();
    for value in [
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-2","prompt":"same"}),
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-2","status":"completed"}),
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn-2","text":"I will ask first.real final","transcript_path":transcript}),
    ] {
        record(&dir, Provider::Cursor, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 8)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    for (id, turn) in [(request, "turn"), (next, "turn-2")] {
        let original = worker.state.request(id).unwrap();
        assert_eq!(original.phase, RequestPhase::Completed);
        assert_eq!(original.provider_turn_id.as_deref(), Some(turn));
        assert_eq!(original.pending_final.as_ref().unwrap().text, "real final");
    }
    assert_eq!(worker.state.room(room).unwrap().unread_count, 2);
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn claude_still_requires_real_session_start_before_submission() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::ClaudeCode, vec![]);
    queue(&mut worker, room, agent, "queued");
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"fresh-session"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("fresh-session")
    );
    assert_eq!(*calls.lock().unwrap(), vec!["pane.report_agent_session"]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn claude_background_pending_reply_stays_active_until_its_trusted_continuation_settles() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = queue(&mut worker, room, agent, "review the plan");
    worker.submit_ready().unwrap();
    for value in [
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":"prompt-1","prompt":"review the plan"}),
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"prompt-1","last_assistant_message":"Waiting for reviewers","background_tasks":[{"task_id":"reviewer-1","status":"running"}],"session_crons":[]}),
    ] {
        record(&dir, Provider::ClaudeCode, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 20)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());

    for value in [
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":"prompt-2","prompt":"<task-notification>reviewer-1 finished</task-notification>"}),
        json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"prompt-2","last_assistant_message":"Ready","background_tasks":[],"session_crons":[]}),
    ] {
        record(&dir, Provider::ClaudeCode, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();

    let settled = worker.state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert_eq!(settled.provider_turn_id.as_deref(), Some("prompt-2"));
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "Ready"
    );
    let reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn claude_task_notification_after_a_stop_with_a_running_shell_keeps_the_bus_reply() {
    // Consumed together, or the notification turn in a later pass.
    for split in [false, true] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        let spool = dir.join("callbacks/launch");
        let request = queue(&mut worker, room, agent, "do the task");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("prompt-1", "do the task"),
        );
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop(
                "prompt-1",
                "Task done",
                json!([{"id":"shell-1","type":"shell","status":"running"}]),
            ),
        );
        if split {
            worker.consume_callbacks(agent, &spool).unwrap();
        }
        for value in claude_task_notification_turn("prompt-2", json!([])) {
            record(&dir, Provider::ClaudeCode, value);
        }
        worker.consume_callbacks(agent, &spool).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, 20)
            .unwrap();

        let settled = worker.state.request(request).unwrap();
        assert_eq!(settled.phase, RequestPhase::Completed, "split={split}");
        assert_eq!(settled.provider_turn_id.as_deref(), Some("prompt-1"));
        assert_eq!(
            worker.state.room(room).unwrap().latest_replies[&agent].text,
            "Task done"
        );
        assert_eq!(worker.state.agent(agent).unwrap().actionable_error, None);
        assert!(callbacks::records(&spool).unwrap().is_empty());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn claude_unrelated_turns_never_error_or_block_the_next_request() {
    // The notification turn may end while a monitor it started still runs.
    for background in [
        json!([]),
        json!([{"id":"monitor-1","type":"monitor","status":"running"}]),
    ] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        let spool = dir.join("callbacks/launch");
        let first = queue(&mut worker, room, agent, "first task");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("prompt-1", "first task"),
        );
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("prompt-1", "First done", json!([])),
        );
        worker.consume_callbacks(agent, &spool).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, 20)
            .unwrap();
        assert_eq!(
            worker.state.request(first).unwrap().phase,
            RequestPhase::Completed
        );

        // The shell finishes while the next request is pasted but not yet submitted.
        let second = queue(&mut worker, room, agent, "second task");
        worker.submit_ready().unwrap();
        // The fake transport leaves the submission uncertain; the turn adds no error.
        let submitted_error = worker.state.agent(agent).unwrap().actionable_error.clone();
        for value in claude_task_notification_turn("prompt-2", background.clone()) {
            record(&dir, Provider::ClaudeCode, value);
        }
        worker.consume_callbacks(agent, &spool).unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().actionable_error,
            submitted_error
        );
        assert!(!worker.state.request(second).unwrap().trusted_start_bound);

        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("prompt-3", "second task"),
        );
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("prompt-3", "Second done", json!([])),
        );
        worker.consume_callbacks(agent, &spool).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, 30)
            .unwrap();
        assert_eq!(
            worker.state.request(second).unwrap().phase,
            RequestPhase::Completed
        );
        assert_eq!(
            worker.state.room(room).unwrap().latest_replies[&agent].text,
            "Second done"
        );

        // With nothing in flight, the user typing in the terminal is only activity.
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("prompt-4", "typed by the user"),
        );
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("prompt-4", "Typed reply", json!([])),
        );
        worker.consume_callbacks(agent, &spool).unwrap();
        assert_eq!(worker.state.agent(agent).unwrap().actionable_error, None);
        assert_eq!(
            worker.state.room(room).unwrap().latest_replies[&agent].text,
            "Second done"
        );
        assert!(callbacks::records(&spool).unwrap().is_empty());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn codex_native_question_answer_keeps_the_active_request_until_its_final() {
    // A native async question answer invokes UserPromptSubmit while the task
    // is still running; it does not supply a Stop for the original prompt.
    for answer_turn in ["original-turn", "answer-turn"] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
        let spool = dir.join("callbacks/launch");
        let request = queue(&mut worker, room, agent, "finish the task");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::Codex,
            json!({
                "hook_event_name":"UserPromptSubmit", "session_id":"session",
                "turn_id":"original-turn", "prompt":"finish the task"
            }),
        );
        worker.consume_callbacks(agent, &spool).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Blocked, 10)
            .unwrap();
        record(
            &dir,
            Provider::Codex,
            json!({
                "hook_event_name":"UserPromptSubmit", "session_id":"session",
                "turn_id":answer_turn,
                "prompt":"<send_user_message_question_reply>\n[{\"answer\":\"Continue\"}]\n</send_user_message_question_reply>"
            }),
        );
        worker.consume_callbacks(agent, &spool).unwrap();
        assert!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .is_none(),
            "answering a native question must not report an unbound callback"
        );
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(request)
        );
        assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
        worker
            .state
            .observe_status(agent, RuntimeStatus::Working, 11)
            .unwrap();
        record(
            &dir,
            Provider::Codex,
            json!({
                "hook_event_name":"Stop", "session_id":"session",
                "turn_id":answer_turn, "last_assistant_message":"The task is finished"
            }),
        );
        worker.consume_callbacks(agent, &spool).unwrap();
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, 12)
            .unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Completed
        );
        assert_eq!(
            worker.state.room(room).unwrap().latest_replies[&agent].text,
            "The task is finished"
        );
        assert!(callbacks::records(&spool).unwrap().is_empty());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn saved_state_without_unrelated_turns_keeps_pending_reply_and_tracks_new_activity() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let spool = dir.join("callbacks/launch");
    let request = queue(&mut worker, room, agent, "finish the task");
    worker.submit_ready().unwrap();
    for value in [
        json!({"hook_event_name":"UserPromptSubmit", "session_id":"session", "turn_id":"original-turn", "prompt":"finish the task"}),
        json!({"hook_event_name":"Stop", "session_id":"session", "turn_id":"original-turn", "last_assistant_message":"The task is finished"}),
    ] {
        record(&dir, Provider::Codex, value);
    }
    worker.consume_callbacks(agent, &spool).unwrap();
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("state.json")).unwrap()).unwrap();
    document["state"]
        .as_object_mut()
        .unwrap()
        .remove("unrelated_provider_turns");
    drop(worker);
    std::fs::write(
        dir.join("state.json"),
        serde_json::to_vec(&document).unwrap(),
    )
    .unwrap();
    let mut worker = reopen_saved(&dir);
    record(
        &dir,
        Provider::Codex,
        json!({
            "hook_event_name":"UserPromptSubmit", "session_id":"session",
            "turn_id":"typed-turn", "prompt":"a separate terminal instruction"
        }),
    );
    worker.consume_callbacks(agent, &spool).unwrap();
    drop(worker);
    let mut worker = reopen_saved(&dir);
    record(
        &dir,
        Provider::Codex,
        json!({
            "hook_event_name":"Stop", "session_id":"session", "turn_id":"typed-turn",
            "last_assistant_message":"The separate terminal reply"
        }),
    );
    worker.consume_callbacks(agent, &spool).unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 12)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "The task is finished"
    );
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    assert!(callbacks::records(&spool).unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn text_typed_into_a_turn_the_agent_began_itself_binds_to_that_turn() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    // A task notification starts a turn while the native status still reads idle.
    record(
        &dir,
        Provider::ClaudeCode,
        claude(
            "UserPromptSubmit",
            "own",
            json!({"prompt":"<task-notification>"}),
        ),
    );
    consume(&mut worker, agent, &dir);
    fresh_idle_poll(&mut worker, agent);
    let request = queue(&mut worker, room, agent, "Answer the dialog");
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request),
        "Claude accepts a follow-up without waiting for the own-turn grace"
    );
    // A second pass must not paste twice; the provider reports this turn's input.
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    record(
        &dir,
        Provider::ClaudeCode,
        claude(
            "UserPromptSubmit",
            "own",
            json!({"prompt":"Answer the dialog"}),
        ),
    );
    record(
        &dir,
        Provider::ClaudeCode,
        claude("Stop", "own", json!({"last_assistant_message":"Answered"})),
    );
    consume(&mut worker, agent, &dir);
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    let settled = worker.state.request(request).unwrap();
    assert_eq!(settled.phase, RequestPhase::Completed);
    assert_eq!(settled.pending_final.as_ref().unwrap().text, "Answered");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cursor_steered_generation_stop_settles_the_group_from_hook_text_when_idle() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Cursor, vec![]);
    let first = queue(&mut worker, room, agent, "first message");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-1","prompt":"first message"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let follow = queue(&mut worker, room, agent, "follow up");
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, 3)
        .unwrap();
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-2","prompt":"follow up"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let transcript = dir.join("cursor-session.jsonl");
    std::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"follow up\"}]}}\n{\"role\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"unrelated\"}]}}\n",
    )
    .unwrap();
    let hook = "I'll search.The published docs still have no launch-time system prompt.";
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-2","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn-2","text":hook,"transcript_path":transcript}),
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 4)
        .unwrap();
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.request(first).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.request(follow).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        hook
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cursor_background_task_notice_is_not_the_room_reply() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Cursor, vec![]);
    let request = queue(&mut worker, room, agent, "real work");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-1","prompt":"real work"}),
    );
    let notice = "<timestamp>Wednesday, Oct 7, 2026, 8:30 PM (UTC+8)</timestamp>  <user_query>Briefly inform the user about the task result and perform any follow-up actions (if needed). If there's no follow-ups needed, don't explicitly say that.</user_query>";
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn-bg","prompt":notice}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn-bg","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn-bg","text":"The job finished."}),
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 3)
        .unwrap();
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_paste_the_provider_never_showed_is_requeued_and_retried_after_a_pause() {
    let not_shown = || {
        Err(TransportError {
            // Exactly what the API client reports: the server's message, no code.
            message: crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE.into(),
            code: Some("agent_prompt_not_shown".into()),
            definitely_rejected: true,
        })
    };
    let (mut worker, agent, room, dir, calls) =
        fixture(Provider::ClaudeCode, vec![not_shown(), not_shown()]);
    let request = queue(&mut worker, room, agent, "look\n\"/tmp/shot.png\"");
    worker.submit_ready().unwrap();
    let prompts = |calls: &Arc<Mutex<Vec<&'static str>>>| {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.starts_with("agent.prompt"))
            .count()
    };
    assert_eq!(prompts(&calls), 1);
    let state = &worker.state;
    assert_eq!(state.request(request).unwrap().phase, RequestPhase::Queued);
    assert!(!state.request(request).unwrap().uncertain_outcome);
    assert_eq!(state.agent(agent).unwrap().current_request, None);
    assert_eq!(
        state.agent(agent).unwrap().delivery_rejection.as_deref(),
        Some("prompt_not_shown")
    );

    // Each attempt pastes and clears in the agent's pane: not on every tick.
    worker.submit_ready().unwrap();
    assert_eq!(prompts(&calls), 1);
    for at in worker.withdrawn_at.values_mut() {
        *at -= crate::messaging::coordinator::delivery::PROMPT_NOT_SHOWN_RETRY;
    }
    worker.submit_ready().unwrap();
    assert_eq!(prompts(&calls), 2);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn aged_unconfirmed_submission(worker: &mut Worker, room: RoomId, agent: AgentId) -> RequestId {
    let sent = io::now_ms() - crate::messaging::model::DELIVERED_STALL_MS - 5_000;
    let mut state = worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Working, sent - 1)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, sent)
        .unwrap();
    state.set_draft_text(room, "finish the task").unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, sent).unwrap()[0];
    state.begin_submission(request, "launch", 0).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Uncertain {
                message: "Submit outcome unknown; no automatic retry: timeout".into(),
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    request
}

#[test]
fn unconfirmed_submission_with_spooled_final_is_completed_before_expiry() {
    // A delayed coordinator resumes with the provider's start and final already on disk.
    // Consuming those same records before the tick is the control for hook attribution.
    for consume_before_tick in [true, false] {
        let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
        let request = aged_unconfirmed_submission(&mut worker, room, agent);
        let mut info = native_info("pane", "claude", "session");
        info.interactive_ready = true;
        worker.transport = Box::new(FakeTransport {
            replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
            calls: Arc::new(Mutex::new(Vec::new())),
            state_path: dir.join("state.json"),
        });
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("landed", "finish the task"),
        );
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("landed", "Task finished", json!([])),
        );
        if consume_before_tick {
            consume(&mut worker, agent, &dir);
        }
        worker.tick().unwrap();
        let phase = worker.state.request(request).unwrap().phase;
        let final_text = worker
            .state
            .room(room)
            .unwrap()
            .latest_replies
            .get(&agent)
            .map(|reply| reply.text.clone());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(phase, RequestPhase::Completed,
            "matching hooks already in the spool must win over expiry; consume_before_tick={consume_before_tick}");
        assert_eq!(final_text.as_deref(), Some("Task finished"));
    }
}

#[test]
fn unconfirmed_submission_uses_the_current_working_poll_before_expiry() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = aged_unconfirmed_submission(&mut worker, room, agent);
    let mut info = native_info("pane", "claude", "session");
    info.interactive_ready = true;
    info.agent_status = schema::AgentStatus::Working;
    worker.transport = Box::new(FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    });
    worker.tick().unwrap();
    let phase = worker.state.request(request).unwrap().phase;
    let status = worker.state.agent(agent).unwrap().status;
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(status, RuntimeStatus::Working);
    assert_eq!(phase, RequestPhase::Submitting,
        "a current Working poll must protect the unconfirmed write from a preceding Idle observation");
}

#[test]
fn unconfirmed_submission_survives_failed_native_poll() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = aged_unconfirmed_submission(&mut worker, room, agent);
    worker.transport = Box::new(FakeTransport {
        replies: vec![Err(TransportError {
            message: "native poll failed".into(),
            code: Some("timeout".into()),
            definitely_rejected: false,
        })]
        .into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    });
    worker.tick().unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unconfirmed_submission_survives_failed_callback_reconciliation() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = aged_unconfirmed_submission(&mut worker, room, agent);
    let mut info = native_info("pane", "claude", "session");
    info.interactive_ready = true;
    worker.transport = Box::new(FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    });
    std::fs::rename(
        dir.join("callbacks/launch"),
        dir.join("callbacks/launch.saved"),
    )
    .unwrap();
    std::fs::write(
        dir.join("callbacks/launch"),
        b"not a readable spool directory",
    )
    .unwrap();
    assert!(worker.tick().is_err());
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn message_status_retains_native_withdrawal_detail_after_uncertain_settlement() {
    let detail = "The draft was left because it was edited. Inspect the composer before resending.";
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::ClaudeCode,
        vec![Err(TransportError {
            message: detail.into(),
            code: Some("timeout".into()),
            definitely_rejected: false,
        })],
    );
    fresh_idle_poll(&mut worker, agent);
    let request = queue(&mut worker, room, agent, "brief");
    worker.submit_ready().unwrap();
    let message = worker.state.request(request).unwrap().prompt.id;
    let bound = worker
        .state
        .request(request)
        .unwrap()
        .progress_at_ms
        .unwrap()
        + crate::messaging::model::DELIVERED_STALL_MS;
    worker.expire_unconfirmed_submissions(bound).unwrap();
    let status = worker.control_message_at(message, bound).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        status["requests"][0]["detail"]
            .as_str()
            .is_some_and(|value| value.contains(detail)),
        "{status}"
    );
}
