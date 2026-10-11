#[test]
fn a_stale_session_sequence_is_ignored() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Pi), AgentState::Idle);
    assert!(terminal
        .set_agent_session_ref_for_session_start(
            "herdr:pi".into(),
            "pi".into(),
            Some(session_id("first")),
            Some(2),
            None,
            &[],)
        .is_some());
    assert!(terminal
        .set_agent_session_ref_for_session_start(
            "herdr:pi".into(),
            "pi".into(),
            Some(session_id("older")),
            Some(2),
            None,
            &[],)
        .is_none());
    assert_eq!(
        terminal.persisted_agent_session.unwrap().session_ref.value,
        "first"
    );
}
