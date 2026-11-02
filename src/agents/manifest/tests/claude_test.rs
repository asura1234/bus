use super::*;

#[test]
fn claude_idle_prompt_with_background_shell_is_idle() {
    // Captured from Claude Code 2.1.251 after its foreground turn ended while
    // a long-lived background shell remained active (issue #3414).
    let screen = concat!(
        "✻ Sautéed for 10s · 1 shell still running\n\n",
        "──────────────────────────────────────────────────────── WINDOWS ─\n",
        "❯\n",
        "────────────────────────────────────────────────────────────────\n",
        "  ⏵⏵ auto mode on · 1 shell · ← for agents                     /rc\n",
    );
    let result = detect_screen_with_osc(AgentKind::Claude, screen, "", "");

    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("live_prompt_box")
    );
    assert!(result.visible_idle);
    assert!(!result.visible_working);
}

#[test]
fn claude_background_shell_without_foreground_evidence_is_idle_fallback() {
    let result = detect_screen_with_osc(
        AgentKind::Claude,
        "  ⏵⏵ auto mode on · 1 shell · ← for agents\n",
        "",
        "",
    );

    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(result.matched_rule, None);
    assert!(!result.visible_working);
}

#[test]
fn claude_live_turn_with_background_shell_remains_working() {
    let screen = concat!(
        "────────────────────────────────────────────────────────────────\n",
        "❯\n",
        "────────────────────────────────────────────────────────────────\n",
        "  ⏵⏵ auto mode on · 1 shell · esc to interrupt\n",
    );
    let result = detect_screen_with_osc(AgentKind::Claude, screen, "", "");

    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("live_turn_working")
    );
    assert!(result.visible_working);
}

#[test]
fn claude_osc_title_braille_prefix_is_working() {
    // "⠂" is U+2802, in the braille block U+2800-U+28FF
    let result = detect_screen_with_osc(AgentKind::Claude, "", "⠂ project", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
    assert!(result.visible_working);
}

#[test]
fn claude_osc_title_half_circle_frames_are_working() {
    for frame in ['◐', '◓', '◑', '◒'] {
        let title = format!("{frame} Initial conversation with Claude");
        let result = detect_screen_with_osc(AgentKind::Claude, "", &title, "");
        assert_eq!(result.state, AgentState::Working, "frame {frame}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("osc_title_working"),
            "frame {frame}"
        );
        assert!(result.visible_working, "frame {frame}");
    }
}

#[test]
fn claude_osc_title_static_prefix_is_idle() {
    // "✳" is U+2733, static prefix when Claude is not working
    let result = detect_screen_with_osc(AgentKind::Claude, "", "✳ Claude Code", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn claude_osc_progress_4_3_alone_does_not_force_working() {
    // Claude leaves progress stuck at 4;3 while waiting for permission, so
    // 4;3 must not be a working signal on its own. With no other evidence it
    // falls back to idle; current input controls are handled by the terminal driver.
    let result = detect_screen_with_osc(AgentKind::Claude, "", "", "4;3;");
    assert_eq!(result.state, AgentState::Idle);
    assert!(!result.visible_working);
}

#[test]
fn claude_osc_progress_4_0_is_idle() {
    let result = detect_screen_with_osc(AgentKind::Claude, "", "", "4;0;");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_progress_idle")
    );
}

#[test]
fn claude_empty_osc_empty_screen_is_idle_fallback() {
    // No OSC data, no matching screen rule → fallback idle (unchanged V3 behavior)
    let result = detect_screen_with_osc(AgentKind::Claude, "", "", "");
    assert_eq!(result.state, AgentState::Idle);
    assert!(!result.visible_idle);
}

// --- Codex OSC rules ---
