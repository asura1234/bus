#[test]
fn moved_agent_detection_routes_through_production_dispatch() {
    let detection = detect_agent_with_osc(Some(AgentKind::Pi), "Working...", "", "");

    assert_eq!(detection.state, AgentState::Working);
    assert!(detection.visible_working);
}

#[test]
fn session_identity_integrations_leave_state_to_screen_detection() {
    for (source, label, agent) in [
        ("herdr:hermes", "hermes", AgentKind::Hermes),
        ("herdr:qwen", "qwen", AgentKind::Qwen),
        ("herdr:antigravity_cli", "agy", AgentKind::Antigravity),
    ] {
        assert!(session_identity_only_integration(source, label));
        assert!(AgentKind::SCREEN_MANIFEST_AGENTS.contains(&agent));
    }
}

#[test]
fn no_agent_returns_unknown() {
    assert_eq!(
        detect_agent_with_osc(None, "anything", "", "").state,
        AgentState::Unknown
    );
}
