use super::*;

#[test]
fn agent_errors_are_logged_once_each_and_never_shown_as_room_notices() {
    let (mut ui, room, agent) = fixture();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        for error in [
            "Hook setup is blocked",
            "Hook setup is blocked",
            "Hook setup needs approval",
        ] {
            let mut snapshot = (*ui.snapshot).clone();
            snapshot
                .state
                .set_agent_error(agent, Some(error.into()))
                .unwrap();
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
        }
    });
    let logs = capture.text();
    assert_eq!(logs.matches("bus.agent.notice").count(), 2, "{logs}");
    assert!(logs.contains("agent=author"), "{logs}");
    assert!(logs.contains(&format!("room_id={}", room.0)), "{logs}");
    assert!(logs.contains("Hook setup needs approval"), "{logs}");
    assert!(!room_screen(&mut ui, 100, 30).contains("Hook setup needs approval"));
}

#[test]
fn coordinator_errors_are_logged_once_and_never_title_the_composer() {
    let (mut ui, _, _) = fixture();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        for _ in 0..2 {
            let mut snapshot = (*ui.snapshot).clone();
            snapshot.error = Some("Coordinator storage failed".into());
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
        }
    });
    let logs = capture.text();
    assert_eq!(logs.matches("bus.coordinator.error").count(), 1, "{logs}");
    assert!(logs.contains("Coordinator storage failed"), "{logs}");
    let screen = room_screen(&mut ui, 100, 30);
    assert!(!screen.contains("Coordinator storage failed"), "{screen}");
    assert_eq!(
        ui.snapshot.error.as_deref(),
        Some("Coordinator storage failed")
    );
}

#[test]
fn storage_pause_is_logged_with_its_cause_and_never_drawn() {
    let (mut ui, _, _) = fixture();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.error = Some("Storage paused; disk full".into());
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    let logs = capture.text();
    assert!(logs.contains("bus.storage.paused"), "{logs}");
    assert!(logs.contains("disk full"), "{logs}");
    for _ in 0..2 {
        let screen = room_screen(&mut ui, 100, 30);
        assert!(!screen.contains("Storage paused"), "{screen}");
        assert!(!screen.contains("disk full"), "{screen}");
        assert!(!screen.contains("Storage recovered"), "{screen}");
    }

    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = None;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.receive_event(BusEvent::StorageRecovered);
    let screen = room_screen(&mut ui, 100, 30);
    assert!(!screen.contains("Storage recovered"), "{screen}");
}

#[test]
fn storage_pause_stays_log_only_during_quit_notices() {
    let (mut ui, _, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = Some("Storage paused; retrying".into());
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.request_quit();
    ui.quitting = Some(std::time::Instant::now() - std::time::Duration::from_secs(6));
    ui.tick_quit(std::time::Instant::now());
    assert!(!room_screen(&mut ui, 100, 30).contains("Storage paused"));
    ui.force_exit_until = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));
    ui.tick_quit(std::time::Instant::now());
    assert!(!room_screen(&mut ui, 100, 30).contains("Storage paused"));
}

#[test]
fn scrolling_draws_no_storage_pause_marker() {
    let (mut ui, _, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = Some("Storage paused; retrying".into());
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.sidebar_scroll = 1;
    let screen = room_screen(&mut ui, 100, 10);
    assert_eq!(ui.sidebar_scroll, 1, "the fixture must really scroll");
    assert!(!screen.contains("Storage paused"), "{screen}");
    assert!(!screen.contains("ROOMSge paused"), "{screen}");
}

#[test]
fn failed_agent_add_logs_its_worker_projected_cause_once() {
    let (mut ui, room, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::Codex));
    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        *name = editor::Editor::new("worker".into());
        *cwd = editor::Editor::new("/definitely/missing-log-projection".into());
    }
    ui.action(render::Action::Add);
    let command_id = ui
        .pending
        .iter()
        .find(|pending| matches!(pending.command, BusCommand::AddAgent(_)))
        .unwrap()
        .id;
    let preceding: Vec<_> = ui
        .pending
        .iter()
        .filter(|p| p.id < command_id)
        .map(|p| p.id)
        .collect();
    let error = "Agent PWD /definitely/missing-log-projection cannot be read";
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        for command_id in preceding {
            ui.receive_event(BusEvent::CommandFinished {
                command_id,
                result: Ok(()),
            });
        }
        ui.receive_event(BusEvent::CommandFinished {
            command_id,
            result: Err(error.into()),
        });
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = command_id;
        snapshot.revision += 1;
        // Worker::run publishes both projections of the same failed command.
        snapshot.error = Some(error.into());
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
    });
    let logs = capture.text();
    eprintln!("worker rejection logs:\n{logs}");
    assert!(matches!(ui.form, Some(forms::Form::Agent { .. })));
    assert!(ui
        .snapshot
        .state
        .agents()
        .all(|agent| agent.name != "worker"));
    assert!(!room_screen(&mut ui, 100, 40).contains(error));
    assert!(
        logs.contains(&format!("room_id=Some({})", room.0)),
        "{logs}"
    );
    assert_eq!(
        logs.matches(error).count(),
        1,
        "one rejected Add should have one attributable client warning: {logs}"
    );
}

