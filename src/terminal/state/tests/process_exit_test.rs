#[test]
fn process_exit_clears_the_matching_persisted_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: session_id("codex-1"),
    });
    terminal.set_detected_state_with_screen_signals_at(
        Some(AgentKind::Codex),
        AgentState::Unknown,
        false,
        true,
        None,
        Instant::now(),
    );
    assert!(terminal.persisted_agent_session.is_none());
    assert!(terminal.effective_agent_label().is_none());
}
#[test]
fn process_exit_keeps_a_foreign_persisted_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: session_id("claude-1"),
    });
    terminal.set_detected_state_with_screen_signals_at(
        Some(AgentKind::Codex),
        AgentState::Unknown,
        false,
        true,
        None,
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
#[test]
fn feishu_process_exit_requires_explicit_latch_and_excludes_self_update() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    assert!(!terminal.attention_process_exited());
    terminal.set_detected_state_with_screen_signals_at(
        Some(AgentKind::Codex),
        AgentState::Unknown,
        false,
        true,
        None,
        Instant::now(),
    );
    assert!(terminal.attention_process_exited());
    let now = Instant::now();
    for phase in [
        SelfUpdatePhase::Installing {
            deadline: now,
            interrupted: false,
        },
        SelfUpdatePhase::Relaunching {
            failure: None,
            next_try: now,
            deadline: now,
        },
        SelfUpdatePhase::Stuck("waiting".into()),
    ] {
        terminal.self_update = Some(SelfUpdate {
            name: "codex".into(),
            kind: AgentKind::Codex,
            args: Vec::new(),
            session: None,
            success: "updated".into(),
            phase,
        });
        assert!(
            !terminal.attention_process_exited(),
            "automatic update exit is not developer attention"
        );
    }
    terminal.self_update = None;
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    assert!(!terminal.attention_process_exited());
}

#[test]
fn feishu_automatic_update_relaunch_does_not_report_the_previous_process_exit() {
    let mut terminal = test_terminal();
    let now = Instant::now();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.set_detected_state_with_screen_signals_at(
        Some(AgentKind::Codex),
        AgentState::Unknown,
        false,
        true,
        None,
        now,
    );
    terminal.begin_managed_agent(
        "codex".into(),
        AgentKind::Codex,
        now,
        Duration::from_secs(3),
        Duration::from_secs(30),
    );
    terminal.self_update = Some(SelfUpdate {
        name: "codex".into(),
        kind: AgentKind::Codex,
        args: Vec::new(),
        session: None,
        success: "updated".into(),
        phase: SelfUpdatePhase::Relaunched,
    });

    assert!(terminal.managed_agent_launch_pending());
    assert!(terminal.effective_agent_label().is_none());
    assert!(
        !terminal.attention_process_exited(),
        "the automatic update exit must stay suppressed until the new process is detected"
    );
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.set_detected_state_with_screen_signals_at(
        Some(AgentKind::Codex),
        AgentState::Unknown,
        false,
        true,
        None,
        now + Duration::from_secs(5),
    );
    assert!(
        terminal.attention_process_exited(),
        "a real exit of the replacement process must still report"
    );
}
