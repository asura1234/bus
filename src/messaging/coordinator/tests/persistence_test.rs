use super::*;
use crate::messaging::coordinator::storage::{STORAGE_NEEDS_REPAIR, STORAGE_RETRYING};
use crate::messaging::storage::state_store::SaveStage;
use std::time::Instant;

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
    assert!(matches!(
        worker.storage_pause.as_ref(),
        Some(StoragePause::NeedsRepair)
    ));
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

#[test]
fn status_only_polls_skip_disk_writes_and_recovery_checks_the_last_commit() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let before = std::fs::read(worker.store.path()).unwrap();
    let committed = worker.durable_state.clone();
    let mut observed = worker.state.clone();
    observed
        .observe_status(agent, RuntimeStatus::Idle, 42)
        .unwrap();
    worker.store.fail_once_at(SaveStage::TempSync);
    worker.apply_poll(observed).unwrap();
    assert_eq!(std::fs::read(worker.store.path()).unwrap(), before);
    assert_eq!(worker.durable_state, committed);
    assert_eq!(worker.state.agent(agent).unwrap().observed_at_ms, 42);

    let mut changed = worker.state.clone();
    changed
        .observe_status(agent, RuntimeStatus::Working, 43)
        .unwrap();
    assert_eq!(worker.apply_poll(changed).unwrap_err(), STORAGE_RETRYING);
    assert_eq!(std::fs::read(worker.store.path()).unwrap(), before);
    let (events, received) = mpsc::channel();
    worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
    assert!(matches!(
        received.try_recv(),
        Ok(BusEvent::StorageRecovered)
    ));
    assert_eq!(worker.store.load().unwrap(), Some(worker.state.clone()));
    assert_eq!(worker.state.agent(agent).unwrap().observed_at_ms, 42);

    let mut changed = worker.state.clone();
    changed
        .observe_status(agent, RuntimeStatus::Working, 44)
        .unwrap();
    worker.apply_poll(changed).unwrap();
    assert_eq!(worker.store.load().unwrap(), Some(worker.state.clone()));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn skipped_observation_is_saved_before_submission_and_survives_restart() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "keep ownership");
    let before = std::fs::read(worker.store.path()).unwrap();
    let mut observed = worker.state.clone();
    observed
        .observe_status(agent, RuntimeStatus::Idle, 85)
        .unwrap();
    worker.apply_poll(observed).unwrap();
    assert_eq!(std::fs::read(worker.store.path()).unwrap(), before);

    let mut submitting = worker.state.clone();
    submitting.begin_submission(request, "launch", 0).unwrap();
    worker.save(submitting).unwrap();
    let on_disk = worker.store.load().unwrap().unwrap();
    assert_eq!(
        on_disk.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert_eq!(on_disk.request(request).unwrap().progress_at_ms, Some(85));
    assert_eq!(
        on_disk.request(request).unwrap().submission_status_revision,
        worker.state.agent(agent).unwrap().status_revision
    );
    assert!(calls.lock().unwrap().is_empty());

    // A duplicated descriptor keeps a flock alive after the original handle
    // closes. Coordinator teardown must explicitly unlock before reopening.
    let held_clone = worker._lease.0.try_clone().unwrap();
    assert!(io::lock(&dir.join("coordinator.lock")).is_err());
    drop(worker);
    let recovered = Worker::open(
        dir.clone(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: calls.clone(),
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap();
    assert_eq!(
        recovered.state.request(request).unwrap().phase,
        RequestPhase::Submitting
    );
    assert!(calls.lock().unwrap().is_empty());
    drop(recovered);
    drop(held_clone);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn temporary_state_write_failures_pause_then_recover_without_replaying_a_mutation() {
    for stage in [
        SaveStage::TempCreate,
        SaveStage::TempWrite,
        SaveStage::TempSync,
    ] {
        let (mut worker, _, room, dir, calls) = fixture(Provider::Codex, vec![]);
        let committed = worker.state.clone();
        let mut changed = committed.clone();
        changed.set_room_notes(room, "not committed").unwrap();
        worker.store.fail_once_at(stage);
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| assert_eq!(worker.save(changed).unwrap_err(), STORAGE_RETRYING));
        assert!(matches!(
            worker.storage_pause.as_ref(),
            Some(StoragePause::Retrying { .. })
        ));
        assert_eq!(worker.store.load().unwrap(), Some(committed.clone()));
        assert_eq!(worker.state, committed);
        assert!(calls.lock().unwrap().is_empty());
        let logs = capture.text();
        assert!(logs.contains(stage.name()), "{logs}");
        assert!(logs.contains("raw_os_error=Some(28)"), "{logs}");
        assert!(logs.contains("bytes="), "{logs}");

        let (events, received) = mpsc::channel();
        worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
        assert!(worker.storage_pause.is_none());
        assert!(worker.error.is_none());
        assert!(matches!(
            received.try_recv(),
            Ok(BusEvent::StorageRecovered)
        ));
        assert_eq!(worker.store.load().unwrap(), Some(committed));
        assert!(calls.lock().unwrap().is_empty());
        assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".state.json.tmp-")
        }));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn post_replace_failure_never_overwrites_an_ambiguous_commit() {
    let (mut worker, _, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let committed = worker.state.clone();
    let mut changed = committed.clone();
    changed.set_room_notes(room, "maybe committed").unwrap();
    worker.store.fail_once_at(SaveStage::ParentSync);
    assert_eq!(worker.save(changed.clone()).unwrap_err(), STORAGE_RETRYING);
    assert_eq!(worker.store.load().unwrap(), Some(changed));
    assert_eq!(worker.state, committed);
    let before = std::fs::read(worker.store.path()).unwrap();
    let (events, received) = mpsc::channel();
    worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
    assert!(matches!(
        worker.storage_pause.as_ref(),
        Some(StoragePause::NeedsRepair)
    ));
    assert_eq!(worker.error.as_deref(), Some(STORAGE_NEEDS_REPAIR));
    assert_eq!(std::fs::read(worker.store.path()).unwrap(), before);
    assert!(received.try_recv().is_err());
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovery_refuses_corrupt_durable_state() {
    let (mut worker, _, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut changed = worker.state.clone();
    changed.set_room_notes(room, "not committed").unwrap();
    worker.store.fail_once_at(SaveStage::TempWrite);
    assert!(worker.save(changed).is_err());
    std::fs::write(worker.store.path(), b"{corrupt").unwrap();
    let (events, _) = mpsc::channel();
    worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
    assert!(matches!(
        worker.storage_pause.as_ref(),
        Some(StoragePause::NeedsRepair)
    ));
    assert_eq!(std::fs::read(worker.store.path()).unwrap(), b"{corrupt");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovered_storage_does_not_resubmit_uncertain_delivery() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "submit outcome unknown".into(),
            code: None,
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "one delivery");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    let calls_before = calls.lock().unwrap().len();
    worker.store.fail_once_at(SaveStage::TempSync);
    assert!(worker.save(worker.state.clone()).is_err());
    let (events, _) = mpsc::channel();
    worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), calls_before);
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn paused_worker_rejects_send_without_touching_native_transport() {
    let (mut worker, _, room, dir, calls) = fixture(Provider::Codex, vec![]);
    worker.store.fail_once_at(SaveStage::TempCreate);
    assert!(worker.save(worker.state.clone()).is_err());
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (commands, receiver) = mpsc::sync_channel(4);
    let (events, received) = mpsc::channel();
    commands.send((1, BusCommand::Submit(room))).unwrap();
    commands.send((2, BusCommand::Shutdown)).unwrap();
    worker.run(receiver, events, snapshots);
    assert!(received.try_iter().any(|event| {
        matches!(event, BusEvent::CommandFinished { command_id: 1, result: Err(error) } if error == STORAGE_RETRYING)
    }));
    assert!(calls.lock().unwrap().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn storage_retry_delay_doubles_and_stops_at_thirty_seconds() {
    let (mut worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    worker.store.fail_once_at(SaveStage::TempCreate);
    assert!(worker.save(worker.state.clone()).is_err());
    let (events, _) = mpsc::channel();
    let mut now = Instant::now() + Duration::from_secs(1);
    for expected in [2, 4, 8, 16, 30, 30] {
        worker.store.fail_once_at(SaveStage::TempSync);
        worker.retry_storage(now, &events);
        let Some(StoragePause::Retrying { next_retry, delay }) = worker.storage_pause.as_ref()
        else {
            panic!("retry should remain scheduled");
        };
        assert_eq!(*delay, Duration::from_secs(expected));
        assert_eq!(*next_retry, now + *delay);
        now = *next_retry;
    }
    worker.retry_storage(now, &events);
    assert!(worker.storage_pause.is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
