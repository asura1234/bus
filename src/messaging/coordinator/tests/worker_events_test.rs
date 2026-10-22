use super::*;

fn ready_worker(f: &mut Fixture) {
    ready_worker_at(f, io::now_ms() - 10_000);
}

fn ready_worker_at(f: &mut Fixture, at_ms: u64) {
    let mut state = f.worker.state.clone();
    state
        .set_agent_runtime_identity(
            f.agent,
            AgentRuntimeIdentity {
                launch_id: Some("worker".into()),
                terminal_id: Some("worker-terminal".into()),
                pane_id: Some("worker-pane".into()),
                session_id: Some("worker-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(f.agent).unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, at_ms)
        .unwrap();
    f.worker.save(state).unwrap();
}

fn callback(f: &Fixture, turn: &str, sequence: u64, kind: CallbackEventKind) -> ProviderCallback {
    ProviderCallback {
        callback_id: format!("worker-{turn}-{sequence}"),
        sequence,
        occurred_at_ms: io::now_ms() - 4_000,
        agent_id: f.agent,
        launch_id: "worker".into(),
        provider_session_id: Some("worker-session".into()),
        provider_turn_id: Some(turn.into()),
        provider_prompt_id: Some(turn.into()),
        prompt_payload: None,
        kind,
    }
}

fn autonomous(f: &mut Fixture, turn: &str, text: &str) {
    let cb = callback(f, turn, 10, CallbackEventKind::Final { text: text.into() });
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    f.worker.save(state).unwrap();
}

fn collect(f: &mut Fixture) {
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
}

fn running(f: &mut Fixture, author: Author) -> RequestId {
    ready_worker(f);
    let id = f.send(author, "open work");
    let mut state = f.worker.state.clone();
    state.begin_submission(id, "worker", 0).unwrap();
    state
        .record_submission(
            id,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    let mut cb = callback(f, "owned", 1, CallbackEventKind::PromptStarted);
    cb.prompt_payload = Some("open work".into());
    assert_eq!(
        state.accept_callback(cb),
        CallbackDisposition::AcceptedBinding
    );
    state
        .observe_status(f.agent, RuntimeStatus::Working, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    id
}

#[test]
fn autonomous_background_final_reaches_orchestrator_without_a_request() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    autonomous(&mut f, "background", "Background build completed.");
    collect(&mut f);
    assert_eq!(
        f.notices().len(),
        1,
        "autonomous final must wake the orchestrator"
    );
    let text = &f.calls.lock().unwrap()[0];
    assert!(text.contains("builder") && text.contains("Turn final") && text.contains("message"));
    assert!(text.contains("Background build completed."));
}

#[test]
fn reply_and_autonomous_final_share_one_short_worker_burst() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    let orch = f.orch;
    let source = f.send(Author::Agent(orch), "build this");
    f.finish(source, "Requested build complete.");
    autonomous(&mut f, "background", "Independent tests complete.");
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("Requested build complete."));
    assert!(
        calls[0].contains("Independent tests complete."),
        "all events in the burst must be delivered"
    );
}

#[test]
fn abandoned_request_reports_reason_including_developer_authored_requests() {
    for developer in [false, true] {
        let mut f = Fixture::new();
        let author = if developer {
            Author::Developer
        } else {
            Author::Agent(f.orch)
        };
        let source = f.send(author, "unanswered work");
        let mut state = f.worker.state.clone();
        state
            .recover_idle_request(source, io::now_ms() - 4_000)
            .unwrap();
        f.worker.save(state).unwrap();
        collect(&mut f);
        assert_eq!(
            f.notices().len(),
            1,
            "no-reply outcomes must not sit unread"
        );
        let text = &f.calls.lock().unwrap()[0];
        assert!(text.contains("abandoned") && text.contains("message"));
    }
}

#[test]
fn reply_not_captured_then_late_final_keeps_the_actual_result() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let now = io::now_ms();
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(source)
    );
    f.worker.save(state).unwrap();
    autonomous(&mut f, "owned", "The real late result.");
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("reply_not_captured"));
    assert!(calls[0].contains("The real late result."));
    assert!(calls[0].contains("Late reply"));
}

