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
