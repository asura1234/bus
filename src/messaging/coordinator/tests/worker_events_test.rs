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

fn report(f: &mut Fixture, text: &str) {
    let mut state = f.worker.state.clone();
    state
        .submit_message_from(
            f.room,
            Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            io::now_ms() - 4_000,
        )
        .unwrap();
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
fn abandoned_outcome_notice_has_no_deliverable_header() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "unanswered work");
    let mut state = f.worker.state.clone();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    collect(&mut f);
    let text = &f.calls.lock().unwrap()[0];
    assert!(text.contains("No reply (abandoned:"), "{text}");
    assert!(
        !text.contains("[Bus message "),
        "a no-reply outcome must not look deliverable: {text}"
    );
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
    assert!(calls[0].contains("The real late result."));
    assert!(calls[0].contains("Late reply"));
    assert!(
        !calls[0].contains("reply_not_captured"),
        "a result in the same minute is the only notice: {calls:?}"
    );
}

fn answered_background_request(f: &mut Fixture) -> RequestId {
    let orch = f.orch;
    let source = running(f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    assert_eq!(
        state.accept_callback(callback(
            f,
            "owned",
            2,
            CallbackEventKind::Final {
                text: "Waiting for CI and background reviewers.".into(),
            }
        )),
        CallbackDisposition::AcceptedCompleted
    );
    // The async sender already printed/acknowledged the interim and exited.
    state
        .see_worker_reply(source, f.orch, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    source
}

#[test]
fn captured_stop_late_same_turn_final_wakes_sender_after_restart() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::with_provider(provider);
        let source = if provider == Provider::ClaudeCode {
            answered_background_request(&mut f)
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
            None
        );
        assert_eq!(
            f.worker
                .state
                .request(source)
                .unwrap()
                .reply()
                .unwrap()
                .text,
            "Waiting for CI and background reviewers."
        );
    }
}

#[test]
fn background_timeout_does_not_spend_subsequent_changed_same_turn_finals() {
    let mut f = Fixture::new();
    answered_background_request(&mut f);
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
    answered_background_request(&mut f);
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
    assert!(f.calls.lock().unwrap().is_empty());
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
fn worker_authored_request_final_stays_with_the_peer() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let peer = state
        .create_agent(f.room, "peer", Provider::ClaudeCode, f.dir.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    let source = f.send(Author::Agent(peer), "peer request");
    f.finish(source, "Peer task finished.");
    collect(&mut f);
    assert!(
        f.notices().is_empty(),
        "a reply to a peer's request goes to that peer, not to the orchestrator"
    );
}

#[test]
fn interrupted_and_deleted_agent_requests_end_without_a_notice() {
    for deleted in [false, true] {
        let mut f = Fixture::new();
        let orch = f.orch;
        let source = f.send(Author::Agent(orch), "work");
        let mut state = f.worker.state.clone();
        if deleted {
            state.prepare_delete_agent(f.agent).unwrap();
            state
                .recover_idle_request(source, io::now_ms() - 4_000)
                .unwrap();
        } else {
            state.begin_interrupt(f.agent).unwrap();
            state
                .close_interrupted(f.agent, io::now_ms() - 4_000)
                .unwrap();
        }
        let r = state.request(source).unwrap();
        assert!(
            matches!(r.phase, crate::messaging::model::RequestPhase::Abandoned),
            "deleted={deleted} {r:?}"
        );
        f.worker.save(state).unwrap();
        collect(&mut f);
        assert!(f.notices().is_empty(), "deleted={deleted}");
    }
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
        state.accept_callback(callback(
            f,
            "held",
            3,
            CallbackEventKind::Final {
                text: "Shell still running".into(),
            },
        ));
    }
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 10_000)
        .unwrap();
    f.worker.save(state).unwrap();
    source
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
fn resumed_work_does_not_create_stall_outcomes() {
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
    assert!(old.is_empty());
    f.worker.save(state).unwrap();
    continue_background_work(&mut f, first_at + 1_000);
    let mut state = f.worker.state.clone();
    let second_at = first_at + crate::messaging::model::DELIVERED_STALL_MS + 4_000;
    state.retain_worker_outcomes(second_at);
    let current: Vec<_> = state
        .worker_outcomes_for(state.request(source).unwrap())
        .into_iter()
        .filter(|(_, text)| text.contains("No reply (stalled:"))
        .map(|(key, _)| key)
        .collect();
    assert!(current.is_empty());
    assert_eq!(state.request(source).unwrap().phase, RequestPhase::Active);
}

