use super::*;

#[test]
fn kimi_captured_moon_with_a_tip_is_working() {
    let loaded =
        bundled_loaded_manifest(AgentKind::Kimi, bundled_manifest(AgentKind::Kimi).unwrap());
    for (screen, expected_rule) in [
        (
            include_str!("../../../../tests/fixtures/kimi-steer/thinking-tip.txt"),
            "moon_tip_spinner_working",
        ),
        (
            include_str!("../../../../tests/fixtures/kimi-steer/command-tip.txt"),
            "footer_running_command_working",
        ),
    ] {
        let detection = detect_loaded(
            DetectionInput {
                screen,
                osc_title: "",
                osc_progress: "",
            },
            &loaded,
        );
        assert_eq!(detection.state, AgentState::Working);
        assert!(detection.visible_working);
        assert_eq!(
            detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some(expected_rule)
        );
    }
}

#[test]
fn kimi_bare_moon_keeps_its_existing_precedence_over_command_text() {
    let screen = format!(
        "● Running a command · $ sleep 45\n🌒\n{}",
        kimi_frame("K3.1 Pro thinking: high")
    );
    let detection = kimi_detection(&screen);
    assert_eq!(detection.state, AgentState::Working);
    assert!(detection.visible_working);
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("moon_spinner_working")
    );
    assert!(!super::super::kimi_running_command_is_winner(&screen));
}

#[test]
fn kimi_240_idle_composer_is_ready_and_working_status_is_busy() {
    let loaded =
        bundled_loaded_manifest(AgentKind::Kimi, bundled_manifest(AgentKind::Kimi).unwrap());
    let idle = "No session yet — one will be created on your first message.\n╭────────────────────────────╮\n│ >                         │\n╰────────────────────────────╯\nAsk When Needed  K2.8 Preview thinking: max\ncontext: 0% (0/1M)";
    let detection = evaluate_loaded_manifest(
        DetectionInput {
            screen: idle,
            osc_title: "",
            osc_progress: "",
        },
        &loaded,
        true,
    );
    assert_eq!(detection.state, AgentState::Idle);
    assert!(detection.visible_idle);

    let working = format!("⠋ Working…\n{idle}");
    let detection = evaluate_loaded_manifest(
        DetectionInput {
            screen: &working,
            osc_title: "",
            osc_progress: "",
        },
        &loaded,
        true,
    );
    assert_eq!(detection.state, AgentState::Working);
    assert!(detection.visible_working);

    let draft = idle.replace(
        "│ >                         │",
        "│ > unowned draft           │",
    );
    let detection = evaluate_loaded_manifest(
        DetectionInput {
            screen: &draft,
            osc_title: "",
            osc_progress: "",
        },
        &loaded,
        true,
    );
    assert_eq!(detection.state, AgentState::Unknown);
    assert!(!detection.visible_idle);
}

fn kimi_frame(footer: &str) -> String {
    format!(
        "╭──────────────────────────────────────────────────╮\n│ >                                                │\n╰──────────────────────────────────────────────────╯\n{footer}\n                               context: 3% (24.4k/1M)"
    )
}

fn kimi_detection(screen: &str) -> Detected {
    let loaded =
        bundled_loaded_manifest(AgentKind::Kimi, bundled_manifest(AgentKind::Kimi).unwrap());
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
fn kimi_live_tracking_agents_running_footer_is_working() {
    let detection = kimi_detection(&kimi_frame("kimi thinking [2 agents running]"));
    assert_eq!(detection.state, AgentState::Working);
    assert!(detection.visible_working);
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("background_agent_status_working")
    );
}

#[test]
fn kimi_live_tracking_tasks_running_is_working() {
    let detection = kimi_detection(&kimi_frame(
        "Ask When Needed  K2.8 Preview thinking: max  [2 tasks running]",
    ));
    assert_eq!(
        detection.state,
        AgentState::Working,
        "matched {:?}",
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str())
    );
    assert!(detection.visible_working);
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("footer_tasks_running_working")
    );
}

#[test]
fn kimi_live_tracking_steer_footer_is_working() {
    let detection = kimi_detection(&kimi_frame("↑ to edit · ctrl-s to steer"));
    assert_eq!(
        detection.state,
        AgentState::Working,
        "matched {:?}",
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str())
    );
    assert!(detection.visible_working);
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("footer_steer_working")
    );
}

#[test]
fn kimi_live_tracking_running_a_command_is_working() {
    let detection = kimi_detection(&kimi_frame("Running a command"));
    assert_eq!(
        detection.state,
        AgentState::Working,
        "matched {:?}",
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str())
    );
    assert!(detection.visible_working);
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("footer_running_command_working")
    );
}

