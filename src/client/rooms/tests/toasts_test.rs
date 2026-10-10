use super::*;

#[test]
fn toast_defaults_to_fifteen_seconds_and_accepts_an_explicit_duration() {
    let (mut ui, _, _) = fixture();

    ui.show_toast("Saved");
    let started_at = ui.toast.as_ref().expect("default toast").started_at;
    assert_eq!(
        ui.toast_text_at(started_at + std::time::Duration::from_secs(14), 20)
            .as_deref(),
        Some("Saved")
    );
    assert!(ui
        .toast_text_at(started_at + std::time::Duration::from_secs(15), 20)
        .is_none());

    ui.show_toast_for("Brief", std::time::Duration::from_secs(2));
    let started_at = ui.toast.as_ref().expect("explicit toast").started_at;
    assert_eq!(
        ui.toast_text_at(started_at + std::time::Duration::from_millis(1_999), 20)
            .as_deref(),
        Some("Brief")
    );
    assert!(ui
        .toast_text_at(started_at + std::time::Duration::from_secs(2), 20)
        .is_none());
}

#[test]
fn long_toast_pauses_then_scrolls_at_eight_characters_per_second() {
    let (mut ui, _, _) = fixture();
    ui.show_toast_for("abcdefghijklmnop", std::time::Duration::from_secs(30));
    let started_at = ui.toast.as_ref().expect("long toast").started_at;
    let text_at = |ui: &BusUi, millis| {
        ui.toast_text_at(started_at + std::time::Duration::from_millis(millis), 8)
            .expect("visible toast")
    };

    assert_eq!(text_at(&ui, 999), "abcdefgh");
    assert_eq!(text_at(&ui, 1_125), "bcdefghi");
    assert_eq!(text_at(&ui, 2_000), "ijklmnop");
    assert_eq!(text_at(&ui, 2_999), "ijklmnop");
    assert_eq!(text_at(&ui, 3_000), "abcdefgh");
}

#[test]
fn active_toast_tick_repaints_for_scrolling_and_expiration() {
    let (mut ui, _, _) = fixture();
    ui.show_toast_for("abcdefghijklmnop", std::time::Duration::from_secs(15));
    ui.toast_animation_last_tick =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(125));
    assert!(ui.tick(), "scrolling toast needs a new rendered frame");

    ui.toast.as_mut().expect("toast before expiry").started_at -=
        std::time::Duration::from_secs(15);
    assert!(ui.tick(), "toast expiry needs a frame that removes its row");
    assert!(ui.toast.is_none());
}

#[test]
fn repeated_agent_error_snapshots_do_not_restart_the_toast() {
    let (mut ui, _, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_error(agent, Some("Hook setup is blocked".into()))
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.toast.as_mut().expect("agent error toast").started_at -= std::time::Duration::from_secs(1);
    let first_started_at = ui.toast.as_ref().unwrap().started_at;

    let mut repeated = (*ui.snapshot).clone();
    repeated.revision += 1;
    ui.receive_snapshot(Arc::new(repeated));
    assert_eq!(ui.toast.as_ref().unwrap().started_at, first_started_at);

    let mut changed = (*ui.snapshot).clone();
    changed
        .state
        .set_agent_error(agent, Some("Hook setup needs approval".into()))
        .unwrap();
    changed.revision += 1;
    ui.receive_snapshot(Arc::new(changed));
    assert_eq!(
        ui.toast.as_ref().unwrap().message,
        "author: Hook setup needs approval"
    );
    assert!(ui.toast.as_ref().unwrap().started_at > first_started_at);
}

#[test]
fn expired_coordinator_toast_stays_in_diagnostics_without_reappearing() {
    let (mut ui, _, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = Some("Coordinator storage failed".into());
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let started_at = ui.toast.as_ref().expect("coordinator toast").started_at;

    assert!(ui
        .toast_text_at(started_at + std::time::Duration::from_secs(15), 80)
        .is_none());
    assert_eq!(
        ui.snapshot.error.as_deref(),
        Some("Coordinator storage failed")
    );

    let mut repeated = (*ui.snapshot).clone();
    repeated.revision += 1;
    ui.receive_snapshot(Arc::new(repeated));
    assert_eq!(ui.toast.as_ref().unwrap().started_at, started_at);
}

#[test]
fn a_wide_character_toast_scrolls_far_enough_to_render_its_tail() {
    let (mut ui, _, _) = fixture();
    ui.show_toast_for(
        "一二三四五六七八九十甲乙丙丁戊己庚辛",
        std::time::Duration::from_secs(30),
    );
    let mut rendered = String::new();
    for elapsed_ms in (0..15_000).step_by(125) {
        ui.toast.as_mut().unwrap().started_at =
            std::time::Instant::now() - std::time::Duration::from_millis(elapsed_ms);
        ui.compute_view(36, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 36, 30));
        ui.render(&mut buffer);
        rendered.extend((0..36).flat_map(|x| buffer[(x, 29)].symbol().chars()));
    }
    assert!(
        rendered.contains('辛'),
        "the notice's final character must become visible before expiration"
    );
}
