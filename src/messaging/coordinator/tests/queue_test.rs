use super::*;

#[test]
fn delivery_logs_explain_queued_hook_gate_once_without_prompt_contents() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
        let mut value = serde_json::to_value(&worker.state).unwrap();
        value["agents"][agent.0.to_string()]["hook_setup_confirmed"] = json!(false);
        worker.save(serde_json::from_value(value).unwrap()).unwrap();
        let (events, _) = mpsc::channel();
        worker
            .command(
                BusCommand::SetDraftText(room, "PRIVATE_PROMPT".into()),
                &events,
            )
            .unwrap();
        worker
            .command(BusCommand::SetRecipients(room, [agent].into()), &events)
            .unwrap();
        worker.command(BusCommand::Submit(room), &events).unwrap();
        for _ in 0..3 {
            worker.submit_ready().unwrap();
        }
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(
            worker.state.requests().next().unwrap().phase,
            RequestPhase::Queued
        );
        let logs = capture.text();
        assert!(logs.contains("bus.message.queued"), "{logs}");
        assert_eq!(logs.matches("bus.delivery.wait").count(), 1, "{logs}");
        assert!(logs.contains("hook_setup_unconfirmed"), "{logs}");
        assert!(logs.contains("request_id=4"), "{logs}");
        assert!(!logs.contains("PRIVATE_PROMPT"), "{logs}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn queued_delete_is_applied_before_background_delivery_of_earlier_submit() {
    struct ReadyTransport {
        calls: Arc<Mutex<Vec<&'static str>>>,
        delete_during_poll: Option<(mpsc::SyncSender<(u64, BusCommand)>, AgentId)>,
    }
    impl Transport for ReadyTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            self.calls
                .lock()
                .unwrap()
                .push(crate::protocol::api::api_method_name(&method));
            if matches!(method, Method::AgentList(_)) {
                if let Some((commands, agent)) = self.delete_during_poll.take() {
                    commands.send((2, BusCommand::DeleteAgent(agent))).unwrap();
                    commands.send((3, BusCommand::Shutdown)).unwrap();
                }
                let info = serde_json::from_value(json!({
                    "terminal_id":"terminal", "agent":"codex", "agent_status":"idle",
                    "agent_session":{"source":"herdr:codex","agent":"codex","kind":"id","value":"session"},
                    "workspace_id":"workspace", "tab_id":"tab", "pane_id":"pane",
                    "focused":false, "interactive_ready":true, "revision":1
                })).unwrap();
                Ok(ResponseResult::AgentList { agents: vec![info] })
            } else {
                Ok(ResponseResult::Ok {})
            }
        }
    }
    for during_poll in [false, true] {
        let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
        let (commands, receiver) = mpsc::sync_channel(256);
        worker.transport = Box::new(ReadyTransport {
            calls: Arc::clone(&calls),
            delete_during_poll: during_poll.then(|| (commands.clone(), agent)),
        });
        worker
            .state
            .set_draft_text(room, "must be discarded before delivery")
            .unwrap();
        worker.state.set_draft_recipients(room, [agent]).unwrap();
        worker.save(worker.state.clone()).unwrap();
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (events, received) = mpsc::channel();
        commands.send((1, BusCommand::Submit(room))).unwrap();
        if !during_poll {
            commands.send((2, BusCommand::DeleteAgent(agent))).unwrap();
            commands.send((3, BusCommand::Shutdown)).unwrap();
        }
        worker.run(receiver, events, Arc::clone(&snapshots));
        let expected = if during_poll {
            vec!["agent.list", "pane.close_if_identity"]
        } else {
            vec!["pane.close_if_identity"]
        };
        assert_eq!(*calls.lock().unwrap(), expected);
        assert!(snapshots.lock().unwrap().state.agent(agent).is_none());
        let outcomes: Vec<_> = received
            .try_iter()
            .filter_map(|event| match event {
                BusEvent::CommandFinished { command_id, result } => Some((command_id, result)),
                _ => None,
            })
            .collect();
        assert_eq!(outcomes, vec![(1, Ok(())), (2, Ok(())), (3, Ok(()))]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn failed_draft_command_keeps_its_outcome_after_later_success() {
    // Removing command-specific results must fail: an aggregate acknowledgement
    // cannot tell the shell whether it may discard a pending local draft edit.
    let (worker, _, room, dir, _) = fixture(Provider::Codex, vec![]);
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (commands, receiver) = mpsc::sync_channel(8);
    let (events, outcomes) = mpsc::channel();
    commands
        .send((
            11,
            BusCommand::SetDraftText(RoomId(u64::MAX), "unsaved".into()),
        ))
        .unwrap();
    commands
        .send((12, BusCommand::SetNotes(room, "saved notes".into())))
        .unwrap();
    drop(commands);
    worker.run(receiver, events, Arc::clone(&snapshots));
    let results: Vec<_> = outcomes
        .try_iter()
        .filter_map(|event| match event {
            BusEvent::CommandFinished { command_id, result } => Some((command_id, result)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results.len(),
        2,
        "each processed command needs its own outcome"
    );
    assert_eq!(results[0].0, 11);
    assert!(results[0].1.is_err());
    assert_eq!(results[1], (12, Ok(())));
    let snapshot = snapshots.lock().unwrap();
    assert_eq!(snapshot.last_command_id, 12);
    assert_eq!(snapshot.state.room(room).unwrap().notes, "saved notes");
    drop(snapshot);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn busy_rejection_restores_exact_fifo_and_blocked_does_not_submit() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "busy".into(),
            code: Some("agent_not_idle".into()),
            definitely_rejected: true,
        })],
    );
    let first = queue(&mut worker, room, agent, "first");
    let second = queue(&mut worker, room, agent, "second");
    worker.submit_ready().unwrap();
    assert_eq!(worker.state.queued_requests(agent), &[first, second]);
    assert!(worker.state.agent(agent).unwrap().current_request.is_none());
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 3)
        .unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn late_old_identical_codex_final_cannot_bind_and_provider_error_holds_queue() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "unknown".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"old","prompt":"same"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let request = queue(&mut worker, room, agent, "same");
    let next = queue(&mut worker, room, agent, "next");
    // Delivery holds while that turn of its own runs, until the grace ends.
    worker.submit_ready().unwrap();
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);
    worker.own_turns.clear();
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"old","last_assistant_message":"stale same prompt final"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"current","prompt":"same"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"StopFailure","session_id":"session","turn_id":"current"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert_eq!(worker.state.queued_requests(agent), &[next]);
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_ref()
        .unwrap()
        .contains("failed"));
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unstarted_request_is_released_when_the_agent_begins_another_turn_of_its_own() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = unstarted(&mut worker, agent, room, &dir);
    let next = queue(&mut worker, room, agent, "Human message");
    record(
        &dir,
        Provider::ClaudeCode,
        claude(
            "UserPromptSubmit",
            "own-2",
            json!({"prompt":"typed in terminal"}),
        ),
    );
    consume(&mut worker, agent, &dir);
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);
    // Bus is not an agent: releasing the request posts nothing anywhere.
    assert!(worker.state.rooms().all(|room| room.notices.is_empty()));
    // The queue moves on once that turn settles.
    record(
        &dir,
        Provider::ClaudeCode,
        claude("Stop", "own-2", json!({"last_assistant_message":"done"})),
    );
    consume(&mut worker, agent, &dir);
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(next)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_unstarted_request_is_released_once_the_agent_stays_idle() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = unstarted(&mut worker, agent, room, &dir);
    let settled = worker
        .state
        .request(request)
        .unwrap()
        .foreign_turn_settled_at_ms
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, settled + 1)
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request),
        "a queued prompt starts right after the turn before it; wait for it"
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, settled + UNBOUND_SETTLE_MS)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Abandoned
    );
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_session_start_alone_never_releases_an_unstarted_request() {
    // Claude reports SessionStart `resume` ahead of a task-notification turn,
    // and Codex its first `startup` with the prompt itself.
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = queue(&mut worker, room, agent, "Before the hook");
    worker.submit_ready().unwrap();
    for source in ["startup", "resume", "clear"] {
        record(
            &dir,
            Provider::ClaudeCode,
            json!({"hook_event_name":"SessionStart","session_id":"session","source":source}),
        );
    }
    consume(&mut worker, agent, &dir);
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_started_request_is_never_released_by_the_agents_own_turns() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = queue(&mut worker, room, agent, "Real work");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        claude("UserPromptSubmit", "bus", json!({"prompt":"Real work"})),
    );
    consume(&mut worker, agent, &dir);
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"session","source":"resume"}),
    );
    consume(&mut worker, agent, &dir);
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms() + UNBOUND_SETTLE_MS)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Active
    );
    std::fs::remove_dir_all(dir).unwrap();
}
