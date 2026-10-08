use super::*;

// Reduced to the live control lines captured from Cursor Agent
// v2026.09.10-fd3934a, pane w1:p8, on 2026-09-13. Question wording and the
// transcript above the chooser are deliberately not detection invariants.
const CURSOR_QUESTION_CONTROLS: &str = concat!(
    " │   › [ ] Continue │\n",
    " │     [ ] Cancel │\n",
    " │     [ ] Other: (type to answer) │\n",
    " │ │\n",
    " │ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n",
    " └────────────────────────────────────────────────────────────────────────────────┘\n",
);

fn cursor_question_detection(screen: &str) -> Detected {
    let loaded = bundled_loaded_manifest(Agent::Cursor, bundled_manifest(Agent::Cursor).unwrap());
    detect_loaded(
        DetectionInput {
            screen,
            osc_title: "",
            osc_progress: "",
        },
        &loaded,
    )
}

#[test]
fn devin_manifest_detects_idle_working_and_blocked_states() {
    let idle = detect_screen(
        Agent::Devin,
        "─────────────────────────────────────────────────────\n❭ Ask Devin to build features, fix bugs, or work on\n  your code\n─────────────────────────────────────────────────────\nSWE-1.6               Context: 16k / 200k tokens (7%)",
    );
    assert_eq!(idle.state, AgentState::Idle);
    assert!(idle.visible_idle);

    let live_footer_idle = detect_screen(
        Agent::Devin,
        "Done.\n\n────────────────────────────────────────────────── (bypass permissions on) ─\n❭\n────────────────────────────────────────────────────────────────────────────\nClaude Opus 4.6 Thinking                                    Context: 38k / 200k tokens (18%)",
    );
    assert_eq!(live_footer_idle.state, AgentState::Idle);
    assert_eq!(
        live_footer_idle
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("live_prompt_footer")
    );
    assert!(live_footer_idle.visible_idle);

    let welcome_footer_idle = detect_screen(
        Agent::Devin,
        "⠀⠀⠀⠀⠀⣴⣾⣶⡄⠀⠀⠀⠀\n⠀⣴⣾⣶⡾⠛⠿⠟⠃⣴⣾⣶⡄  Devin CLI\n⠀⠛⠿⠟⠃⣴⣾⣶⡾⠛⠿⠟⠃  v2026.5.26-8\n⠀⣤⣶⣦⡄⠻⢿⠿⢷⣤⣶⣦⡄\n⠀⠻⢿⠿⢷⣤⣶⣦⡄⠻⢿⠿⠃  Hybrid\n⠀⠀⠀⠀⠀⠻⢿⠿⠃⠀⠀⠀⠀\n\n───────────────────────────\n❭ Ask Devin to build\n  features, fix bugs, or\n  work on your code\n───────────────────────────\nClaude Opus Looking for\n4.6 Thinkingplan mode? /\n            plan",
    );
    assert_eq!(welcome_footer_idle.state, AgentState::Idle);
    assert_eq!(
        welcome_footer_idle
            .matched_rule
            .as_ref()
            .map(|rule| rule.id.as_str()),
        Some("welcome_prompt_footer")
    );
    assert!(welcome_footer_idle.visible_idle);

    let working = detect_screen(
        Agent::Devin,
        "◔ Reading shell 91b655\n  │ Timeout: 35s\n\n⠀⡆ Running tools · 27s (esc to interrupt)\n─────────────────────────────────────────────────────\n❭ Guide Devin while it works",
    );
    assert_eq!(working.state, AgentState::Working);
    assert!(working.visible_working);

    let trust_prompt = detect_screen(
        Agent::Devin,
        "Do you trust the authors of this directory?\nFor security, devin should not be run in directories\nwith untrusted content.\n❭ 1 Yes, trust /private/tmp/devin-hook-probe\n· 2 No, exit",
    );
    assert_eq!(trust_prompt.state, AgentState::Blocked);
    assert!(trust_prompt.visible_blocker);

    let permission_prompt = detect_screen(
        Agent::Devin,
        "⏺ Running command\n  └ $ sleep 30\n\n❭ 1 Yes  (Approve once)\n· 2 Yes, allow `sleep` commands\n· 3 Yes, always allow `sleep` commands\n· 4 No\n↑↓ select · ↵ confirm · esc cancel",
    );
    assert_eq!(permission_prompt.state, AgentState::Blocked);
    assert!(permission_prompt.visible_blocker);
}