fn late_final_after_new_input_joins_the_completed_turn(provider: Provider) {
    let mut f = Fixture::with_provider(provider);
    let source = if provider == Provider::ClaudeCode {
        answered_background_request(&mut f)
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
    let source = answered_background_request(&mut f);
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

#[test]
fn explicit_send_then_same_provider_final_adds_no_second_body() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    let text = "Verdict at head abc.";
    report(&mut f, text);
    autonomous(&mut f, "background", text);
    collect(&mut f);
    let copies = f
        .worker
        .state
        .notices(f.room)
        .filter(|prompt| prompt.text.trim() == text)
        .count()
        + f.worker
            .state
            .requests()
            .filter(|request| request.prompt.text.trim() == text)
            .count();
    assert_eq!(
        copies, 1,
        "the explicit send is the only copy of the result"
    );
    let calls = f.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|call| !call.contains("Turn final") && !call.contains("Late reply")),
        "the orchestrator gets no summary wrapper: {calls:?}"
    );
}

#[test]
fn five_polls_of_the_same_stall_post_one_notice() {
    let mut f = Fixture::new();
    let source = earlier_background_request(&mut f, false);
    let at = io::now_ms() + crate::messaging::model::DELIVERED_STALL_MS;
    let mut state = f.worker.state.clone();
    for poll in 0..5 {
        state.retain_worker_outcomes(at + poll);
    }
    let stalls: Vec<_> = state
        .worker_outcomes_for(state.request(source).unwrap())
        .into_iter()
        .filter(|(_, text)| text.contains("No reply (stalled:"))
        .collect();
    assert_eq!(stalls.len(), 1, "{stalls:?}");
    state
        .route_worker_replies(at + 10_000, &BTreeSet::new())
        .unwrap();
    let notices = state
        .requests()
        .filter(|request| request.prompt.text.contains("No reply (stalled:"))
        .count();
    assert_eq!(notices, 1);
}

#[test]
fn working_agent_with_an_older_queued_request_posts_no_stall() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    ready_worker_at(&mut f, now - 30_000);
    let mut state = f.worker.state.clone();
    let queued = state
        .submit_message_from(
            f.room,
            Draft {
                text: "older queued work".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            now - crate::messaging::model::QUEUED_STALL_MS - 10_000,
        )
        .unwrap()[0];
    state
        .observe_status(f.agent, RuntimeStatus::Working, now - 1_000)
        .unwrap();
    state.retain_worker_outcomes(now);
    let outcomes = state.worker_outcomes_for(state.request(queued).unwrap());
    assert!(
        outcomes.iter().all(|(_, text)| !text.contains("No reply")),
        "a Working agent posts no stall for an older queued request: {outcomes:?}"
    );
}

#[test]
fn a_stall_and_a_later_abandon_post_one_outcome() {
    let mut f = Fixture::new();
    let source = earlier_background_request(&mut f, false);
    let at = io::now_ms() + crate::messaging::model::DELIVERED_STALL_MS;
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(at);
    state.recover_idle_request(source, at + 1_000).unwrap();
    state.retain_worker_outcomes(at + 1_000);
    let outcomes = state.worker_outcomes_for(state.request(source).unwrap());
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(outcomes[0].1.contains("No reply (stalled:"), "{outcomes:?}");
    assert!(
        outcomes.iter().all(|(_, text)| !text.contains("abandoned")),
        "{outcomes:?}"
    );
}

#[test]
fn an_abandon_without_a_closer_does_not_render_abandoned_abandoned() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "unanswered work");
    let mut state = f.worker.state.clone();
    state
        .recover_idle_request(source, io::now_ms() - 4_000)
        .unwrap();
    f.worker.save(state).unwrap();
    collect(&mut f);
    let text = f.calls.lock().unwrap()[0].clone();
    assert!(text.contains("No reply (abandoned:"), "{text}");
    assert!(!text.contains("abandoned: abandoned"), "{text}");
    assert!(text.contains("abandoned: idle"), "{text}");
}