#[test]
fn kimi_live_tracking_running_command_above_composer_is_working() {
    let screen = "\
● Running a command · $ sleep 45\n\
╭──────────────────────────────────────────────────╮\n\
│ >                                                │\n\
╰──────────────────────────────────────────────────╯\n\
K3.1 Pro thinking: high  …/tmp/project  master\n\
                               context: 3% (24.4k/1M)";
    let detection = kimi_detection(screen);
    assert_eq!(
        detection.state,
        AgentState::Working,
        "matched {:?}",
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str())
    );
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("footer_running_command_working")
    );
}

#[test]
fn kimi_live_tracking_effort_footer_without_busy_marker_stays_idle() {
    for footer in [
        "K3.1 Pro thinking: high  …/tmp/project  master",
        "● Ran a command · $ sleep 45\nK3.1 Pro thinking: high  …/tmp/project  master",
    ] {
        let detection = kimi_detection(&kimi_frame(footer));
        assert_eq!(
            detection.state,
            AgentState::Idle,
            "{footer:?} matched {:?}",
            detection.matched_rule.as_ref().map(|rule| rule.id.as_str())
        );
        assert!(detection.visible_idle, "{footer:?}");
    }
}

#[test]
fn kimi_standalone_running_command_reply_is_idle_once_the_turn_is_not_live() {
    // Captured shape: final reply "● Running a command" above an empty composer.
    // Text alone still matches the live header. The turn wire, or a non-bold
    // line when that wire cannot be read, is what makes this Idle.
    let screen = "\
 ● Running a command\n\
╭──────────────────────────────────────╮\n\
│ >                                    │\n\
╰──────────────────────────────────────╯\n\
K3.1 Pro thinking: high  …/tmp/project  master\n\
                               context: 2% (17.9k/1M)";
    let live = kimi_detection(screen);
    assert_eq!(live.state, AgentState::Working);
    assert_eq!(
        live.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("footer_running_command_working")
    );
    assert!(super::super::kimi_running_command_is_winner(screen));
    let settled = super::super::kimi_screen_without_running_command(screen);
    assert_eq!(settled.state, AgentState::Idle);
    assert!(settled.visible_idle);

    let tasks = kimi_frame("Ask When Needed  K2.8 Preview thinking: max  [1 task running]");
    assert!(!super::super::kimi_running_command_is_winner(&tasks));
    let still_working = super::super::kimi_screen_without_running_command(&tasks);
    assert_eq!(still_working.state, AgentState::Working);
    assert!(still_working.visible_working);
}

#[test]
fn kimi_live_command_header_above_approved_and_spinner_tip_is_working() {
    // Real Kimi 2.4.1 while sleep 45 runs: the header sits above Approved
    // and a moon tip, so a 6-line footer window misses it.
    let screen = "\
 ● Running a command · $ sleep 45\n\
   Approved: Running: sleep 45\n\
  🌒 · Tip: /goal for multi-step work with a clear finish line\n\
╭──────────────────────────────────────╮\n\
│ >                                    │\n\
╰──────────────────────────────────────╯\n\
K3.1 Pro thinking: high  …/tmp/project  master\n\
                               context: 2% (12.8k/1M)";
    let detection = kimi_detection(screen);
    assert_eq!(detection.state, AgentState::Working);
    assert_eq!(
        detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
        Some("footer_running_command_working")
    );
    assert!(super::super::kimi_running_command_is_winner(screen));
    assert!(crate::agents::providers::kimi::activity::running_command_has_command_suffix(screen));
}

#[test]
fn kimi_idle_composer_with_command_phrase_in_finished_reply_stays_idle() {
    let detection = kimi_detection(&kimi_frame(
        "Running a command is the in-progress label.\n✻ Thinking 9 times\nK3.1 Pro thinking: high  …/tmp/project  master",
    ));
    assert_eq!(detection.state, AgentState::Idle);
    assert!(detection.visible_idle);
}