fn capped_background_request(f: &mut Fixture) -> RequestId {
    let orch = f.orch;
    let source = running(f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    assert_eq!(
        state.accept_callback(callback(
            f,
            "owned",
            2,
            CallbackEventKind::BackgroundPending
        )),
        CallbackDisposition::AcceptedProgress
    );
    state.hold_background_reply(
        f.agent,
        crate::messaging::model::PendingFinal {
            callback_id: "interim".into(),
            text: "Waiting for CI and background reviewers.".into(),
            received_at_ms: io::now_ms() - 4_000,
            provider_session_id: Some("worker-session".into()),
            provider_turn_id: Some("owned".into()),
        },
    );
    let since = state
        .request(source)
        .unwrap()
        .shell_hold
        .as_ref()
        .unwrap()
        .since_ms;
    assert_eq!(
        state.release_shell_hold(f.agent, since + 900_000, 900_000),
        Some(source)
    );
    // The async sender already printed/acknowledged the interim and exited.
    state
        .see_worker_reply(source, f.orch, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    source
}

#[test]
fn background_timeout_or_codex_stop_late_same_turn_final_wakes_sender_after_restart() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::with_provider(provider);
        let source = if provider == Provider::ClaudeCode {
            capped_background_request(&mut f)
        } else {
            // Codex Stop has no task census: its initial Stop completes normally.
            let orch = f.orch;
            let source = running(&mut f, Author::Agent(orch));
            let mut state = f.worker.state.clone();
            state
                .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms())
                .unwrap();
            state.accept_callback(callback(
                &f,
                "owned",
                2,
                CallbackEventKind::Final {
                    text: "Waiting for CI and background reviewers.".into(),
                },
            ));
            state
                .see_worker_reply(source, f.orch, io::now_ms())
                .unwrap();
            f.worker.save(state).unwrap();
            source
        };
        f.reopen();
        let next = f.send(Author::Agent(f.orch), "new unbound work");
        let mut state = f.worker.state.clone();
        // The old Stop was spooled before Bus typed the newer request. Its
        // sequence must be checked against its own source, not this boundary.
        state.begin_submission(next, "worker", 100).unwrap();
        state
            .record_submission(
                next,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("worker-session".into()),
                    provider_turn_id: None,
                },
            )
            .unwrap();
        f.worker.save(state).unwrap();
        let spool = f.dir.join("callbacks/worker");
        callbacks::initialize(
            &spool,
            &callbacks::Manifest {
                routing_key: callbacks::RoutingKey(f.agent.0),
                provider,
                launch_id: "worker".into(),
            },
        )
        .unwrap();
        callbacks::append(
            &spool,
            "worker",
            provider,
            json!({
                "hook_event_name":"Stop", "session_id":"worker-session",
                "prompt_id":"owned", "turn_id":"owned",
                "transcript_path":f.dir.join("rollout.jsonl"),
                "last_assistant_message":"CI passed; the real final result.",
                "background_tasks":[], "session_crons":[]
            }),
            &[],
            false,
        )
        .unwrap();
        f.worker.consume_callbacks(f.agent, &spool).unwrap();
        collect(&mut f);
        let calls = f.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "{provider:?}: late final must wake the original sender"
        );
        assert!(calls[0].contains("Late reply to message"));
        assert!(calls[0].contains("CI passed; the real final result."));
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(next)
        );
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .actionable_error
            .is_none());
        assert!(f
            .worker
            .state
            .request(next)
            .unwrap()
            .pending_final
            .is_none());
        assert_eq!(
            f.worker
                .state
                .request(source)
                .unwrap()
                .failure_reason
                .as_deref(),
            if provider == Provider::ClaudeCode {
                Some("background_shells_timeout")
            } else {
                None
            }
        );
        assert_eq!(
            f.worker
                .state
                .request(source)
                .unwrap()
                .pending_final
                .as_ref()
                .unwrap()
                .text,
            "Waiting for CI and background reviewers."
        );
    }
}

#[test]
fn background_timeout_does_not_spend_subsequent_changed_same_turn_finals() {
    let mut f = Fixture::new();
    capped_background_request(&mut f);
    // A shell-only Stop after the cap can itself still be an interim report.
    // Its notification must not consume the later real final of that turn.
    autonomous(&mut f, "owned", "Still waiting for the last reviewer.");
    let mut state = f.worker.state.clone();
    state.accept_callback(callback(
        &f,
        "owned",
        11,
        CallbackEventKind::Final {
            text: "All reviewers and CI passed.".into(),
        },
    ));
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("Still waiting for the last reviewer."));
    assert!(calls[0].contains("All reviewers and CI passed."));
}

#[test]
fn background_timeout_late_notifications_deduplicate_text_and_preserve_identity() {
    let mut f = Fixture::new();
    capped_background_request(&mut f);
    let mut state = f.worker.state.clone();
    for (sequence, text) in [
        (0, "STALE RESULT"),
        (10, "Waiting for CI and background reviewers."),
        (11, "Real final."),
        (12, "Real final."),
    ] {
        state.accept_callback(callback(
            &f,
            "owned",
            sequence,
            CallbackEventKind::Final { text: text.into() },
        ));
    }
    for foreign_launch in [false, true] {
        let mut cb = callback(
            &f,
            "owned",
            13 + u64::from(foreign_launch),
            CallbackEventKind::Final {
                text: "FOREIGN RESULT".into(),
            },
        );
        if foreign_launch {
            cb.launch_id = "other-launch".into();
        } else {
            cb.provider_session_id = Some("other-session".into());
        }
        state.accept_callback(cb);
    }
    f.worker.save(state).unwrap();
    f.reopen();
    // A new envelope for the same final after restart must remain deduplicated.
    let mut state = f.worker.state.clone();
    state.accept_callback(callback(
        &f,
        "owned",
        15,
        CallbackEventKind::Final {
            text: "Real final.".into(),
        },
    ));
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].matches("Real final.").count(), 1);
    assert!(!calls[0].contains("Waiting for CI"));
    assert!(!calls[0].contains("FOREIGN RESULT"));
    assert!(!calls[0].contains("STALE RESULT"));
}

