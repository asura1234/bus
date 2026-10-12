use super::*;

// Live Codex 0.154.0 controls captured from w1:pE (startup) and w1:pB
// (Hooks table). Keep the tests about active controls, not hook counts or
// surrounding transcript content.
const CODEX_HOOKS_STARTUP: &str = concat!(
    "  Hooks need review\n",
    "  3 hooks are new or changed.\n",
    "  Hooks can run outside the sandbox after you trust them.\n\n",
    "› 1. Review hooks\n",
    "  2. Trust all and continue\n",
    "  3. Continue without trusting (hooks won't run)\n\n",
    "  Press enter to confirm or esc to go back\n",
);

const CODEX_HOOKS_TABLE: &str = concat!(
    "  Hooks\n",
    "  Lifecycle hooks from config and enabled plugins.\n\n",
    "  ⚠ 3 hooks need review before they can run.\n\n",
    "  Event                 Installed   Active      Review      Description\n",
    "  SessionStart          3           2           1           When a new session starts\n",
    "  Stop                  4           3           1           Right before Codex ends its turn\n\n",
    "  Press t to trust all; enter to review hooks; esc to close\n",
);

// Same live table after the three Bus callbacks were reviewed and trusted.
const CODEX_HOOKS_TRUSTED_TABLE: &str = concat!(
    "  Hooks\n",
    "  Lifecycle hooks from config and enabled plugins.\n\n",
    "  Event                 Installed   Active      Description\n",
    "  SessionStart          3           3           When a new session starts\n",
    "  Stop                  4           4           Right before Codex ends its turn\n\n",
    "  Press enter to view hooks; esc to close\n",
);

fn codex_hooks_detection(screen: &str) -> Detected {
    let loaded = bundled_loaded_manifest(
        AgentKind::Codex,
        bundled_manifest(AgentKind::Codex).unwrap(),
    );
    detect_loaded(
        DetectionInput {
            screen,
            osc_title: "bus",
            osc_progress: "",
        },
        &loaded,
    )
}

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
fn codex_trust_directory_requires_live_chooser() {
    let screen = "> You are in C:\\Users\\user\\project\n\n\
        Do you trust the contents of this\n\
        directory? Working with untrusted\n\
        contents comes with higher risk of\n\
        prompt injection. Trusting the\n\
        directory allows project-local config,\n\
        hooks, and exec policies to load.\n\n\
        › 1. Yes, continue\n\
          2. No, quit\n\n\
        Press enter to continue\n";
    // The live bottom buffer can retain the shell launch title above the chooser.
    for prefix in ["", "codex\n~/work/bus\n\n✗ codex\n"] {
        let live = format!("{prefix}{screen}");
        let result = detect_screen_with_osc(AgentKind::Codex, &live, "project", "");
        assert_eq!(result.state, AgentState::Blocked, "prefix: {prefix:?}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("trust_directory")
        );
        assert!(result.visible_blocker);
    }

    for inactive in [
        format!("{screen}\n› New prompt\n"),
        screen.replace("› 1. Yes, continue\n", ""),
        screen.replace("2. No, quit\n", ""),
        screen.replace("Press enter to continue\n", ""),
    ] {
        let result = detect_screen_with_osc(AgentKind::Codex, &inactive, "project", "");
        assert_eq!(result.state, AgentState::Idle);
        assert!(!result.visible_blocker);
    }

    let transcript = "› > You are in C:\\Users\\user\\project\n\n\
        Do you trust the contents of this\n\
        directory? Working with untrusted contents comes with higher risk.\n";
    let result = detect_screen_with_osc(AgentKind::Codex, transcript, "project", "");

    assert_eq!(result.state, AgentState::Idle);
    assert_ne!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("trust_directory")
    );
    assert!(!result.visible_blocker);
}

#[test]
fn codex_startup_update_requires_complete_live_chooser() {
    let chooser = "Update available! 0.153.0 -> 9.8.7\n\
        Run bun add -g @openai/codex to update.\n\n\
        › 1. Update now\n\
          2. Skip until next version\n\n\
        Press enter to continue   \n";
    let wrapped = "✨ Update available! 0.153.0\n\n\
        Release notes: https://example\n\n\
        › 1. Update now (runs `npm\n\
             install -g\n\
             @openai/codex`)\n\
          2. Skip\n\
          3. Skip until next\n\
             version\n\n\
        Press enter to continue\n";

    for screen in [chooser, wrapped] {
        let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");
        assert_eq!(result.state, AgentState::Blocked);
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("startup_update")
        );
        assert!(result.visible_blocker);
    }

    for screen in [
        chooser.replace("Update now", "Install"),
        format!("{wrapped}\n› Ask Codex to do anything\n"),
    ] {
        let result = detect_screen_with_osc(AgentKind::Codex, &screen, "project", "");
        assert_eq!(result.state, AgentState::Idle);
        assert_ne!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("startup_update")
        );
        assert!(!result.visible_blocker);
    }
}

