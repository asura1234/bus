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
fn grok_g_saved_identity_is_not_ready_before_the_resume_process_starts() {
    for kind in [AgentKind::Grok, AgentKind::Claude] {
        let mut terminal = test_terminal();
        terminal.restore_managed_agent("restored".into(), kind);
        terminal.pending_agent_resume_plan =
            Some(crate::agents::resume::catalog::AgentResumePlan {
                agent: crate::agents::agent_label(kind).into(),
                argv: vec!["not-started".into()],
                dedupe_key: "saved-session".into(),
            });
        assert!(terminal.managed_agent_launch_pending());
        assert!(!terminal.managed_agent_interactive_ready());
        terminal.pending_agent_resume_plan = None;
        assert!(terminal.managed_agent_interactive_ready());
    }
}
