use super::*;

#[test]
fn delete_agent_stops_terminal_before_removing_persisted_work() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "discard me");
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.request(request).is_none());
    assert!(worker
        .state
        .room(room)
        .unwrap()
        .draft
        .recipient_ids
        .is_empty());
    assert_eq!(*calls.lock().unwrap(), vec!["pane.close_if_identity"]);
    worker.submit_ready().unwrap();
    drop(worker);
    let recovered = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(recovered.agent(agent).is_none());
    assert!(recovered.request(request).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn delete_room_partial_close_preserves_retryable_state_and_other_rooms() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![
            Ok(ResponseResult::Ok {}),
            Err(TransportError {
                message: "stop failed".into(),
                code: Some("terminal_stop_failed".into()),
                definitely_rejected: false,
            }),
        ],
    );
    let project = dir.join("project-file.txt");
    std::fs::write(&project, "keep project data").unwrap();
    let other_room = worker.state.create_room("other").unwrap();
    let other_agent = worker
        .state
        .create_agent(other_room, "unrelated", Provider::Codex, dir.clone(), None)
        .unwrap();
    let second = worker
        .state
        .create_agent(room, "second", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker
        .state
        .set_agent_runtime_identity(
            second,
            AgentRuntimeIdentity {
                launch_id: Some("second-launch".into()),
                terminal_id: Some("second-terminal".into()),
                pane_id: Some("second-pane".into()),
                session_id: Some("second-session".into()),
            },
        )
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "remove");
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteRoom(room), &events)
        .is_err());
    assert!(worker.state.room(room).unwrap().deletion_pending);
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(worker.state.agent(second).unwrap().deletion_pending);
    assert!(!worker.state.agent(other_agent).unwrap().deletion_pending);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 2);
    drop(worker);
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    assert!(recovered.state.room(room).is_none());
    assert!(recovered.state.agent(agent).is_none());
    assert!(recovered.state.agent(second).is_none());
    assert!(recovered.state.request(request).is_none());
    assert!(recovered.state.agent(other_agent).is_some());
    assert!(recovered.state.room(other_room).is_some());
    assert_eq!(
        std::fs::read_to_string(project).unwrap(),
        "keep project data"
    );
    assert_eq!(calls.lock().unwrap().len(), 4);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn storage_failure_prevents_send_and_startup_failure_releases_coordinator() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "prompt");
    io::atomic_write(&dir.join("state.json"), b"corrupt").unwrap();
    assert!(worker.submit_ready().is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.storage_failed);
    drop(worker);
    let fake = || {
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: calls.clone(),
            state_path: dir.join("state.json"),
        })
    };
    assert!(Worker::open(dir.clone(), fake()).is_err());
    assert!(io::lock(&dir.join("coordinator.lock")).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_open_gives_every_session_exactly_one_master_room() {
    let (worker, _agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let master = worker
        .state
        .master_room()
        .expect("new session has MASTER")
        .id;
    assert_ne!(master, room);
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.master_room().map(|room| room.id), Some(master));
    drop(worker);

    let reopened = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls,
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap();
    assert_eq!(
        reopened
            .state
            .rooms()
            .filter(|room| room.kind == RoomKind::Master)
            .map(|room| room.id)
            .collect::<Vec<_>>(),
        [master]
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn master_session_saved_before_sound_and_compactions_keeps_its_orchestrators() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("pr-1").unwrap();
    let orchestrator = state
        .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    let mut value = serde_json::to_value(&state).unwrap();
    for room in value["rooms"].as_object_mut().unwrap().values_mut() {
        room.as_object_mut().unwrap().remove("sound");
    }
    for agent in value["agents"].as_object_mut().unwrap().values_mut() {
        agent.as_object_mut().unwrap().remove("compactions");
    }
    value
        .as_object_mut()
        .unwrap()
        .remove("consumed_dialog_fingerprints");
    let (worker, dir) = open_saved_document(json!({"version": 1, "state": value}));
    assert_eq!(worker.state.master_room().map(|room| room.id), Some(master));
    assert_eq!(
        worker
            .state
            .rooms()
            .filter(|room| room.kind == RoomKind::Master)
            .count(),
        1
    );
    assert!(worker.state.room(master).unwrap().sound_enabled());
    assert!(!worker.state.room(work).unwrap().sound_enabled());
    let agent = worker.state.agent(orchestrator).unwrap();
    assert_eq!(agent.orchestrates, Some(work));
    assert_eq!(agent.compactions.count, 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
