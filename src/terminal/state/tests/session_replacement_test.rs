#[test]
fn claude_resume_replaces_the_persisted_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: session_id("old"),
    });
    let mutation = terminal
        .set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(session_id("new")),
            Some(1),
            Some("resume".into()),
            &[],)
        .unwrap();
    assert!(mutation.session_ref_changed);
    assert_eq!(
        terminal.persisted_agent_session.unwrap().session_ref.value,
        "new"
    );
}
#[test]
fn claude_startup_does_not_replace_an_existing_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Claude), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:claude".into(),
        agent: "claude".into(),
        session_ref: session_id("old"),
    });
    assert!(terminal
        .set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(session_id("new")),
            Some(1),
            Some("startup".into()),
            &[],)
        .is_none());
    assert_eq!(
        terminal.persisted_agent_session.unwrap().session_ref.value,
        "old"
    );
}
#[test]
fn a_different_owner_does_not_replace_the_session_without_the_foreground_agent() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Codex), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:codex".into(),
        agent: "codex".into(),
        session_ref: session_id("codex"),
    });
    assert!(terminal
        .set_agent_session_ref_for_session_start(
            "herdr:claude".into(),
            "claude".into(),
            Some(session_id("claude")),
            Some(1),
            None,
            &[],)
        .is_none());
}

#[test]
fn grok_new_and_load_hooks_replace_only_the_same_owned_session() {
    for source in ["new", "load"] {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(AgentKind::Grok), AgentState::Idle);
        terminal.set_persisted_agent_session(
            crate::agents::resume::catalog::PersistedAgentSession {
                source: "herdr:grok".into(),
                agent: "grok".into(),
                session_ref: session_id("old"),
            },
        );
        let mutation = terminal
            .set_agent_session_ref_for_session_start(
                "herdr:grok".into(),
                "grok".into(),
                Some(session_id("new")),
                Some(2),
                Some(source.into()),
                &[],
            )
            .expect("Grok 1.0.70 new/load report must rebind its own pane");
        assert!(mutation.session_ref_changed);
        assert_eq!(
            terminal.persisted_agent_session.unwrap().session_ref.value,
            "new"
        );
    }
}

#[test]
fn grok_unknown_start_source_cannot_replace_an_existing_session() {
    let mut terminal = test_terminal();
    terminal.set_detected_state(Some(AgentKind::Grok), AgentState::Idle);
    terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
        source: "herdr:grok".into(),
        agent: "grok".into(),
        session_ref: session_id("old"),
    });
    assert!(terminal
        .set_agent_session_ref_for_session_start(
            "herdr:grok".into(),
            "grok".into(),
            Some(session_id("new")),
            Some(3),
            Some("other".into()),
            &[],
        )
        .is_none());
}

#[test]
fn kimi_clear_rebinds_only_the_explicit_new_session_report() {
    for (source, should_rebind) in [("new", true), ("startup", false)] {
        let mut terminal = test_terminal();
        terminal.set_detected_state(Some(AgentKind::Kimi), AgentState::Idle);
        terminal.set_persisted_agent_session(crate::agents::resume::catalog::PersistedAgentSession {
            source: "herdr:kimi".into(),
            agent: "kimi".into(),
            session_ref: session_id("old"),
        });
        let changed = terminal.set_agent_session_ref_for_session_start(
            "herdr:kimi".into(), "kimi".into(), Some(session_id("fresh")), Some(2),
            Some(source.into()), &[],
        );
        assert_eq!(changed.is_some(), should_rebind, "source={source}");
        assert_eq!(terminal.persisted_agent_session.unwrap().session_ref.value,
            if should_rebind { "fresh" } else { "old" });
    }
}