#[test]
fn unrelated_same_minute_text_does_not_hide_idle_abandonment() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "unanswered work");
    let at = io::now_ms() - 4_000;
    let mut state = f.worker.state.clone();
    state.recover_idle_request(source, at).unwrap();
    f.worker.save(state).unwrap();
    report(&mut f, "The review landed.");
    collect(&mut f);
    // One submit delivers only the queue head, which is the unrelated send.
    // The abandon is the next notice, so the proof is that notice, not the call.
    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("abandoned: idle")),
        "unrelated text is not this request's result: {notices:?}"
    );
}

#[test]
fn unrelated_send_does_not_hide_reply_not_captured() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let now = io::now_ms() + 120_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(source)
    );
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Already reported the head.".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    // One submit delivers only the queue head, which is the unrelated send.
    // The no-reply is the next notice, so the proof is that notice, not the call.
    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "an unrelated send is not this request's result: {notices:?}"
    );
}

#[test]
fn a_restart_burst_posts_no_per_request_reply_not_captured() {
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
    assert_eq!(
        state.request(source).unwrap().failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert!(state
        .announce_restart("Restarted old -> new (by agent:9).", now)
        .unwrap());
    f.worker.save(state).unwrap();
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|call| call.contains("Restarted old -> new")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|call| !call.contains("reply_not_captured")),
        "{calls:?}"
    );
}

#[test]
fn same_minute_late_result_replaces_pending_reply_not_captured() {
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
    state.retain_worker_outcomes(now);
    let pending: Vec<_> = state
        .worker_outcomes_for(state.request(source).unwrap())
        .into_iter()
        .filter(|(_, text)| text.contains("reply_not_captured"))
        .collect();
    assert_eq!(
        pending.len(),
        1,
        "the quiet outcome is retained before the result arrives: {pending:?}"
    );
    f.worker.save(state).unwrap();
    autonomous(&mut f, "owned", "The real late result.");
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .any(|call| call.contains("The real late result.")),
        "the late result still posts: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|call| !call.contains("reply_not_captured")),
        "a result in the same minute replaces the pending no-reply: {calls:?}"
    );
}

#[test]
fn identical_results_for_distinct_requests_each_wake_once() {
    let mut f = Fixture::new();
    let first = f.send(Author::Agent(f.orch), "first task");
    f.finish(first, "Ready for review.");
    report(&mut f, "Ready for review.");
    collect(&mut f);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    // That send holds the orchestrator's turn, so the next prompt cannot land
    // until the turn ends. Ending it keeps the sent text in the room.
    let held = f
        .worker
        .state
        .agent(f.orch)
        .and_then(|agent| agent.current_request)
        .expect("explicit send");
    f.finish(held, "ack");

    let second = f.send(Author::Agent(f.orch), "second task");
    f.finish(second, "Ready for review.");
    collect(&mut f);
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        2,
        "the second request still owes its own outcome: {calls:?}"
    );
    assert!(calls[1].contains("Reply to message"), "{calls:?}");
}

#[test]
fn provider_error_replaces_an_earlier_stall_outcome() {
    let mut f = Fixture::new();
    let source = earlier_background_request(&mut f, false);
    let at = io::now_ms() + crate::messaging::model::DELIVERED_STALL_MS;
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(at);
    state.accept_callback(callback(
        &f,
        "held",
        4,
        CallbackEventKind::Error {
            message: "Connection ended".into(),
            provider_error: Some("connection_lost".into()),
        },
    ));
    state.retain_worker_outcomes(at + 1_000);
    let outcomes = state.worker_outcomes_for(state.request(source).unwrap());
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        outcomes[0].1.contains("provider_error"),
        "the terminal provider error must survive the earlier stall: {outcomes:?}"
    );
}

#[test]
fn progress_message_does_not_hide_reply_not_captured() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let now = io::now_ms() + 120_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(source)
    );
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Still working on the request.".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state.retain_worker_outcomes(now);
    let outcomes = state.worker_outcomes_for(state.request(source).unwrap());
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        outcomes[0].1.contains("reply_not_captured"),
        "progress is not the request's result: {outcomes:?}"
    );
}

