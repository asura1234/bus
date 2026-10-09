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
fn claude_blocker_with_background_shell_remains_blocked() {
    let screen = concat!(
        "do you want to proceed?\n",
        "bash command: rm -rf /tmp/test\n",
        "❯ 1. Yes\n",
        "  2. No\n\n",
        "Esc to cancel · Tab to amend · ctrl+e to explain\n",
        "  ⏵⏵ auto mode on · 1 shell · ← for agents\n",
    );
    let result = detect_screen_with_osc(AgentKind::Claude, screen, "", "");

    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("bash_permission_prompt")
    );
    assert!(result.visible_blocker);
    assert!(!result.visible_working);
}

#[test]
fn claude_bash_prompt_with_dont_ask_again_option_matches_bash_rule() {
    // Captured from a Bash approval prompt at its resting cursor position. The
    // "don't ask again" choice pushes No to option 3, so the only cursor-free
    // option line is one bash_permission_prompt used not to cover, which let
    // the narrower generic_permission_prompt claim the prompt instead (#2650).
    let screen = concat!(
        "────────────────────────────────────────────────────────────────
",
        " Bash command

",
        "   curl -sS -o /tmp/probe.html https://example.com
",
        "   Download example.com to /tmp/probe.html

",
        " This command requires approval

",
        " Do you want to proceed?
",
        " ❯ 1. Yes
",
        "   2. Yes, and don't ask again for: curl *
",
        "   3. No

",
        " Esc to cancel · Tab to amend · ctrl+e to explain
",
    );
    let result = detect_screen_with_osc(AgentKind::Claude, screen, "", "");

    assert_eq!(result.state, AgentState::Blocked);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("bash_permission_prompt")
    );
    assert!(result.visible_blocker);
}

#[test]
fn claude_permission_prompt_matches_at_every_cursor_position() {
    // The selected option carries "❯", so no option branch may assume its line
    // is cursor-free. Walk the cursor across both option layouts.
    let layouts: [&[&str]; 2] = [
        &[" ❯ 1. Yes", "   2. No"],
        &[
            " ❯ 1. Yes",
            "   2. Yes, and don't ask again for: curl *",
            "   3. No",
        ],
    ];

    for layout in layouts {
        for selected in 0..layout.len() {
            let options: Vec<String> = layout
                .iter()
                .enumerate()
                .map(|(index, line)| {
                    let bare = line.trim_start().trim_start_matches('❯').trim_start();
                    if index == selected {
                        format!(" ❯ {bare}")
                    } else {
                        format!("   {bare}")
                    }
                })
                .collect();
            let screen = format!(
                concat!(
                    "────────────────────────────────────────────────────────────────
",
                    " Bash command

",
                    "   curl -sS https://example.com

",
                    " Do you want to proceed?
",
                    "{}

",
                    " Esc to cancel · Tab to amend · ctrl+e to explain
",
                ),
                options.join("\n"),
            );
            let result = detect_screen_with_osc(AgentKind::Claude, &screen, "", "");

            assert_eq!(
                result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
                Some("bash_permission_prompt"),
                "{options:?} selected={selected}"
            );
            assert_eq!(result.state, AgentState::Blocked, "selected={selected}");
            assert!(result.visible_blocker, "selected={selected}");
        }
    }
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
    // falls back to idle; blocked screen rules can win when present.
    let result = detect_screen_with_osc(AgentKind::Claude, "", "", "4;3;");
    assert_eq!(result.state, AgentState::Idle);
    assert!(!result.visible_working);
}

#[test]
fn claude_blocker_screen_outranks_stale_osc_progress() {
    // Regression: progress 4;3 persists during permission prompts. The
    // blocked form on screen must win because no rule treats 4;3 as working.
    let blocker_screen =
        "──────────\n  1. Yes\n  2. No\n\nEnter to select · ↑/↓ to navigate · Esc to cancel\n";
    let result = detect_screen_with_osc(AgentKind::Claude, blocker_screen, "✳ Task title", "4;3;");
    assert_eq!(result.state, AgentState::Blocked);
    assert!(result.visible_blocker);
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
fn claude_blocker_screen_outranks_osc_idle_title() {
    // When the OSC title shows ✳ (idle) but the screen has a bash permission
    // prompt, the blocked rule at priority 850 beats osc_title_idle at 250.
    let blocker_screen = "do you want to proceed?\n\
        bash command: rm -rf /tmp/test\n\
        ❯ 1. Yes\n   2. No\n\n\
        Esc to cancel · Tab to amend · ctrl+e to explain\n";
    let result = detect_screen_with_osc(AgentKind::Claude, blocker_screen, "✳ Claude Code", "");
    assert_eq!(result.state, AgentState::Blocked);
    assert!(result.visible_blocker);
}

#[test]
fn claude_mcp_elicitation_is_blocked() {
    // Regression for issue #3283: an MCP elicitation dialog has Accept/Decline
    // controls and an "Esc to cancel" footer but no Enter hint, so no blocked
    // rule matched and the static OSC title reported idle.
    // Live capture uses curly quotes around the server name; the issue report
    // transcribed straight quotes. Both must classify as blocked.
    for screen in [
        "MCP server \u{201c}my-server\u{201d} requests your input\n\nGrant temporary access to the demo gateway for 15 minutes?\n\n\u{276f} Accept    Decline\n\nEsc to cancel \u{b7} \u{2191}/\u{2193} to navigate\n",
        "MCP server \"my-server\" requests your input\n\nserver-supplied message\n\n\u{276f} Accept    Decline\n\nEsc to cancel \u{b7} \u{2191}/\u{2193} to navigate\n",
    ] {
        let result = with_manifest_dirs("claude-mcp-elicitation", || {
            detect_screen_with_osc(AgentKind::Claude, screen, "\u{2733} Claude Code", "")
        });
        assert_eq!(result.state, AgentState::Blocked, "{result:#?}");
        assert!(result.visible_blocker, "{result:#?}");
        assert_eq!(
            result.matched_rule.as_ref().map(|r| r.id.as_str()),
            Some("mcp_elicitation_prompt"),
            "{result:#?}"
        );
    }
}

#[test]
fn claude_empty_osc_empty_screen_is_idle_fallback() {
    // No OSC data, no matching screen rule → fallback idle (unchanged V3 behavior)
    let result = detect_screen_with_osc(AgentKind::Claude, "", "", "");
    assert_eq!(result.state, AgentState::Idle);
    assert!(!result.visible_idle);
}

// --- Codex OSC rules ---
