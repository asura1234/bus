#[test]
fn error_pipe_developer_dropdown_lists_only_current_master_orchestrators_and_saves_selection() {
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.ensure_master_room();
    let master = snapshot.state.master_room().unwrap().id;
    let lead = snapshot.state.create_agent(master, "lead", Provider::ClaudeCode, "/project".into(), None).unwrap();
    snapshot.state.bind_orchestrator(lead, room).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.dev_settings = true;
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 120, 65);
    assert!(screen.contains("DEVELOPER"), "{screen}");
    assert!(screen.contains("Bus errors: Off"), "{screen}");
    for _ in 0..ui.feishu_field()+3 { key(&mut ui, KeyCode::Down, KeyModifiers::NONE); }
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let screen = room_screen(&mut ui, 120, 65);
    assert!(screen.contains("lead"), "{screen}");
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.settings.error_orchestrator.as_deref(), Some("lead"));
    assert!(ui.pending.iter().any(|pending| matches!(&pending.command, BusCommand::SetErrorOrchestrator(Some(name)) if name=="lead")));
    assert!(matches!(ui.form, Some(forms::Form::Settings)));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(ui.settings.error_orchestrator.as_deref(), Some("lead"), "cancel changes nothing");
    assert!(matches!(ui.form, Some(forms::Form::Settings)));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.settings.error_orchestrator, None);
}

#[test]
fn error_pipe_option_is_absent_from_non_dev_settings() {
    let (mut ui, _, _) = fixture();
    ui.dev_settings = false;
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 120, 65);
    assert!(!screen.contains("Bus errors:"), "{screen}");
    assert!(!screen.contains("DEVELOPER"), "{screen}");
}
