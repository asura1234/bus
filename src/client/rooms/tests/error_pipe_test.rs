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


#[test]
fn error_report_provider_failure_does_not_reenter_the_capture_inbox() {
    use crate::messaging::model::{
        AgentRuntimeIdentity, CallbackDisposition, CallbackEventKind, ProviderCallback,
        SubmissionOutcome,
    };
    use tracing_subscriber::prelude::*;

    let root = crate::utils::test_temp::unique_temp_path("bus-error-report-provider-failure");
    let mut runtime = crate::utils::error_pipe::Runtime::default();
    runtime.configure(&root, Some("lead"));
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.ensure_master_room();
    let master = snapshot.state.master_room().unwrap().id;
    let state = &mut snapshot.state;
    let lead = state
        .create_agent(master, "lead", Provider::ClaudeCode, root.clone(), None)
        .unwrap();
    state.bind_orchestrator(lead, room).unwrap();
    state.set_agent_runtime_identity(lead, AgentRuntimeIdentity {
        launch_id: Some("launch".into()),
        terminal_id: Some("terminal".into()),
        pane_id: Some("pane".into()),
        session_id: Some("session".into()),
    }).unwrap();
    state.confirm_hook_setup(lead).unwrap();
    state.observe_status(lead, RuntimeStatus::Idle, 100).unwrap();
    let report = state.submit_message_with(room, Draft {
        text: "Bus error: original.failure (x1 since last notice). Logs: /logs/server.log".into(),
        files: vec![],
        recipient_ids: [lead].into(),
    }, Author::Bus, 100, true).unwrap()[0];
    state.begin_submission(report, "launch", 0).unwrap();
    state.record_submission(report, SubmissionOutcome::Confirmed {
        provider_session_id: Some("session".into()),
        provider_turn_id: Some("turn-1".into()),
    }).unwrap();
    let started = ProviderCallback {
        callback_id: "start-report".into(),
        sequence: 1,
        occurred_at_ms: 101,
        agent_id: lead,
        launch_id: "launch".into(),
        provider_session_id: Some("session".into()),
        provider_turn_id: Some("turn-1".into()),
        provider_prompt_id: Some("turn-1".into()),
        prompt_payload: Some(state.render_request_payload(report).unwrap()),
        kind: CallbackEventKind::PromptStarted,
    };
    assert_eq!(state.accept_callback(started.clone()), CallbackDisposition::AcceptedBinding);
    assert!(state.request(report).unwrap().is_error_report());
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot.clone()));

    let subscriber = tracing_subscriber::registry().with(
        crate::utils::error_pipe::CaptureLayer::new(root.join("client.log")),
    );
    tracing::subscriber::with_default(subscriber, || {
        // A native StopFailure arrives after report submission and its suppression ended.
        assert_eq!(snapshot.state.accept_callback(ProviderCallback {
            callback_id: "report-stop-failure".into(),
            sequence: 2,
            occurred_at_ms: 102,
            prompt_payload: None,
            kind: CallbackEventKind::Error {
                message: "Provider failed".into(),
                provider_error: Some("API Error: Connection lost mid-response".into()),
            },
            ..started
        }), CallbackDisposition::AcceptedError);
        assert!(snapshot.state.request(report).unwrap().failure_reason
            .as_deref().unwrap().starts_with("provider_error:"));
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
    });
    let records = crate::utils::error_pipe::drain(&root).unwrap();
    let events: Vec<_> = records.iter().map(|record| record.event.as_str()).collect();
    std::fs::remove_dir_all(root).unwrap();
    assert!(events.is_empty(), "report's asynchronous provider failure must not create another error notice: {events:?}");
}
