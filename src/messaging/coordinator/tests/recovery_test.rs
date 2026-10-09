use super::*;

#[test]
fn delete_failure_suspends_delivery_across_restart_and_allows_retry() {
    for ownership_lookup in [
        native_agents(Some("bus-r1-a2".into())),
        Err(TransportError {
            message: "server busy".into(),
            code: None,
            definitely_rejected: false,
        }),
    ] {
        delete_failure_suspends_while_native_ownership_is_unproven(ownership_lookup);
    }
}

/// Regression: a Codex agent whose pane respawned after a Bus restart lost its
/// native managed name, so every guarded close answered
/// `terminal_identity_changed` and deletion stayed suspended forever.
#[test]
fn delete_finishes_and_reports_terminal_left_open_when_native_ownership_was_released() {
    for delete_room in [false, true] {
        let (mut worker, agent, room, dir, calls) = fixture(
            Provider::Codex,
            vec![identity_changed(), native_agents(None)],
        );
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        worker.save(worker.state.clone()).unwrap();
        let request = queue(&mut worker, room, agent, "stranded at awaiting_start");
        let (events, received) = mpsc::channel();
        let command = if delete_room {
            BusCommand::DeleteRoom(room)
        } else {
            BusCommand::DeleteAgent(agent)
        };
        worker.command(command, &events).unwrap();
        assert!(worker.state.agent(agent).is_none());
        assert!(worker.state.request(request).is_none());
        assert_eq!(worker.state.room(room).is_none(), delete_room);
        // Never a second, unguarded close: the pane now holds foreign work.
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["pane.close_if_identity", "agent.list"]
        );
        let left_open = vec![LeftOpenTerminal {
            agent_id: agent,
            agent_name: "author".into(),
            pane_id: "pane".into(),
            terminal_id: "terminal".into(),
        }];
        assert!(received
            .try_iter()
            .any(|event| matches!(&event, BusEvent::TerminalsLeftOpen(t) if *t == left_open)));
        let report = worker.snapshot().error.unwrap();
        assert!(
            report.contains("\"author\"") && report.contains("pane pane, terminal"),
            "{report}"
        );
        drop(worker);
        let recovered = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert!(recovered.agent(agent).is_none());
        assert!(recovered.request(request).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delete_unknown_launch_identity_fails_closed_without_transport() {
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![]);
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("uncertain-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unsupported_native_close_requires_server_restart_and_never_falls_back() {
    for code in ["unknown_method", "invalid_request"] {
        let (mut worker, agent, _, dir, calls) = fixture(
            Provider::Codex,
            vec![Err(TransportError {
                message: "RAW_API_DETAIL unknown_method: pane.close_if_identity; expected one of many API variants".repeat(20),
                code: Some(code.into()),
                definitely_rejected: true,
            })],
        );
        let (events, _) = mpsc::channel();
        let error = worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap_err();
        assert!(
            error.contains("Update and restart the Bus server"),
            "{error}"
        );
        assert!(error.len() < 220, "user-facing error must remain compact");
        assert!(!error.contains("RAW_API_DETAIL"));
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deletion_transport_details_stay_out_of_user_errors() {
    for result in [
        Err(TransportError {
            message: "RAW_API_DETAIL terminal driver details".repeat(100),
            code: Some("terminal_stop_failed".into()),
            definitely_rejected: false,
        }),
        Ok(ResponseResult::AgentList { agents: vec![] }),
    ] {
        let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![result]);
        let (events, _) = mpsc::channel();
        let error = worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap_err();
        assert!(error.len() < 220, "{error}");
        assert!(
            !error.contains("RAW_API_DETAIL") && !error.contains("AgentList"),
            "{error}"
        );
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn successful_deletion_retry_clears_previous_worker_snapshot_error() {
    for delete_room in [false, true] {
        let (worker, agent, room, dir, _) = fixture(
            Provider::Codex,
            vec![
                Err(TransportError {
                    message: "temporary stop failure".into(),
                    code: Some("terminal_stop_failed".into()),
                    definitely_rejected: true,
                }),
                Ok(ResponseResult::Ok {}),
            ],
        );
        let deletion = if delete_room {
            BusCommand::DeleteRoom(room)
        } else {
            BusCommand::DeleteAgent(agent)
        };
        let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
        let (commands, receiver) = mpsc::sync_channel(256);
        let (events, received) = mpsc::channel();
        commands.send((1, deletion.clone())).unwrap();
        commands.send((2, deletion)).unwrap();
        commands.send((3, BusCommand::Shutdown)).unwrap();
        worker.run(receiver, events, Arc::clone(&snapshots));
        let outcomes: Vec<_> = received
            .try_iter()
            .filter_map(|event| match event {
                BusEvent::CommandFinished { result, .. } => Some(result),
                _ => None,
            })
            .collect();
        assert!(outcomes[0].is_err());
        assert!(outcomes[1].is_ok());
        assert!(snapshots.lock().unwrap().error.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn delete_last_room_stays_empty_after_restart_and_ignores_late_callbacks() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "deleted request");
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"SessionStart", "session_id":"late-session"}),
    );
    drop(worker);
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![] })].into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    // Only the undeletable MASTER room remains.
    assert_eq!(
        recovered
            .state
            .rooms()
            .map(|room| room.kind)
            .collect::<Vec<_>>(),
        [RoomKind::Master]
    );
    assert!(!recovered.state.has_work());
    assert_eq!(recovered.state.agents().count(), 0);
    assert_eq!(recovered.state.requests().count(), 0);
    assert!(!recovered.state.is_pristine());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["pane.close_if_identity", "agent.list"]
    );
    let new_room = recovered.state.create_room("new room").unwrap();
    assert!(new_room.0 > agent.0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn initial_recovery_tick_keeps_detached_room_reply_unread_until_selected() {
    // A saved visible room belongs to the detached client. The first coordinator
    // tick must not mark its newly spooled reply seen before any UI selection.
    let (mut worker, agent, room_b, dir, _) = fixture(Provider::Codex, vec![]);
    let room_a = worker.state.create_room("other room").unwrap();
    worker.state.select_room(room_b).unwrap();
    let request = queue(&mut worker, room_b, agent, "finish while detached");
    worker.state.begin_submission(request, "launch", 0).unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"finish while detached"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    assert_eq!(worker.state.room(room_b).unwrap().unread_count, 0);
    drop(worker);
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"completed while detached"}),
    );
    let info = serde_json::from_value(json!({
        "terminal_id": "terminal", "agent": "codex", "agent_status": "idle",
        "agent_session": {"source": "herdr:codex", "agent": "codex", "kind": "id", "value": "session"},
        "workspace_id": "workspace", "tab_id": "tab", "pane_id": "pane",
        "focused": false, "interactive_ready": true, "revision": 1
    }))
    .unwrap();
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    assert_eq!(
        recovered.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        recovered.state.room(room_b).unwrap().latest_replies[&agent].text,
        "completed while detached"
    );
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    let (events, _) = mpsc::channel();
    recovered
        .command(BusCommand::SelectRoom(room_a), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    assert_eq!(
        recovered
            .store
            .load()
            .unwrap()
            .unwrap()
            .room(room_b)
            .unwrap()
            .unread_count,
        1
    );
    recovered
        .command(BusCommand::SelectRoom(room_b), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Regression: a Cursor agent rebound to its new chat while the server kept the
/// old one; every guarded close answered `terminal_identity_changed` and the
/// agent could never be deleted.
#[test]
fn delete_leaves_open_an_owned_terminal_whose_provider_session_moved() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            native_agents(Some("bus-r1-a2".into())),
            Ok(ResponseResult::AgentInfo {
                agent: native_info("pane", "cursor", "moved"),
            }),
        ],
    );
    let request = queue(&mut worker, room, agent, "stuck");
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["pane.close_if_identity", "agent.list", "agent.get"]
    );
    assert!(received.try_iter().any(|event| matches!(
        event,
        BusEvent::TerminalsLeftOpen(terminals) if terminals[0].pane_id == "pane"
    )));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_keeps_an_agent_whose_terminal_carries_another_managed_name() {
    let mut foreign = native_info("pane", "cursor", "moved");
    foreign.name = Some("bus-r1-a9".into());
    let (mut worker, agent, _room, dir, calls) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            native_agents(Some("bus-r1-a2".into())),
            Ok(ResponseResult::AgentInfo { agent: foreign }),
        ],
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "pane.close_if_identity",
            "agent.list",
            "agent.get",
            "agent.list"
        ]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Regression: after a restart the agent's managed name moved to a restored