#[test]
fn persistent_idle_background_hold_with_open_orchestrator_request_notifies_once() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let cb = callback(&f, "owned", 2, CallbackEventKind::BackgroundPending);
    let mut state = f.worker.state.clone();
    assert_eq!(
        state.accept_callback(cb),
        CallbackDisposition::AcceptedProgress
    );
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 8_000)
        .unwrap();
    f.worker.save(state).unwrap();
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Idle with open request"));
    assert_eq!(
        f.worker.state.request(source).unwrap().phase,
        RequestPhase::Active
    );
    f.reopen();
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

struct ExitProbe {
    calls: Arc<Mutex<Vec<String>>>,
    exited: bool,
}
impl Transport for ExitProbe {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => Ok(ResponseResult::AgentList { agents: vec![
                serde_json::from_value(json!({"terminal_id":"terminal","pane_id":"pane","agent":"claude","interactive_ready":true,"agent_status":"idle","agent_session":{"source":"herdr:claude","agent":"claude","kind":"id","value":"session"},"workspace_id":"w","tab_id":"t","focused":false,"revision":1})).unwrap()
            ] }),
            Method::PaneGet(p) => Ok(ResponseResult::PaneInfo { pane: serde_json::from_value(json!({
                "pane_id":p.pane_id,"terminal_id":"worker-terminal","workspace_id":"w","tab_id":"t","focused":false,"agent_status":"unknown","revision":1,"agent_process_exited":self.exited
            })).unwrap() }),
            // An unreadable pane still reports the exit, without a snapshot.
            Method::PaneRead(_) => Err(TransportError {
                code: Some("pane_not_found".into()),
                message: "pane not found".into(),
                definitely_rejected: true,
            }),
            other => Accepting(self.calls.clone()).request(other),
        }
    }
}

#[test]
fn confirmed_worker_exit_with_open_request_wakes_but_unavailability_does_not() {
    for exited in [false, true] {
        let mut f = Fixture::new();
        let orch = f.orch;
        running(&mut f, Author::Agent(orch));
        f.worker.transport = Box::new(ExitProbe {
            calls: f.calls.clone(),
            exited,
        });
        f.worker.poll().unwrap();
        collect(&mut f);
        assert_eq!(f.notices().len(), usize::from(exited));
        if exited {
            let notice = &f.calls.lock().unwrap()[0];
            assert!(notice.contains("Exited with open request"), "{notice}");
            assert!(notice.contains("Exit snapshot unavailable"), "{notice}");
        }
    }
}

#[test]
fn pending_autonomous_burst_survives_restart_and_duplicate_turn_callbacks() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    autonomous(&mut f, "background", "Durable result.");
    f.reopen();
    autonomous(&mut f, "background", "Durable result.");
    collect(&mut f);
    f.reopen();
    let cb = callback(
        &f,
        "background",
        11,
        CallbackEventKind::Final {
            text: "Durable result.".into(),
        },
    );
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    state.accept_callback(callback(
        &f,
        "background",
        12,
        CallbackEventKind::Final {
            text: "Changed text on the same autonomous turn.".into(),
        },
    ));
    f.worker.save(state).unwrap();
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert!(!f
        .worker
        .state
        .notices(f.room)
        .any(|p| p.text == "Changed text on the same autonomous turn."));
}

#[test]
fn stalled_request_notifies_with_its_reason() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = f.send(Author::Agent(orch), "not submitted");
    let mut state = f.worker.state.clone();
    let later = io::now_ms() + crate::messaging::model::QUEUED_STALL_MS * 2;
    assert!(state
        .expire_stalled_queued_requests(later)
        .unwrap()
        .contains(&source));
    state
        .route_worker_replies(later + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("hook_setup_unconfirmed"));
}

#[test]
fn printed_reply_removes_only_its_component_of_a_queued_mixed_batch() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    let orch = f.orch;
    let source = f.send(Author::Agent(orch), "build");
    f.finish(source, "Printed reply.");
    autonomous(&mut f, "background", "Unprinted autonomous result.");
    let mut state = f.worker.state.clone();
    state
        .observe_status(orch, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    state
        .route_worker_replies(io::now_ms() + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":orch.0.to_string()})
        )
        .ok
    );
    let mut state = f.worker.state.clone();
    state
        .observe_status(orch, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "Seen must preserve the unprinted components"
    );
    assert!(calls[0].contains("Unprinted autonomous result."));
    assert!(!calls[0].contains("Printed reply."));
}

#[test]
fn author_waiter_printed_outcome_is_seen_once_across_restart() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = f.send(Author::Agent(orch), "unanswered");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let mut state = f.worker.state.clone();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    let status = f.call(
        "message.status",
        json!({"message":message.0.to_string(),"as":orch.0.to_string()}),
    );
    assert!(status.ok);
    let keys = status.result["requests"][0]["event_acks"].clone();
    assert_eq!(keys.as_array().unwrap().len(), 1);
    assert!(status.result["requests"][0]["events"][0]["text"]
        .as_str()
        .unwrap()
        .contains("abandoned"));
    f.worker.submit_ready().unwrap();
    assert!(
        f.notices().is_empty(),
        "live author waiter defers its own outcome"
    );
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":orch.0.to_string(),"events":keys})
        )
        .ok
    );
    f.reopen();
    collect(&mut f);
    assert!(f.notices().is_empty());
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "transport-seen must still allow one action digest"
    );
    assert!(calls[0].starts_with("Room digest:"));
    assert!(calls[0].contains("abandoned"));
}