#[test]
fn muse_manifest_requires_complete_live_controls() {
    let working = detect_screen(
        Agent::Muse,
        "⟩ hello\n\n◆ Working (0s · esc to interrupt)\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(working.state, AgentState::Working);
    assert!(working.visible_working);

    let picker = detect_screen(
        Agent::Muse,
        "Which option should I use?\n\n› 1. Alpha\n  2. Beta\n\nEnter to select · ↑/↓ to move · Tab for an optional note · Esc to interrupt\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(picker.state, AgentState::Blocked);
    assert!(picker.visible_blocker);

    let command_approval = detect_screen(
        Agent::Muse,
        "Would you like to run the following command?\n\n$ printf muse-safe-probe\n\n› 1. Allow this stage once (y)\n  2. Always allow in this workspace: printf muse-safe-probe ... (p)\n  3. Abort the entire command (esc)\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(command_approval.state, AgentState::Blocked);
    assert!(command_approval.visible_blocker);

    let network_approval = detect_screen(
        Agent::Muse,
        "network: example.com:443 https\nrequested by:\n$ curl -fsS https://example.com\n\n› 1. Yes, proceed (y)\n  2. Yes, don't ask again this session (p)  example.com:443 (https)\n  3. No, and tell Muse Code what to do differently (esc)\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(network_approval.state, AgentState::Blocked);
    assert!(network_approval.visible_blocker);

    let menu = detect_screen(
        Agent::Muse,
        "Theme\n\n⟩ Default (active)\n  Dynamic\n\n↑↓ move · enter save · esc go back",
    );
    assert_eq!(menu.state, AgentState::Unknown);
    assert!(menu.skip_state_update);
    assert!(!menu.visible_blocker);

    let ordinary_reply = detect_screen(
        Agent::Muse,
        "⟩ say the phrase\n\n◆ Yes, proceed\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(ordinary_reply.state, AgentState::Idle);
    assert!(ordinary_reply.visible_idle);
}

#[test]
fn cursor_native_question_controls_are_a_visible_blocker() {
    let result = cursor_question_detection(CURSOR_QUESTION_CONTROLS);
    assert_eq!(result.state, AgentState::Blocked);
    assert!(result.visible_blocker);
    assert!(!result.visible_working);
    assert_eq!(
        result.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("question_prompt")
    );
}

#[test]
fn cursor_question_blocker_requires_both_options_and_navigation_controls() {
    let screens = [
        CURSOR_QUESTION_CONTROLS.replace("[ ]", "option"),
        CURSOR_QUESTION_CONTROLS.replace("Space select", "Selection finished"),
        "Question 1 of 1: Continue or Cancel?\n".to_owned(),
        "Explain ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip\n"
            .to_owned(),
    ];
    for screen in screens {
        let result = cursor_question_detection(&screen);
        assert_eq!(result.state, AgentState::Idle, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
    }
}

#[test]
fn cursor_previous_question_does_not_block_current_prompt_or_working_turn() {
    for (suffix, expected) in [
        ("\n > Ask anything\n", AgentState::Idle),
        ("\n ctrl+c to stop\n", AgentState::Working),
    ] {
        let screen = format!("{CURSOR_QUESTION_CONTROLS}{suffix}");
        let result = cursor_question_detection(&screen);
        assert_eq!(result.state, expected, "{screen}");
        assert!(!result.visible_blocker, "{screen}");
    }
}
