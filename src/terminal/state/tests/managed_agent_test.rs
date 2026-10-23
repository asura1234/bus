#[test]
fn managed_agent_becomes_ready_when_detection_is_idle() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent(
        "builder".into(),
        AgentKind::Claude,
        now,
        Duration::from_millis(1),
        Duration::from_secs(30),
    );
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    assert!(terminal.reconcile_managed_agent_at(now + Duration::from_millis(2), false));
    assert!(terminal.managed_agent_interactive_ready());
}

#[test]
fn staffing_codex_captured_startup_screen_confirms_native_readiness_without_a_session() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.begin_managed_agent("bus-r8259-a21848".into(), AgentKind::Codex, now, Duration::from_secs(3), Duration::from_secs(30));
    let screen = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · ~/work/libtv-app-prd-canvasbar\n  ? for shortcuts                       ⚠ 1 warning · f2 to view\n";
    let detection = crate::agents::detect_agent_with_osc(Some(AgentKind::Codex), screen, "⠋ Codex", "");
    terminal.set_detected_state(Some(AgentKind::Codex), detection.state);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
    assert!(terminal.managed_agent_interactive_ready());
    assert!(terminal.persisted_agent_session.is_none());
}
