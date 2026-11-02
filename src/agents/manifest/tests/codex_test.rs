use super::*;

#[test]
fn codex_osc_title_braille_spinner_is_working() {
    // "⠋" is U+280B, in the braille block
    let result = detect_screen_with_osc(AgentKind::Codex, "", "⠋ llm-proxy", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_osc_title_action_required_is_blocked() {
    let result = detect_screen_with_osc(
        AgentKind::Codex,
        "",
        "[ . ] Action Required | llm-proxy",
        "",
    );
    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_blocked")
    );
    assert!(result.visible_blocker);
}

#[test]
fn codex_osc_title_plain_is_idle() {
    let result = detect_screen_with_osc(AgentKind::Codex, "", "llm-proxy", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn legacy_automatic_update_actions_are_rejected() {
    for table in [
        r#"auto_update = { choose = "Update now" }"#,
        r#"auto_update = { choose = "", success = "done" }"#,
        r#"auto_update = { choose = "Update now", success = "  " }"#,
    ] {
        let manifest = rules_manifest(&format!(
            "[[rules]]\nid = \"update\"\nstate = \"blocked\"\ncontains = [\"update\"]\n{table}\n"
        ));
        assert!(parse_manifest(&manifest).is_err(), "{table}");
    }
    let valid = rules_manifest(
        "[[rules]]\nid = \"update\"\nstate = \"blocked\"\ncontains = [\"update\"]\n\
         auto_update = { choose = \"Update now\", success = \"done\" }\n",
    );
    assert!(parse_manifest(&valid).is_err());
}

#[test]
fn codex_background_terminal_screen_does_not_override_osc_idle() {
    // Background terminal tasks can be long-lived helpers such as dev servers.
    // They should not make Codex look busy once the foreground turn is idle.
    let screen = "background terminal running · /ps to view · /stop to close\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "llm-proxy", "");
    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
}

#[test]
fn codex_screen_working_fallback_handles_static_osc_title() {
    let screen = "• I’ll run it and wait for completion.\n\n\
        ◦ Working (1m 16s • esc to interrupt) · 1 background…\n\n\
        › Use /skills to list available skills\n\n\
        gpt-5.6-sol default · /work\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("screen_working_fallback")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_osc_working_remains_preferred_over_screen_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\n\
        › Use /skills to list available skills\n\n\
        gpt-5.6-sol default · /work\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "⠸ project", "");

    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_current_prompt_keeps_weak_text_from_overriding_working_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\
        do you want to continue? [y/n]\n\
        › Use /skills to list available skills\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("screen_working_fallback")
    );
    assert!(result.visible_working);
}

#[test]
fn codex_weak_blocker_ignores_finished_response_above_current_prompt() {
    let screen = "• The `wt rm` transcript now shows [y/N] / esc, matching the real prompt.\n\n\
        ─ Worked for 4m 59s ─\n\n\
        › Ask Codex to do anything\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
}

#[test]
fn codex_weak_blocker_ignores_wrapped_current_prompt_text() {
    let screen = "› Explain why this prompt wraps before quoting the confirmation text\n\
          [y/N] / esc and whether the docs should include it\n\n\
          gpt-5.6-sol default · /work\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
}

#[test]
fn codex_transcript_viewer_outranks_working_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\
        › transcript\n\
        ↑/↓ to scroll · pgup/pgdn to move · home/end to jump · q to quit · esc to edit prev\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Unknown);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("transcript_viewer")
    );
    assert!(result.skip_state_update);
    assert!(!result.visible_working);
}

#[test]
fn codex_screen_working_fallback_ignores_stale_and_prompt_text() {
    let screens = [
        "◦ Working (1m 16s • esc to interrupt)\n\
         ■ Conversation interrupted\n\
         › Use /skills to list available skills\n\
         gpt-5.6-sol default · /work\n",
        "› Explain the text ◦ Working (1m 16s • esc to interrupt)\n\
         gpt-5.6-sol default · /work\n",
        "  ◦ Working (1m 16s • esc to interrupt)\n\
         › Use /skills to list available skills\n\
         gpt-5.6-sol default · /work\n",
    ];

    for screen in screens {
        let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");
        assert_eq!(result.state, AgentState::Idle);
        assert_eq!(
            result.matched_rule.as_ref().map(|r| r.id.as_str()),
            Some("osc_title_idle")
        );
        assert!(result.visible_idle);
        assert!(!result.visible_working);
    }
}

#[test]
fn codex_screen_working_fallback_ignores_interrupted_short_terminal() {
    let screen = "◦ Working (1m 16s • esc to interrupt)\n\
        ■ Conversation interrupted\n\
        ›\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Idle);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_idle")
    );
    assert!(result.visible_idle);
    assert!(!result.visible_working);
}