#[test]
fn request_starting_after_restart_keeps_reply_not_captured() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    let mut state = f.worker.state.clone();
    assert!(state
        .announce_restart("Restarted old -> new (by agent:9).", now - 60_000)
        .unwrap());
    f.worker.save(state).unwrap();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let settled_at = now + 120_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, settled_at - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, settled_at, |_| None),
        Some(source)
    );
    state.retain_worker_outcomes(settled_at);
    let outcomes = state.worker_outcomes_for(state.request(source).unwrap());
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        outcomes[0].1.contains("reply_not_captured"),
        "this request did not exist during the restart: {outcomes:?}"
    );
}

#[test]
fn same_minute_progress_does_not_hide_idle_abandonment() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "unanswered work");
    let now = io::now_ms();
    let mut state = f.worker.state.clone();
    state.recover_idle_request(source, now).unwrap();
    f.worker.save(state).unwrap();
    report(&mut f, "Still working on something else.");
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(now + 10_000);
    let outcomes = state.worker_outcomes_for(state.request(source).unwrap());
    assert_eq!(outcomes.len(), 1, "{outcomes:?}");
    assert!(
        outcomes[0].1.contains("abandoned: idle"),
        "unrelated progress is not this request's result: {outcomes:?}"
    );
}

#[test]
fn explicit_send_withdraws_an_already_queued_duplicate_reply_wake() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Built and tested.");
    let at = io::now_ms() + 10_000;
    let mut state = f.worker.state.clone();
    state.route_worker_replies(at, &BTreeSet::new()).unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(f.notices().len(), 1);

    report(&mut f, "Built and tested.");
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(at + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    let calls = f.calls.lock().unwrap();
    assert!(
        calls.iter().all(|call| !call.contains("Reply to message")),
        "the explicit send must replace the queued wrapper: {calls:?}"
    );
}

#[test]
fn explicit_result_send_suppresses_reply_not_captured() {
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
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "The actual result.".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    // The owned turn's final is what ties this send to the closed request.
    autonomous(&mut f, "owned", "The actual result.");
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("The actual result.")),
        "the explicit send is the delivery: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .all(|notice| !notice.prompt.text.contains("reply_not_captured")),
        "a send tied to this request replaces the no-reply: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .all(|notice| !notice.prompt.text.contains("Late reply")),
        "the explicit copy is the only wake: {notices:?}"
    );
}

#[test]
fn identical_late_final_for_a_distinct_request_still_wakes() {
    let mut f = Fixture::new();
    ready_worker(&mut f);
    report(&mut f, "Done.");
    f.worker.submit_ready().unwrap();
    let held = f
        .worker
        .state
        .agent(f.orch)
        .and_then(|agent| agent.current_request)
        .expect("earlier explicit result");
    f.finish(held, "ack");

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
    autonomous(&mut f, "owned", "Done.");
    collect(&mut f);

    let calls = f.calls.lock().unwrap();
    assert!(
        calls.iter().any(|call| {
            call.contains("Late reply") && call.contains(&format!("request {}", source.0))
        }),
        "an earlier request's identical text must not consume this late result: {calls:?}"
    );
}

#[test]
fn later_explicit_copy_is_not_claimed_by_an_earlier_identical_reply() {
    let mut f = Fixture::new();
    let first = f.send(Author::Agent(f.orch), "first task");
    f.finish(first, "Done.");
    collect(&mut f);
    let held = f
        .worker
        .state
        .agent(f.orch)
        .and_then(|agent| agent.current_request)
        .expect("first reply wrapper");
    f.finish(held, "ack");

    let second = f.send(Author::Agent(f.orch), "second task");
    f.finish(second, "Done.");
    report(&mut f, "Done.");
    collect(&mut f);

    let notices = f.notices();
    assert!(
        notices.iter().all(|notice| {
            !notice.prompt.text.contains("Reply to message")
                || !notice
                    .prompt
                    .text
                    .contains(&format!("request {}", second.0))
        }),
        "the later request's explicit send is its only copy: {notices:?}"
    );
}