#[test]
fn codex_0_162_update_chooser_blocks_and_asks_bus_to_update() {
    // Codex 0.162 dropped the "!" and the "Press enter to continue" footer.
    // Missing it left the agent Idle, so a queued message's Enter picked
    // "Update now" and Codex exited under the message.
    let chooser = include_str!("../../../../tests/fixtures/codex-update/chooser-0.162.txt");
    let result = detect_screen_with_osc(AgentKind::Codex, chooser, "project", "");
    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("startup_update")
    );
    assert!(result.visible_blocker);
    let update = auto_update(AgentKind::Codex, chooser).unwrap();
    assert_eq!(update.choose, "Update now");
    assert_eq!(update.success, "Update ran successfully");

    for screen in [
        format!("{chooser}\n› Ask Codex to do anything\n"),
        chooser.replace("Update now", "Install"),
    ] {
        let result = detect_screen_with_osc(AgentKind::Codex, &screen, "project", "");
        assert_ne!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("startup_update")
        );
        assert!(auto_update(AgentKind::Codex, &screen).is_none());
    }
    assert!(auto_update(AgentKind::Codex, "› Ask Codex to do anything\n").is_none());
    assert!(auto_update(AgentKind::Claude, chooser).is_none());
}

#[test]
fn auto_update_needs_both_the_option_and_the_success_text() {
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
    assert!(parse_manifest(&valid).is_ok());
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
fn codex_screen_blocker_outranks_working_fallback() {
    let screen = "• Working (4s • esc to interrupt)\n\
        › 1. Yes, proceed\n\
        Press enter to confirm or esc to cancel\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "project", "");

    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("live_strong_blocker")
    );
    assert!(result.visible_blocker);
    assert!(!result.visible_working);
}

#[test]
fn codex_weak_blocker_without_current_prompt_is_blocked() {
    let result = detect_screen_with_osc(
        AgentKind::Codex,
        "do you want to continue? [y/n]\n",
        "project",
        "",
    );

    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("weak_blocker")
    );
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
    // A stale [y/n] on screen triggers weak_blocker at priority 600, but an
    // active braille spinner in the OSC title is priority 1050 — OSC wins.
    let screen = "do you want to continue? [y/n]\n";
    let result = detect_screen_with_osc(AgentKind::Codex, screen, "⠋ llm-proxy", "");
    assert_eq!(result.state, AgentState::Working);
    assert_eq!(
        result.matched_rule.as_ref().map(|r| r.id.as_str()),
        Some("osc_title_working")
    );
}

#[test]
fn codex_hooks_startup_chooser_blocks_at_every_selected_option() {
    for selected in 1..=3 {
        let screen = CODEX_HOOKS_STARTUP
            .replace("› 1.", "  1.")
            .replace(&format!("  {selected}."), &format!("› {selected}."));
        let result = codex_hooks_detection(&format!("codex\n~/work/bus\n✗ codex\n{screen}"));
        assert_eq!(result.state, AgentState::Blocked, "selected={selected}");
        assert!(result.visible_blocker, "selected={selected}");
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("hooks_startup_review")
        );
    }
}

#[test]
fn codex_hooks_table_with_live_controls_blocks_without_a_pending_count_gate() {
    for screen in [
        CODEX_HOOKS_TABLE.to_owned(),
        CODEX_HOOKS_TABLE.replace("  ⚠ 3 hooks need review before they can run.\n", ""),
        CODEX_HOOKS_TRUSTED_TABLE.to_owned(),
    ] {
        let result = codex_hooks_detection(&screen);
        assert_eq!(result.state, AgentState::Blocked);
        assert!(result.visible_blocker);
        assert_eq!(
            result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("hooks_table_review")
        );
    }
}

#[test]
fn codex_hooks_blockers_require_live_controls_and_ignore_previous_menus() {
    for screen in [
        CODEX_HOOKS_STARTUP,
        CODEX_HOOKS_TABLE,
        CODEX_HOOKS_TRUSTED_TABLE,
    ] {
        let inactive = format!("{screen}\n› Ask Codex to do anything\n");
        let result = codex_hooks_detection(&inactive);
        assert_eq!(result.state, AgentState::Idle);
        assert!(!result.visible_blocker);
        let working = format!("{screen}\n• Working (4s • esc to interrupt)\n");
        let result = codex_hooks_detection(&working);
        assert_eq!(result.state, AgentState::Working);
        assert!(!result.visible_blocker);
    }
    for screen in [
        CODEX_HOOKS_STARTUP.replace("  2. Trust all and continue\n", ""),
        CODEX_HOOKS_STARTUP.replace("Press enter to confirm or esc to go back", ""),
        CODEX_HOOKS_TABLE.replace("Installed   Active      Review", ""),
        CODEX_HOOKS_TABLE.replace(
            "Press t to trust all; enter to review hooks; esc to close",
            "",
        ),
        CODEX_HOOKS_TRUSTED_TABLE.replace("Installed   Active", ""),
        CODEX_HOOKS_TRUSTED_TABLE.replace("Press enter to view hooks; esc to close", ""),
        "› Explain Hooks need review and Press enter to confirm or esc to go back\n".into(),
    ] {
        let result = codex_hooks_detection(&screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
    }
}