#[test]
fn codex_osc_working_beats_weak_blocker_screen() {
    // A native working title remains authoritative over prompt-like text in
    // the screen-only manifest projection.
    let screen = "do you want to continue? [y/n]\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "⠋ llm-proxy", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
}

fn detect_startup_screen_with_osc(
    agent: AgentKind,
    screen: &str,
    title: &str,
    progress: &str,
) -> AgentDetection {
    detect_with_launch_context(
        agent,
        DetectionInput {
            screen,
            osc_title: title,
            osc_progress: progress,
        },
        true,
    )
}

#[test]
fn staffing_codex_startup_prompt_is_ready_even_with_a_stale_working_title() {
    let screen = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · ~/work/libtv-app-prd-canvasbar\n  ? for shortcuts                       ⚠ 1 warning · f2 to view\n";
    let result = detect_startup_screen_with_osc(AgentKind::Codex, screen, "⠋ Codex", "");
    assert_eq!(
        result.state,
        AgentState::Idle,
        "a live startup prompt is readiness, not working"
    );
    assert!(result.visible_idle);
}

#[test]
fn staffing_codex_startup_idle_requires_live_complete_footer_and_preserves_blockers() {
    let prompt = "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · ~/work/libtv-app\n  ? for shortcuts                       ⚠ 1 warning · f2 to view\n";
    for screen in [
        format!("{prompt}• Working (1s · esc to interrupt)\n"),
        format!("• Working (1s · esc to interrupt)\n{prompt}"),
        "› Ask Codex to do anything\n  ? for shortcuts\n".to_owned(),
    ] {
        assert_eq!(
            detect_startup_screen_with_osc(AgentKind::Codex, &screen, "⠋ Codex", "").state,
            AgentState::Working
        );
    }
    let blocked = detect_startup_screen_with_osc(AgentKind::Codex, prompt, "Action Required", "");
    assert_eq!(blocked.state, AgentState::Blocked);
    assert!(blocked.visible_blocker);
}

#[test]
fn codex_startup_prompt_with_the_skills_placeholder_is_ready_with_a_stale_working_title() {
    let screen = "› Use /skills to list available skills\n  GPT-6.1-Sol xhigh · ~/work/libtv-app\n  ? for shortcuts\n";
    let result = detect_startup_screen_with_osc(AgentKind::Codex, screen, "⠋ Codex", "");
    assert_eq!(
        result.state,
        AgentState::Idle,
        "the other empty-composer placeholder is the same live startup prompt"
    );
}

#[test]
fn startup_footer_does_not_override_a_spinner_outside_managed_launch() {
    let screen =
        "› Ask Codex to do anything\n  GPT-6.1-Sol xhigh · ~/work/libtv-app\n  ? for shortcuts\n";
    assert_eq!(
        detect_screen_with_osc(AgentKind::Codex, screen, "⠋ Codex", "").state,
        AgentState::Working
    );
}

#[test]
fn startup_readiness_accepts_a_bare_composer_but_preserves_a_typed_draft() {
    for (body, expected) in [
        ("", AgentState::Idle),
        ("inspect my changes", AgentState::Working),
    ] {
        let screen = format!("› {body}\n  GPT-6.1-Sol xhigh · /repo\n  ? for shortcuts\n");
        assert_eq!(
            detect_startup_screen_with_osc(AgentKind::Codex, &screen, "⠋ Codex", "").state,
            expected
        );
    }
}

#[test]
fn codex_captured_0_164_footer_confirms_startup_and_preserves_negative_controls() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    // Captured from the real v0.164.0 launch that failed STAFF-01. The blank
    // composer/model row is provider chrome, not an incomplete footer.
    let captured = include_str!("../../../../tests/fixtures/codex-startup/empty-footer-0.164.txt");
    for screen in [captured.to_owned(), captured.replacen("\n\n", "\n", 1)] {
        let result = detect_startup_screen_with_osc(AgentKind::Codex, &screen, "⠋ Codex", "");
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(result.visible_idle, "{screen}");
    }
    for screen in [
        captured.replace("Ask Codex to do anything", "inspect my changes"),
        format!("• Working (1s · esc to interrupt)\n{captured}"),
        format!("{captured}• Working (1s · esc to interrupt)\n"),
    ] {
        let result = detect_startup_screen_with_osc(AgentKind::Codex, &screen, "⠋ Codex", "");
        assert_eq!(result.state, AgentState::Working, "{screen}");
        assert!(!result.visible_idle, "{screen}");
    }
    let blocked = detect_startup_screen_with_osc(AgentKind::Codex, captured, "Action Required", "");
    assert_eq!(blocked.state, AgentState::Blocked);
    assert!(blocked.visible_blocker);
}