#[test]
fn explicit_send_withdraws_a_queued_long_turn_final() {
    let mut f = Fixture::new();
    let now = io::now_ms();
    let text = "A detailed review result with evidence. ".repeat(20);
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
        .observe_status(f.agent, RuntimeStatus::Idle, now - 10_000)
        .unwrap();
    state.accept_callback(ProviderCallback {
        callback_id: "long-final-before-send".into(),
        sequence: 10,
        occurred_at_ms: now,
        agent_id: f.agent,
        launch_id: "worker".into(),
        provider_session_id: Some("worker-session".into()),
        provider_turn_id: Some("background".into()),
        provider_prompt_id: Some("background".into()),
        prompt_payload: None,
        kind: CallbackEventKind::Final { text: text.clone() },
    });
    state
        .submit_message_from(
            f.room,
            Draft {
                text,
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .all(|notice| !notice.prompt.text.contains("Turn final")),
        "the explicit long result must replace its queued wrapper: {notices:?}"
    );
}

#[test]
fn post_close_unrelated_message_does_not_hide_reply_not_captured() {
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
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Still working on a different request.".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "an unrelated post-close message is not this request's result: {notices:?}"
    );
}

#[test]
fn post_close_brief_read_does_not_hide_reply_not_captured() {
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
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Brief read.".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "a brief-read note is not this request's result: {notices:?}"
    );
}

#[test]
fn post_close_nothing_changed_does_not_hide_reply_not_captured() {
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
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Nothing changed on the other request.".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "an unrelated status line is not this request's result: {notices:?}"
    );
}

#[test]
fn post_close_arbitrary_message_does_not_hide_reply_not_captured() {
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
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "The warehouse door faces the alley.".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "an arbitrary post-close message is not this request's result: {notices:?}"
    );
}

#[test]
fn explicit_answer_without_a_late_callback_suppresses_reply_not_captured() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    let message = state.request(source).unwrap().prompt.id;
    state
        .answer_message(
            message,
            f.agent,
            "The actual answer despite capture failure.".into(),
            io::now_ms(),
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let now = io::now_ms() + 120_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(source)
    );
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let answered = f.worker.state.request(source).unwrap();
    assert!(
        answered
            .reply()
            .is_some_and(|reply| reply.text.contains("The actual answer")),
        "the --reply-to answer is stored on the request"
    );
    let notices = f.notices();
    assert!(
        notices
            .iter()
            .all(|notice| !notice.prompt.text.contains("reply_not_captured")),
        "a --reply-to answer completes the request: {notices:?}"
    );
    assert_eq!(
        notices.len(),
        1,
        "the completed reply is delivered once: {notices:?}"
    );
    assert!(
        notices[0].prompt.text.contains("Reply to message")
            && notices[0].prompt.text.contains("The actual answer"),
        "the first delivery of the --reply-to answer is Reply to message: {notices:?}"
    );
}

#[test]
fn in_turn_plain_send_of_the_answer_keeps_reply_not_captured() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "The actual answer despite capture failure.".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(f.agent),
            io::now_ms(),
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let now = io::now_ms() + 120_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(source)
    );
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("The actual answer")),
        "the plain send is still delivered: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "a plain send never completes the original request: {notices:?}"
    );
}

#[test]
fn post_settlement_send_of_the_answer_keeps_reply_not_captured() {
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
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "The actual answer despite capture failure.".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(f.agent),
            now,
        )
        .unwrap();
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("The actual answer")),
        "the post-settlement send is still delivered: {notices:?}"
    );
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "a send after the turn closes is not that request's result: {notices:?}"
    );
}

#[test]
fn in_turn_progress_does_not_hide_reply_not_captured() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Still working on the request.".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(f.agent),
            io::now_ms(),
        )
        .unwrap();
    f.worker.save(state).unwrap();

    let now = io::now_ms() + 120_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(f.agent, now, |_| None),
        Some(source)
    );
    state
        .route_worker_replies(now + 10_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();

    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("reply_not_captured")),
        "in-turn progress is not the request's final result: {notices:?}"
    );
}

#[test]
fn during_turn_progress_send_still_posts_a_later_different_final() {
    let mut f = Fixture::new();
    let orch = f.orch;
    let source = running(&mut f, Author::Agent(orch));
    let mut state = f.worker.state.clone();
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Brief read.".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(f.agent),
            io::now_ms(),
        )
        .unwrap();
    f.worker.save(state).unwrap();
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
    // One submit delivers only the queue head, which is the in-turn progress
    // send. The later final is the next notice, so the proof is that notice,
    // not the call.
    let notices = f.notices();
    assert!(
        notices
            .iter()
            .any(|notice| notice.prompt.text.contains("The real late result.")),
        "a later different final must still wake: {notices:?}"
    );
}

#[path = "worker_event_waiter_overlap_test.rs"]
mod waiter_overlap;
