#[test]
fn detected_state_follows_the_screen() {
    let mut terminal = test_terminal();
    let change = terminal
        .set_detected_state(Some(AgentKind::Claude), AgentState::Working)
        .unwrap();
    assert_eq!(change.state, AgentState::Working);
    assert_eq!(terminal.effective_agent_label(), Some("claude"));
    assert!(terminal
        .set_detected_state(Some(AgentKind::Claude), AgentState::Working)
        .is_none());
}
#[test]
fn border_label_prefers_the_manual_label() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.set_manual_label("notes".into());
    assert_eq!(terminal.border_label(true).as_deref(), Some("notes"));
}

#[test]
fn replacement_process_first_idle_does_not_report_work_completion() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.set_detected_agent_process_at(AgentKind::Codex, now);
    terminal.finish_agent_process_acquisition();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    assert!(terminal.finish_agent_process_acquisition());
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Working);
    terminal.finish_agent_process_acquisition();
    terminal.set_detected_state_with_screen_signals_at(
        Some(AgentKind::Codex),
        AgentState::Idle,
        false,
        true,
        now + Duration::from_secs(1),
    );
    terminal.finish_agent_process_acquisition();

    // The detector recognizes a new Codex before publishing a clear-agent event.
    terminal.set_detected_agent_process_at(AgentKind::Codex, now + Duration::from_secs(2));
    terminal.finish_agent_process_acquisition();
    let change = terminal
        .set_detected_state(Some(AgentKind::Codex), AgentState::Idle)
        .unwrap();
    let suppress_completion = terminal.finish_agent_process_acquisition();
    let reports_completion = crate::server::notifications::policy::is_completion_transition_parts(
        change.previous_state,
        change.state,
        change.previous_agent_label.as_deref(),
        change.agent_label.as_deref(),
    );

    assert!(
        !reports_completion || suppress_completion,
        "the replacement's first idle screen must not report completion before any work"
    );
}