#[test]
fn killed_outcome_waiter_expires_its_lease_without_losing_the_wake() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = f.send(Author::Agent(orch), "unanswered");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let mut state = f.worker.state.clone();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":orch.0.to_string()})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
    f.worker.reply_waiters.insert(
        message,
        std::time::Instant::now() - std::time::Duration::from_secs(4),
    );
    f.worker.submit_ready().unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[test]
fn seen_idle_key_cannot_hide_an_outcome_created_after_the_print() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let cb = callback(&f, "owned", 2, CallbackEventKind::BackgroundPending);
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 8_000)
        .unwrap();
    f.worker.save(state).unwrap();
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let status = f.call("message.status", json!({"message":message.0.to_string()}));
    let keys = status.result["requests"][0]["event_acks"].clone();
    assert_eq!(keys.as_array().unwrap().len(), 1);
    let mut state = f.worker.state.clone();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":orch.0.to_string(),"events":keys})
        )
        .ok
    );
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("abandoned"));
    assert!(!f.calls.lock().unwrap()[0].contains("Idle with open request"));
}

#[test]
fn late_stop_inside_settlement_grace_yields_one_reply_wake() {
    let mut f = Fixture::new();
    let orch = f.orch;
    running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 2_000)
        .unwrap();
    state.retain_worker_outcomes(io::now_ms());
    let cb = callback(
        &f,
        "owned",
        2,
        CallbackEventKind::Final {
            text: "Normal late Stop.".into(),
        },
    );
    assert_eq!(
        state.accept_callback(cb),
        CallbackDisposition::AcceptedCompleted
    );
    f.worker.save(state).unwrap();
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Normal late Stop."));
    assert!(!f.calls.lock().unwrap()[0].contains("Idle with open request"));
}

#[test]
fn long_autonomous_text_is_pinned_until_its_queued_pointer_delivers() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    let full = "完整结果".repeat(2_000);
    let mut cb = callback(
        &f,
        "old-background",
        10,
        CallbackEventKind::Final { text: full.clone() },
    );
    cb.occurred_at_ms = io::now_ms() - crate::messaging::model::HISTORY_RETENTION_MS * 2;
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.reopen();
    assert!(f.worker.state.notices(f.room).any(|p| p.text == full));
    collect(&mut f);
    let text = &f.calls.lock().unwrap()[0];
    assert!(text.contains("bus history --room"));
    assert!(text.chars().count() < 1_000);
    assert!(f.worker.state.notices(f.room).any(|p| p.text == full));
}

#[test]
fn developer_owned_late_final_and_foreign_sessions_do_not_become_worker_reports() {
    let mut f = Fixture::new();
    let source = running(&mut f, Author::Developer);
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 8_000)
        .unwrap();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    autonomous(&mut f, "owned", "Developer private late result.");
    let mut cb = callback(
        &f,
        "foreign",
        12,
        CallbackEventKind::Final {
            text: "Foreign result.".into(),
        },
    );
    cb.provider_session_id = Some("foreign-session".into());
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    f.worker.save(state).unwrap();
    collect(&mut f);
    assert_eq!(
        f.notices().len(),
        1,
        "only the request's no-reply outcome is in scope"
    );
    let text = &f.calls.lock().unwrap()[0];
    assert!(!text.contains("Developer private late result."));
    assert!(!text.contains("Foreign result."));
    assert!(!f
        .worker
        .state
        .notices(f.room)
        .any(|p| p.text.contains("private late")));
}

#[test]
fn worker_authored_request_final_also_reaches_the_orchestrator() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let peer = state
        .create_agent(f.room, "peer", Provider::ClaudeCode, f.dir.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    let source = f.send(Author::Agent(peer), "peer request");
    f.finish(source, "Peer task finished.");
    collect(&mut f);
    assert_eq!(
        f.notices().len(),
        1,
        "a non-developer turn the orchestrator did not start must remain visible"
    );
    assert!(f.calls.lock().unwrap()[0].contains("Peer task finished."));
}

#[test]
fn provider_error_does_not_spend_the_later_real_final() {
    let mut f = Fixture::new();
    let orch = f.orch;
    running(&mut f, Author::Agent(orch));
    let cb = callback(
        &f,
        "owned",
        2,
        CallbackEventKind::Error {
            message: "Connection ended".into(),
            provider_error: Some("connection_lost".into()),
        },
    );
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    f.worker.save(state).unwrap();
    autonomous(&mut f, "owned", "Recovered final after error.");
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("provider_error"));
    assert!(f.calls.lock().unwrap()[0].contains("Recovered final after error."));
}

