#[test]
fn disk_settings_arrow_steppers_show_defaults_and_queue_changes() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 120, 60);
    assert!(screen.contains("Disk warning (GB free)"), "{screen}");
    assert!(screen.contains("Disk critical (GB free)"), "{screen}");
    assert!(screen.contains("‹ 20 ›"), "{screen}");
    assert!(screen.contains("‹ 5 ›"), "{screen}");
    let before = ui.pending.len();
    for _ in 0..ui.disk_warning_field() {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(
        serde_json::to_value(&ui.settings).unwrap()["disk_warning_gb"],
        21
    );
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(
        serde_json::to_value(&ui.settings).unwrap()["disk_warning_gb"],
        20
    );
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(
        serde_json::to_value(&ui.settings).unwrap()["disk_critical_gb"],
        6
    );
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(
        serde_json::to_value(&ui.settings).unwrap()["disk_critical_gb"],
        5
    );
    assert_eq!(ui.pending.len(), before + 4);
}

#[test]
fn disk_settings_mouse_steppers_enforce_both_bounds_and_critical_below_warning() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 120, 60);
    for action in [
        render::Action::AdjustDiskWarningGb(true),
        render::Action::AdjustDiskCriticalGb(false),
    ] {
        assert!(ui.view.hits.iter().any(|hit| hit.action == action));
    }
    for (warning, critical, action) in [
        (10_000, 5, render::Action::AdjustDiskWarningGb(true)),
        (20, 1, render::Action::AdjustDiskCriticalGb(false)),
        (6, 5, render::Action::AdjustDiskWarningGb(false)),
        (6, 5, render::Action::AdjustDiskCriticalGb(true)),
    ] {
        ui.settings.disk_warning_gb = warning;
        ui.settings.disk_critical_gb = critical;
        let before = ui.pending.len();
        ui.action(action);
        assert_eq!(
            (ui.settings.disk_warning_gb, ui.settings.disk_critical_gb),
            (warning, critical)
        );
        assert_eq!(ui.pending.len(), before);
    }
    ui.action(render::Action::AdjustDiskWarningGb(true));
    ui.action(render::Action::AdjustDiskCriticalGb(true));
    assert_eq!(
        (ui.settings.disk_warning_gb, ui.settings.disk_critical_gb),
        (7, 6)
    );
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::SetDiskWarningGb(7))));
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::SetDiskCriticalGb(6))));
}