#[test]
fn directory_error_alias_has_one_contextual_client_warning() {
    let (mut ui, _, agent) = fixture();
    let error = "Working directory is unavailable: /gone-log-projection";
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .observe_working_directory_error(agent, Some(error.into()))
        .unwrap();
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        ui.receive_snapshot(Arc::new(snapshot));
        ui.compute_view(100, 40);
        ui.compute_view(100, 40);
    });
    let logs = capture.text();
    eprintln!("directory alias logs:\n{logs}");
    assert!(
        logs.contains("bus.agent.working_directory_failed"),
        "{logs}"
    );
    assert_eq!(
        logs.matches(error).count(),
        1,
        "the model's two aliases represent one directory failure: {logs}"
    );
}

#[test]
fn normalized_unassigned_master_agent_keeps_its_log_replacement() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let agent = state
        .create_agent(
            master,
            "orphan",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    let mut value = serde_json::to_value(&state).unwrap();
    value["agents"][agent.0.to_string()]["orchestrates"] = serde_json::json!(999);
    state = serde_json::from_value(value).unwrap();
    // Exercise the same normalization as Worker::open before a client sees state.
    state.ensure_master_room();
    assert_eq!(state.agent(agent).unwrap().orchestrates, None);
    state = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    state.ensure_master_room();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    ui.open_room(master);
    let capture = crate::utils::logging::test_capture::Capture::default();
    let mut screen = String::new();
    capture.run(|| {
        screen = room_screen(&mut ui, 100, 40);
        ui.compute_view(100, 40);
    });
    let logs = capture.text();
    eprintln!(
        "normalized MASTER screen_has_orphan={} screen_has_unavailable={}; logs={logs}",
        screen.contains("orphan"),
        screen.contains("Unavailable")
    );
    assert_eq!(logs.matches("bus.agent.orchestration_room_unavailable").count(), 1,
        "an invalid saved assignment still needs one client-log replacement after normalization: {logs}");
    assert!(!screen.contains("# unassigned"), "{screen}");
    assert!(screen.contains("Unavailable"), "{screen}");
    assert!(logs.contains("agent=orphan"), "{logs}");
    assert!(logs.contains("room_id=0"), "{logs}");
    assert!(logs.contains("target_room_id=999"), "{logs}");

    let mut snapshot = (*ui.snapshot).clone();
    let room = snapshot.state.create_room("repaired-target").unwrap();
    snapshot.state.bind_orchestrator(agent, room).unwrap();
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        ui.receive_snapshot(Arc::new(snapshot));
        screen = room_screen(&mut ui, 100, 40);
    });
    assert!(!screen.contains("Unavailable"), "{screen}");
    assert!(screen.contains("repaired-target"), "{screen}");
    assert!(!capture
        .text()
        .contains("bus.agent.orchestration_room_unavailable"));
}

#[test]
fn initial_snapshot_errors_are_logged_once_and_observed_again_after_recovery() {
    for (error, event) in [
        ("Storage paused; retrying", "bus.storage.paused"),
        ("Coordinator refresh failed", "bus.coordinator.error"),
    ] {
        let (ui, _, _) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.error = Some(error.into());
        snapshot.last_command_id = 17;
        let capture = crate::utils::logging::test_capture::Capture::default();
        let mut observed = None;
        capture.run(|| observed = Some(BusUi::new(Arc::new(snapshot))));
        let mut ui = observed.unwrap();
        capture.run(|| {
            ui.receive_snapshot(ui.snapshot.clone());
            ui.receive_snapshot(ui.snapshot.clone());
        });
        let logs = capture.text();
        assert_eq!(logs.matches(event).count(), 1, "{logs}");
        assert_eq!(logs.matches(error).count(), 1, "{logs}");
        assert!(logs.contains("last_command_id=17"), "{logs}");
        assert!(!room_screen(&mut ui, 100, 40).contains(error));
        capture.run(|| {
            let mut snapshot = (*ui.snapshot).clone();
            snapshot.error = None;
            ui.receive_snapshot(Arc::new(snapshot));
            let mut snapshot = (*ui.snapshot).clone();
            snapshot.error = Some(error.into());
            ui.receive_snapshot(Arc::new(snapshot));
        });
        assert_eq!(capture.text().matches(event).count(), 2);
    }
}