#[test]
fn late_final_for_old_request_does_not_attach_to_new_active_work() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 4_000)
        .unwrap();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    let next = state
        .submit_message_from(
            f.room,
            Draft {
                text: "next work".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(orch),
            io::now_ms(),
        )
        .unwrap()[0];
    state.begin_submission(next, "worker", 2).unwrap();
    state
        .record_submission(
            next,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    let mut started = callback(&f, "new-turn", 3, CallbackEventKind::PromptStarted);
    started.prompt_payload = Some("next work".into());
    assert_eq!(
        state.accept_callback(started),
        CallbackDisposition::AcceptedBinding
    );
    state
        .observe_status(f.agent, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    let late = callback(
        &f,
        "owned",
        10,
        CallbackEventKind::Final {
            text: "Original request result.".into(),
        },
    );
    assert_eq!(
        state.accept_callback(late),
        CallbackDisposition::Rejected(CallbackRejection::WrongTurn)
    );
    f.worker.save(state).unwrap();
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Original request result."));
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(next)
    );
    assert!(f
        .worker
        .state
        .request(next)
        .unwrap()
        .pending_final
        .is_none());
}

#[test]
fn distinct_turn_finals_join_a_still_queued_worker_batch() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.orch, RuntimeStatus::Unavailable, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    autonomous(&mut f, "first-own", "First independent result.");
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    autonomous(&mut f, "second-own", "Second independent result.");
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.notices().len(),
        1,
        "queued same-worker events in the window share one message"
    );
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.orch, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].contains("First independent result.")
            && calls[0].contains("Second independent result.")
    );
}

#[test]
fn late_spooled_final_is_not_deferred_behind_a_new_unbound_request() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    state.recover_idle_request(source, io::now_ms()).unwrap();
    let next = state
        .submit_message_from(
            f.room,
            Draft {
                text: "unbound next work".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(orch),
            io::now_ms(),
        )
        .unwrap()[0];
    state.begin_submission(next, "worker", 0).unwrap();
    state
        .record_submission(
            next,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let spool = f.dir.join("callbacks/worker");
    callbacks::initialize(
        &spool,
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(f.agent.0),
            provider: Provider::ClaudeCode,
            launch_id: "worker".into(),
        },
    )
    .unwrap();
    callbacks::append(&spool, "worker", Provider::ClaudeCode, json!({"hook_event_name":"Stop","session_id":"worker-session","prompt_id":"owned","last_assistant_message":"Late while next request unbound."}), &[], false).unwrap();
    f.worker
        .own_turns
        .insert(f.agent, std::time::Instant::now());
    f.worker.consume_callbacks(f.agent, &spool).unwrap();
    assert!(
        f.worker.own_turns.contains_key(&f.agent),
        "an old final cannot end the new autonomous turn guard"
    );
    assert!(f
        .worker
        .state
        .notices(f.room)
        .any(|p| p.text == "Late while next request unbound."));
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(next)
    );
    assert!(!f.worker.state.request(next).unwrap().trusted_start_bound);
}

#[test]
fn outcome_ack_does_not_mark_a_reply_arriving_after_print_seen() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let cb = callback(&f, "owned", 2, CallbackEventKind::BackgroundPending);
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 8_000)
        .unwrap();
    f.worker.save(state).unwrap();
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let printed = f.call("message.status", json!({"message":message.0.to_string()}));
    let row = &printed.result["requests"][0];
    assert_eq!(row["reply_ack"], false);
    assert_eq!(row["event_acks"].as_array().unwrap().len(), 1);
    let cb = callback(
        &f,
        "owned",
        3,
        CallbackEventKind::Final {
            text: "New reply after the output.".into(),
        },
    );
    let mut state = f.worker.state.clone();
    assert_eq!(
        state.accept_callback(cb),
        CallbackDisposition::AcceptedCompleted
    );
    f.worker.save(state).unwrap();
    assert!(f.call("message.seen", json!({"request":source.0.to_string(),"as":orch.0.to_string(),"events":row["event_acks"]})).ok);
    f.reopen();
    collect(&mut f);
    assert_eq!(
        f.notices().len(),
        1,
        "an old outcome acknowledgement cannot consume a new reply"
    );
    let calls = f.calls.lock().unwrap();
    assert!(calls[0].contains("New reply after the output."));
    assert!(!calls[0].contains("Idle with open request"));
}

#[test]
fn long_failure_reason_keeps_event_and_original_reference_in_the_short_notice() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let reason = "unavailable detail ".repeat(400);
    let cb = callback(
        &f,
        "owned",
        2,
        CallbackEventKind::Error {
            message: "provider failed".into(),
            provider_error: Some(reason.clone()),
        },
    );
    let mut state = f.worker.state.clone();
    state.accept_callback(cb);
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("No reply"));
    assert!(calls[0].contains(&format!("message {}", message.0)));
    assert!(calls[0].contains("bus message status"));
    assert!(calls[0].chars().count() < 1_000);
    assert!(f
        .worker
        .state
        .request(source)
        .unwrap()
        .failure_reason
        .as_ref()
        .unwrap()
        .contains(&reason));
}

fn earlier_background_request(f: &mut Fixture, shell: bool) -> RequestId {
    earlier_background_request_at(f, shell, io::now_ms())
}