#[test]
fn kimi_tasks_running_footer_is_working() {
    let loaded =
        bundled_loaded_manifest(AgentKind::Kimi, bundled_manifest(AgentKind::Kimi).unwrap());
    for tasks in ["[1 task running]", "[2 tasks running]"] {
        let frame = format!(
            "╭──────────────────────────────────────────────────╮\n│ >                                                │\n╰──────────────────────────────────────────────────╯\nAsk When Needed  K2.8 Preview thinking: max  {tasks}\n                               context: 3% (24.4k/1M)"
        );
        let detection = detect_loaded(
            DetectionInput {
                screen: &frame,
                osc_title: "",
                osc_progress: "",
            },
            &loaded,
        );
        assert_eq!(detection.state, AgentState::Working, "{tasks}");
        assert!(detection.visible_working, "{tasks}");
        assert_eq!(
            detection.matched_rule.as_ref().map(|rule| rule.id.as_str()),
            Some("footer_tasks_running_working"),
            "{tasks}"
        );
    }
}

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
    let loaded = bundled_loaded_manifest(
        AgentKind::Cursor,
        bundled_manifest(AgentKind::Cursor).unwrap(),
    );
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
fn devin_manifest_detects_idle_and_working_states() {
    let idle = detect_screen(
        AgentKind::Devin,
        "─────────────────────────────────────────────────────\n❭ Ask Devin to build features, fix bugs, or work on\n  your code\n─────────────────────────────────────────────────────\nSWE-1.6               Context: 16k / 200k tokens (7%)",
    );
    assert_eq!(idle.state, AgentState::Idle);
    assert!(idle.visible_idle);

    let live_footer_idle = detect_screen(
        AgentKind::Devin,
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
        AgentKind::Devin,
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
        AgentKind::Devin,
        "◔ Reading shell 91b655\n  │ Timeout: 35s\n\n⠀⡆ Running tools · 27s (esc to interrupt)\n─────────────────────────────────────────────────────\n❭ Guide Devin while it works",
    );
    assert_eq!(working.state, AgentState::Working);
    assert!(working.visible_working);
}

#[test]
fn muse_manifest_preserves_working_viewer_and_idle_states() {
    let working = detect_screen(
        AgentKind::Muse,
        "⟩ hello\n\n◆ Working (0s · esc to interrupt)\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(working.state, AgentState::Working);
    assert!(working.visible_working);

    let menu = detect_screen(
        AgentKind::Muse,
        "Theme\n\n⟩ Default (active)\n  Dynamic\n\n↑↓ move · enter save · esc go back",
    );
    assert_eq!(menu.state, AgentState::Unknown);
    assert!(menu.skip_state_update);
    assert!(!menu.visible_blocker);

    let ordinary_reply = detect_screen(
        AgentKind::Muse,
        "⟩ say the phrase\n\n◆ Yes, proceed\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
    );
    assert_eq!(ordinary_reply.state, AgentState::Idle);
    assert!(ordinary_reply.visible_idle);
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

#[test]
fn cursor_managed_startup_requires_a_complete_empty_composer_footer() {
    let loaded = bundled_loaded_manifest(
        AgentKind::Cursor,
        bundled_manifest(AgentKind::Cursor).unwrap(),
    );
    let current =
        " → Plan, search, build anything\n\n  Grok 4.7 256K High  Run Everything\n  /workspace\n";
    let legacy = include_str!("../../../server/api/agents/tests/fixtures/cursor_empty.txt");
    let mut screens = vec![
        current.to_owned(),
        legacy.to_owned(),
        current.replace("Plan, search, build anything", "Add a follow-up"),
    ];
    screens.extend(
        [
            r"C:\Users\qa\work",
            "C:/workspace",
            r"~\work",
            r"\\server\share",
            r"C:\",
            "/",
            "~",
        ]
        .map(|path| current.replace("/workspace", path)),
    );
    for screen in screens {
        let input = DetectionInput {
            screen: &screen,
            osc_title: "",
            osc_progress: "",
        };
        let detection = evaluate_loaded_manifest(input, &loaded, true);
        assert_eq!(detection.state, AgentState::Idle, "{screen}");
        assert!(detection.visible_idle, "{screen}");
        assert!(!evaluate_loaded_manifest(input, &loaded, false).visible_idle);
    }
    for screen in [
        " → Plan, search, build anything\n".to_owned(),
        current.replace("Plan, search, build anything", "my unsubmitted draft"),
        current.replace("  /workspace\n", ""),
        current.replace("  Grok 4.7 256K High  Run Everything\n", ""),
        current.replace("/workspace", "C:workspace"),
        current.replace("/workspace", "workspace"),
        format!("{current}Loading conversation...\n"),
    ] {
        let detection = evaluate_loaded_manifest(
            DetectionInput {
                screen: &screen,
                osc_title: "",
                osc_progress: "",
            },
            &loaded,
            true,
        );
        assert_eq!(detection.state, AgentState::Unknown, "{screen}");
        assert!(!detection.visible_idle, "{screen}");
    }
    let screen = format!("{current} ctrl+c to stop\n");
    let detection = evaluate_loaded_manifest(
        DetectionInput {
            screen: &screen,
            osc_title: "",
            osc_progress: "",
        },
        &loaded,
        true,
    );
    assert_eq!(detection.state, AgentState::Working, "{screen}");
    assert!(!detection.visible_idle, "{screen}");
}

#[cfg(feature = "dev-tools")]
#[test]
fn fake_status_ignores_working_text_above_the_live_prompt() {
    let active = detect_screen(AgentKind::Fake, "fake agent working\n");
    assert_eq!(active.state, AgentState::Working);
    let idle = detect_screen(
        AgentKind::Fake,
        "fake agent working\nfake> \n  scriptable agent\n",
    );
    assert_eq!(idle.state, AgentState::Idle);
}
