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