fn earlier_background_request_at(f: &mut Fixture, shell: bool, now: u64) -> RequestId {
    ready_worker_at(f, now - 30_000);
    let mut state = f.worker.state.clone();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "background work".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            now - 20_000,
        )
        .unwrap()[0];
    state.begin_submission(source, "worker", 0).unwrap();
    state
        .record_submission(
            source,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    let mut start = callback(f, "held", 1, CallbackEventKind::PromptStarted);
    start.occurred_at_ms = now - 12_000;
    start.prompt_payload = Some("background work".into());
    assert_eq!(
        state.accept_callback(start),
        CallbackDisposition::AcceptedBinding
    );
    state
        .observe_status(f.agent, RuntimeStatus::Working, now - 12_000)
        .unwrap();
    let mut pending = callback(f, "held", 2, CallbackEventKind::BackgroundPending);
    pending.occurred_at_ms = now - 11_000;
    state.accept_callback(pending);
    if shell {
        state.hold_background_reply(
            f.agent,
            crate::messaging::model::PendingFinal {
                callback_id: "held-interim".into(),
                text: "Shell still running".into(),
                received_at_ms: now - 11_000,
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: Some("held".into()),
            },
        );
    }
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 10_000)
        .unwrap();
    f.worker.save(state).unwrap();
    source
}

#[test]
fn idle_open_work_wakes_even_while_its_author_waiter_is_alive() {
    let mut f = Fixture::new();
    let source = earlier_background_request(&mut f, false);
    let message = f.worker.state.request(source).unwrap().prompt.id;
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    assert!(f.worker.reply_waiters.contains_key(&message));
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.calls.lock().unwrap().len(),
        1,
        "a waiting process that has printed nothing cannot replace idle awareness"
    );
    assert!(f.calls.lock().unwrap()[0].contains("Idle with open request"));
    assert_eq!(
        f.worker.state.request(source).unwrap().phase,
        RequestPhase::Active
    );
}

#[test]
fn known_background_shell_idle_is_reported_without_releasing_its_hold() {
    let mut f = Fixture::new();
    let source = earlier_background_request(&mut f, true);
    f.worker.submit_ready().unwrap();
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Idle with open request"));
    assert!(f.calls.lock().unwrap()[0].contains("background work"));
    let request = f.worker.state.request(source).unwrap();
    assert_eq!(request.phase, RequestPhase::Active);
    assert!(request.awaiting_background && request.shell_hold.is_some());
}

#[test]
fn genuine_idle_edge_reports_an_orchestrator_request_still_queued() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    ready_worker_at(&mut f, now - 30_000);
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Working, now - 25_000)
        .unwrap();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "still queued".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            now - 20_000,
        )
        .unwrap()[0];
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 10_000)
        .unwrap();
    state.route_worker_replies(now, &BTreeSet::new()).unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(
        f.notices().len(),
        1,
        "a queued request is still open when its worker goes idle"
    );
    assert_eq!(
        f.worker.state.request(source).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(f.notices()[0]
        .prompt
        .text
        .contains("Idle with open request"));
}

#[test]
fn idle_after_uncertain_submission_reports_without_claiming_the_turn_ended() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    ready_worker_at(&mut f, now - 30_000);
    let mut state = f.worker.state.clone();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "unconfirmed work".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            now - 20_000,
        )
        .unwrap()[0];
    state.begin_submission(source, "worker", 0).unwrap();
    state
        .record_submission(
            source,
            SubmissionOutcome::Uncertain {
                message: "submit acknowledgement lost".into(),
            },
        )
        .unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Working, now - 12_000)
        .unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 10_000)
        .unwrap();
    assert!(!state.turn_ended(state.request(source).unwrap()));
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Idle with open request"));
    let request = f.worker.state.request(source).unwrap();
    assert_eq!(request.phase, RequestPhase::Submitting);
    assert!(request.uncertain_outcome && !request.trusted_start_bound);
}

#[test]
fn an_initially_idle_worker_with_a_new_queued_request_does_not_warn() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    ready_worker_at(&mut f, now - 30_000);
    let mut state = f.worker.state.clone();
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "new queued work".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            now - 20_000,
        )
        .unwrap();
    state.route_worker_replies(now, &BTreeSet::new()).unwrap();
    f.worker.save(state).unwrap();
    assert!(f.notices().is_empty());
}

fn continue_background_work(f: &mut Fixture, at: u64) {
    let mut state = f.worker.state.clone();
    let mut start = callback(f, "held-next", 20, CallbackEventKind::PromptStarted);
    start.occurred_at_ms = at;
    start.prompt_payload = Some("background continuation".into());
    assert_eq!(
        state.accept_callback(start),
        CallbackDisposition::AcceptedContinuation
    );
    state
        .observe_status(f.agent, RuntimeStatus::Working, at)
        .unwrap();
    let mut pending = callback(f, "held-next", 21, CallbackEventKind::BackgroundPending);
    pending.occurred_at_ms = at + 1_000;
    state.accept_callback(pending);
    state
        .observe_status(f.agent, RuntimeStatus::Idle, at + 2_000)
        .unwrap();
    f.worker.save(state).unwrap();
}

#[test]
fn a_new_background_idle_episode_wakes_once_after_the_first_notice_was_handled() {
    let mut f = Fixture::new();
    earlier_background_request(&mut f, false);
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let first = f.notices()[0].id;
    f.finish(first, "acknowledged");
    continue_background_work(&mut f, io::now_ms() - 8_000);
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.notices().len(),
        2,
        "new work followed by idle is a new event"
    );
    let second = f.notices().into_iter().find(|r| r.id != first).unwrap().id;
    f.finish(second, "acknowledged again");
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.notices().len(),
        2,
        "stable idle must not replay on restart"
    );
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|text| !text.starts_with("Room digest:"))
            .count(),
        2
    );
    assert_eq!(
        calls
            .iter()
            .filter(|text| text.starts_with("Room digest:"))
            .count(),
        1,
        "replying to notices is not a next action, so one digest is still required"
    );
}

