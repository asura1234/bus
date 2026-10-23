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
    terminal.begin_managed_agent(
        "bus-r8259-a21848".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    let screen = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · ~/work/libtv-app-prd-canvasbar\n  ? for shortcuts                       ⚠ 1 warning · f2 to view\n";
    let detection = crate::agents::detect_agent_with_launch_context(
        Some(AgentKind::Codex),
        screen,
        "⠋ Codex",
        "",
        terminal.managed_agent_launch_pending(),
    );
    terminal.set_detected_state(Some(AgentKind::Codex), detection.state);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(4), false);
    assert!(terminal.managed_agent_interactive_ready());
    assert!(terminal.persisted_agent_session.is_none());
}

#[test]
fn managed_launch_signal_tracks_blocked_active_timeout_and_exit() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let mut terminal = test_terminal();
    let signal = Arc::new(AtomicBool::new(false));
    terminal.bind_managed_launch_pending(signal.clone());
    let now = Instant::now();
    terminal.begin_managed_agent(
        "builder".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(1),
        Duration::from_secs(30),
    );
    assert!(signal.load(Ordering::Acquire));
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Blocked);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(2), false);
    assert!(
        signal.load(Ordering::Acquire),
        "a startup dialog keeps the launch window open"
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.reconcile_managed_agent_at(now + Duration::from_secs(3), false);
    assert!(
        !signal.load(Ordering::Acquire),
        "interactive readiness closes the window"
    );
    let screen = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · /repo\n  ? for shortcuts\n";
    assert_eq!(
        crate::agents::detect_agent_with_launch_context(
            Some(AgentKind::Codex),
            screen,
            "⠋ Codex",
            "",
            signal.load(Ordering::Acquire)
        )
        .state,
        AgentState::Working
    );
    for process_exited in [false, true] {
        terminal.begin_managed_agent(
            "builder".into(),
            AgentKind::Codex,
            now,
            Duration::from_secs(1),
            Duration::from_secs(30),
        );
        assert!(signal.load(Ordering::Acquire));
        terminal.reconcile_managed_agent_at(now + Duration::from_secs(31), process_exited);
        assert!(
            !signal.load(Ordering::Acquire),
            "timeout/exit must close the window"
        );
    }
    terminal.begin_managed_agent(
        "builder".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(1),
        Duration::from_secs(30),
    );
    terminal.clear_agent_name();
    assert!(!signal.load(Ordering::Acquire));
    terminal.restore_managed_agent("resumed".into(), AgentKind::Codex);
    assert!(
        !signal.load(Ordering::Acquire),
        "an already-active restore is not a pending launch"
    );
}