/// terminal that kept another Cursor chat, Bus never rebound to it, and every
/// deletion failed with "The terminal session changed".
#[test]
fn delete_leaves_open_a_restored_terminal_whose_provider_session_moved() {
    let mut restored = native_info("w1:p7", "cursor", "moved");
    restored.terminal_id = "restored".into();
    let restored_list = || {
        Ok(ResponseResult::AgentList {
            agents: vec![restored.clone()],
        })
    };
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            restored_list(),
            Err(TransportError {
                message: "pane not found".into(),
                code: Some("pane_not_found".into()),
                definitely_rejected: true,
            }),
            restored_list(),
        ],
    );
    let request = queue(&mut worker, room, agent, "stuck");
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "pane.close_if_identity",
            "agent.list",
            "agent.get",
            "agent.list"
        ]
    );
    assert!(received.try_iter().any(|event| matches!(
        event,
        BusEvent::TerminalsLeftOpen(terminals)
            if terminals[0].pane_id == "w1:p7" && terminals[0].terminal_id == "restored"
    )));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_keeps_an_agent_whose_restored_terminal_runs_its_own_session() {
    let mut restored = native_info("w1:p7", "cursor", "session");
    restored.terminal_id = "restored".into();
    let restored_list = || {
        Ok(ResponseResult::AgentList {
            agents: vec![restored.clone()],
        })
    };
    let (mut worker, agent, _room, dir, _) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            restored_list(),
            Err(TransportError {
                message: "pane not found".into(),
                code: Some("pane_not_found".into()),
                definitely_rejected: true,
            }),
            restored_list(),
        ],
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).is_some());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