#[test]
fn an_acknowledged_stall_does_not_hide_a_new_stall_after_work_resumes() {
    let mut f = Fixture::new();
    let source = earlier_background_request(&mut f, false);
    let first_at = io::now_ms() + crate::messaging::model::DELIVERED_STALL_MS;
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(first_at);
    let old: Vec<_> = state
        .worker_outcomes_for(state.request(source).unwrap())
        .into_iter()
        .filter(|(_, text)| text.contains("No reply (stalled:"))
        .map(|(key, _)| key)
        .collect();
    assert_eq!(old.len(), 1);
    state
        .see_worker_outcomes(source, f.orch, &old, first_at)
        .unwrap();
    f.worker.save(state).unwrap();
    continue_background_work(&mut f, first_at + 1_000);
    let mut state = f.worker.state.clone();
    let second_at = first_at + crate::messaging::model::DELIVERED_STALL_MS + 4_000;
    state.retain_worker_outcomes(second_at);
    state
        .see_worker_outcomes(source, f.orch, &old, second_at)
        .unwrap();
    let current: Vec<_> = state
        .worker_outcomes_for(state.request(source).unwrap())
        .into_iter()
        .filter(|(_, text)| text.contains("No reply (stalled:"))
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        current.len(),
        1,
        "a stale printed status must not hide a later stalled episode"
    );
    assert_ne!(old, current);
    assert_eq!(state.request(source).unwrap().phase, RequestPhase::Active);
}

#[test]
fn handled_old_idle_episodes_expire_only_after_new_work_supersedes_them() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    let old = now - crate::messaging::model::HISTORY_RETENTION_MS - 30_000;
    let source = earlier_background_request_at(&mut f, false, old);
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(old);
    let keys: Vec<_> = state
        .worker_outcomes_for(state.request(source).unwrap())
        .into_iter()
        .filter(|(_, text)| text.contains("Idle with open request"))
        .map(|(key, _)| key)
        .collect();
    assert_eq!(keys.len(), 1);
    state
        .see_worker_outcomes(source, f.orch, &keys, old)
        .unwrap();
    state.archive(now);
    assert!(
        state
            .worker_events_for(state.request(source).unwrap())
            .iter()
            .any(|(key, _)| keys.contains(key)),
        "a current idle tombstone must prevent rediscovery even on a long-running request"
    );
    state
        .observe_status(f.agent, RuntimeStatus::Working, now - 8_000)
        .unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 6_000)
        .unwrap();
    state.archive(now);
    let current = state.worker_events_for(state.request(source).unwrap());
    assert!(
        !current.iter().any(|(key, _)| keys.contains(key)),
        "handled superseded history is bounded"
    );
    assert_eq!(
        current
            .iter()
            .filter(|(_, text)| text.contains("Idle with open request"))
            .count(),
        1
    );
    state.route_worker_replies(now, &BTreeSet::new()).unwrap();
    f.worker.save(state).unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[path = "worker_event_waiter_overlap_test.rs"]
mod waiter_overlap;

fn late_final_after_new_input_joins_the_completed_turn(provider: Provider) {
    let mut f = Fixture::with_provider(provider);
    let source = if provider == Provider::ClaudeCode {
        capped_background_request(&mut f)
    } else {
        let orch = f.orch;
        let source = running(&mut f, Author::Agent(orch));
        let mut state = f.worker.state.clone();
        state
            .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        state.accept_callback(callback(
            &f,
            "owned",
            2,
            CallbackEventKind::Final {
                text: "Interim result.".into(),
            },
        ));
        state
            .see_worker_reply(source, f.orch, io::now_ms())
            .unwrap();
        f.worker.save(state).unwrap();
        source
    };
    let next = f.send(Author::Agent(f.orch), "continue the background task");
    let mut state = f.worker.state.clone();
    state.begin_submission(next, "worker", 10).unwrap();
    state
        .record_submission(
            next,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    // Both providers can fold newly typed input into the still-running turn.
    let mut started = callback(&f, "owned", 11, CallbackEventKind::PromptStarted);
    started.prompt_payload = Some("continue the background task".into());
    assert_eq!(
        state.accept_callback(started),
        CallbackDisposition::AcceptedBinding
    );
    let disposition = state.accept_callback(callback(
        &f,
        "owned",
        12,
        CallbackEventKind::Final {
            text: "The real background result.".into(),
        },
    ));
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1,
        "{provider:?}: the original async sender must receive the late final; disposition={disposition:?}");
    assert!(calls[0].contains("The real background result."));
    let source_prompt = f.worker.state.request(source).unwrap().prompt.id;
    assert!(calls[0].contains(&format!("Late reply to message {}", source_prompt.0)));
}

#[test]
fn late_final_after_new_input_joins_the_completed_turn_claude() {
    late_final_after_new_input_joins_the_completed_turn(Provider::ClaudeCode);
}