#[test]
fn snapshot_error_waits_for_its_command_acknowledgement() {
    let (mut ui, room, _) = fixture();
    ui.queue(BusCommand::SetNotes(room, "notes".into()), Effect::None);
    let command_id = ui.pending.back().unwrap().id;
    let error = "Notes save rejected";
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = command_id;
        snapshot.error = Some(error.into());
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
    });
    assert!(capture.text().is_empty(), "{}", capture.text());
    capture.run(|| {
        ui.receive_event(BusEvent::CommandFinished {
            command_id,
            result: Err(error.into()),
        });
        ui.settle();
        ui.receive_snapshot(ui.snapshot.clone());
    });
    let logs = capture.text();
    assert_eq!(logs.matches(error).count(), 1, "{logs}");
    assert!(logs.contains("bus.command.failed"), "{logs}");
    assert!(
        logs.contains(&format!("room_id=Some({})", room.0)),
        "{logs}"
    );
}

#[test]
fn independent_snapshot_error_survives_a_different_command_failure() {
    let (mut ui, room, _) = fixture();
    ui.queue(BusCommand::SetNotes(room, "notes".into()), Effect::None);
    let command_id = ui.pending.back().unwrap().id;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        ui.receive_event(BusEvent::CommandFinished {
            command_id,
            result: Err("Notes command rejected".into()),
        });
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = command_id;
        snapshot.error = Some("Independent coordinator refresh failed".into());
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
        ui.receive_snapshot(ui.snapshot.clone());
    });
    let logs = capture.text();
    for error in [
        "Notes command rejected",
        "Independent coordinator refresh failed",
    ] {
        assert_eq!(logs.matches(error).count(), 1, "{logs}");
    }
    assert!(logs.contains("bus.command.failed"), "{logs}");
    assert!(logs.contains("bus.coordinator.error"), "{logs}");
}

#[test]
fn independent_agent_error_is_not_lost_when_directory_error_recovers() {
    let (mut ui, _, agent) = fixture();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .set_agent_error(agent, Some("Hook setup blocked".into()))
            .unwrap();
        snapshot
            .state
            .observe_working_directory_error(agent, Some("Directory vanished".into()))
            .unwrap();
        ui.receive_snapshot(Arc::new(snapshot));
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .observe_working_directory_error(agent, None)
            .unwrap();
        ui.receive_snapshot(Arc::new(snapshot));
        ui.compute_view(100, 40);
    });
    let logs = capture.text();
    assert_eq!(logs.matches("Hook setup blocked").count(), 1, "{logs}");
    assert_eq!(logs.matches("Directory vanished").count(), 1, "{logs}");
    assert_eq!(
        ui.snapshot
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .as_deref(),
        Some("Hook setup blocked")
    );
}

#[test]
fn legacy_unassigned_master_is_unavailable_but_ordinary_workers_are_not() {
    let (mut ui, room, worker) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.confirm_hook_setup(worker).unwrap();
    snapshot
        .state
        .observe_status(worker, RuntimeStatus::Idle, 1)
        .unwrap();
    let master = snapshot.state.ensure_master_room();
    let orphan = snapshot
        .state
        .create_agent(
            master,
            "legacy-orphan",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    snapshot.state.confirm_hook_setup(orphan).unwrap();
    snapshot
        .state
        .observe_status(orphan, RuntimeStatus::Idle, 1)
        .unwrap();
    snapshot.state =
        serde_json::from_value(serde_json::to_value(&snapshot.state).unwrap()).unwrap();
    snapshot.state.ensure_master_room();
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| ui.receive_snapshot(Arc::new(snapshot)));
    let logs = capture.text();
    assert_eq!(
        logs.matches("bus.agent.orchestration_room_unavailable")
            .count(),
        1,
        "{logs}"
    );
    assert!(logs.contains("agent=legacy-orphan"), "{logs}");
    ui.open_room(master);
    assert!(room_screen(&mut ui, 100, 40).contains("Unavailable"));
    ui.open_room(room);
    assert!(!room_screen(&mut ui, 100, 40).contains("Unavailable"));
}

#[test]
fn coalesced_snapshot_keeps_the_original_command_error_single() {
    let (mut ui, room, _) = fixture();
    let rejected = ui.queue(BusCommand::SetNotes(room, "notes".into()), Effect::None);
    let following = ui.queue(BusCommand::SetDraftText(room, "draft".into()), Effect::None);
    let error = "Earlier notes command rejected";
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        ui.receive_event(BusEvent::CommandFinished {
            command_id: rejected,
            result: Err(error.into()),
        });
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = following;
        snapshot.error = Some(error.into());
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
        ui.receive_event(BusEvent::CommandFinished {
            command_id: following,
            result: Ok(()),
        });
        ui.settle();
        ui.receive_snapshot(ui.snapshot.clone());
    });
    let logs = capture.text();
    assert_eq!(logs.matches(error).count(), 1, "{logs}");
    assert!(logs.contains(&format!("command_id={rejected}")), "{logs}");
    assert!(logs.contains("bus.command.failed"), "{logs}");
}
