#[test]
fn process_exit_clears_the_matching_persisted_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: session_id("codex-1"),
    });
    terminal.set_detected_state_with_screen_signals_at(
        Some(Agent::Codex),
        AgentState::Unknown,
        false,
        true,
        Instant::now(),
    );
    assert!(terminal.persisted_agent_session.is_none());
    assert!(terminal.effective_agent_label().is_none());
}
#[test]
fn process_exit_keeps_a_foreign_persisted_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agent_resume::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: session_id("claude-1"),
    });
    terminal.set_detected_state_with_screen_signals_at(
        Some(Agent::Codex),
        AgentState::Unknown,
        false,
        true,
        Instant::now(),
    );
    assert_eq!(
        terminal
            .persisted_agent_session
            .as_ref()
            .map(|session| session.agent.as_str()),
        Some("claude")
    );
}