#[test]
fn late_final_after_new_input_joins_the_completed_turn_codex() {
    late_final_after_new_input_joins_the_completed_turn(Provider::Codex);
}

#[test]
fn background_wake_on_a_new_claude_prompt_after_the_cap_notifies_the_sender() {
    let mut f = Fixture::new();
    let source = capped_background_request(&mut f);
    // Claude wakes for a finished background shell with a task notification,
    // which can open a turn under a new prompt_id rather than the capped one.
    let mut state = f.worker.state.clone();
    let mut wake = callback(&f, "wake", 20, CallbackEventKind::PromptStarted);
    wake.prompt_payload =
        Some("<task-notification>\nCI watcher finished\n</task-notification>".into());
    state.accept_callback(wake);
    state.accept_callback(callback(
        &f,
        "wake",
        21,
        CallbackEventKind::Final {
            text: "CI passed; the real final result.".into(),
        },
    ));
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "the sender must be woken once: {calls:?}");
    assert!(calls[0].contains("CI passed; the real final result."));
    let prompt = f.worker.state.request(source).unwrap().prompt.id;
    assert!(calls[0].contains("Turn final"));
    assert!(
        !calls[0].contains(&format!("Late reply to message {}", prompt.0)),
        "a fresh turn has no proof tying it to the capped message: {}",
        calls[0]
    );
}

#[test]
fn late_final_reaches_orchestrator_message_steered_into_an_abandoned_developer_turn() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    let lead = f.send(Author::Developer, "developer work");
    let mut state = f.worker.state.clone();
    state.begin_submission(lead, "worker", 0).unwrap();
    state
        .record_submission(
            lead,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    let mut started = callback(&f, "owned", 1, CallbackEventKind::PromptStarted);
    started.prompt_payload = Some("developer work".into());
    assert_eq!(
        state.accept_callback(started),
        CallbackDisposition::AcceptedBinding
    );
    state
        .observe_status(f.agent, RuntimeStatus::Working, io::now_ms() - 9_000)
        .unwrap();
    f.worker.save(state).unwrap();
    let member = f.send(Author::Agent(f.orch), "orchestrator follow-up");
    let mut state = f.worker.state.clone();
    assert_eq!(state.next_steering(f.agent), Some((member, lead)));
    state.begin_steering(member, lead).unwrap();
    state
        .record_steering(
            member,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: Some("owned".into()),
            },
        )
        .unwrap();
    let mut steered = callback(&f, "owned", 2, CallbackEventKind::PromptStarted);
    steered.prompt_payload = Some(state.request(member).unwrap().prompt.rendered_payload());
    assert_eq!(
        state.accept_callback(steered),
        CallbackDisposition::AcceptedSteering
    );
    let now = io::now_ms();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(lead)
    );
    assert_eq!(
        state.request(member).unwrap().phase,
        RequestPhase::Abandoned
    );
    f.worker.save(state).unwrap();
    autonomous(&mut f, "owned", "The real late result.");
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|text| text.contains("Late reply") && text.contains("The real late result.")),
        "the orchestrator's steered message owes the late final: {calls:?}"
    );
}

fn replayed_autonomous_final_after_later_abandonment(provider: Provider) {
    let mut f = Fixture::with_provider(provider);
    ready_worker(&mut f);
    autonomous(&mut f, "owned", "Earlier autonomous result.");
    collect(&mut f);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Turn final"));

    let next = f.send(Author::Agent(f.orch), "continue this turn");
    let mut state = f.worker.state.clone();
    state.begin_submission(next, "worker", 20).unwrap();
    state
        .record_submission(
            next,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    let mut started = callback(&f, "owned", 21, CallbackEventKind::PromptStarted);
    started.prompt_payload = Some("continue this turn".into());
    assert_eq!(
        state.accept_callback(started),
        CallbackDisposition::AcceptedBinding
    );
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    state.recover_idle_request(next, io::now_ms()).unwrap();
    // A fresh envelope replays the identical Stop already delivered before
    // this request existed. A new attribution must not bypass its dedup key.
    state.accept_callback(callback(
        &f,
        "owned",
        22,
        CallbackEventKind::Final {
            text: "Earlier autonomous result.".into(),
        },
    ));
    f.worker.save(state).unwrap();
    f.reopen();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        f.worker
            .state
            .notices(f.room)
            .filter(|prompt| prompt.text == "Earlier autonomous result.")
            .count(),
        1,
        "{provider:?}: replayed final must not be delivered again as a late reply to request {next:?}: {calls:?}"
    );
    let notices = f.notices();
    let result_notices: Vec<_> = notices
        .iter()
        .filter(|request| request.prompt.text.contains("Earlier autonomous result."))
        .collect();
    assert_eq!(result_notices.len(), 1, "{provider:?}: {notices:?}");
    assert!(result_notices[0].prompt.text.contains("Turn final"));
    assert!(!result_notices[0].prompt.text.contains("Late reply"));
}

#[test]
fn replayed_autonomous_final_after_later_abandonment_claude() {
    replayed_autonomous_final_after_later_abandonment(Provider::ClaudeCode);
}

#[test]
fn replayed_autonomous_final_after_later_abandonment_codex() {
    replayed_autonomous_final_after_later_abandonment(Provider::Codex);
}
