use super::*;

fn ready_worker(f: &mut Fixture) {
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
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 10_000)
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
            assert!(f.calls.lock().unwrap()[0].contains("Exited with open request"));
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
    f.worker.save(state).unwrap();
    collect(&mut f);
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
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
