use super::*;

fn captured_grid(screen: &str) -> Grid {
    let (runtime, _) = crate::terminal::TerminalRuntime::test_with_channel(180, 64);
    runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
    observe(&runtime).expect("complete captured screen").0
}

#[test]
fn driver_numbered_draft_with_context_footer_is_not_a_foreground_menu() {
    // Reduced native composer frames from the server's draft-edit regression.
    // Numbered editable text has no current action hint or enclosing menu rule.
    for draft in [
        "1. Confirm the bug\n  2. Add",
        "1. Confirm the bug\n  2. Add a regression test",
        "1. Confirm the bug\n  2. Add a regression test\n  3. Run the gates",
        "",
    ] {
        let screen = format!(
            "• Done. The parser is fixed.\n\n› {draft}\n\n  gpt-5.4 · minimal · ~/work/bus"
        );
        let grid = menu(&screen);
        assert!(blocker(&grid, false).is_none(), "{screen}");
        assert!(!has_input_controls(&grid.text()), "{screen}");
    }
}

#[tokio::test]
async fn driver_editable_composer_text_is_outside_the_control_region() {
    for text in [
        "Please print [y/n]",
        "Please print [yes/no]\n  Enter to confirm",
        "1. First choice\n  2. Second choice\n  Enter select · Esc cancel",
    ] {
        for screen in [
            format!("history\n────────────────────\n❯ {text}\n────────────────────\n? for shortcuts"),
            format!("history\n› {text}\n\nGPT-6 · /repo\n? for shortcuts"),
            format!("╭────────────────────────────────╮\n│ > {} │\n╰────────────────────────────────╯\nK2.8 Preview", text.replace('\n', " │\n│   ")),
        ] {
            let grid = captured_grid(&screen);
            assert!(!has_input_controls(&grid.text()), "literal editable text: {screen}");
            assert!(blocker(&grid, false).is_none(), "idle composer: {screen}");
            assert!(blocker(&grid, true).is_none(), "working composer: {screen}");
        }
    }
}

#[tokio::test]
async fn multiline_draft_containing_a_model_footer_line_is_not_a_control() {
    let screen =
        "history\n› saved\ngpt-4 notes\nPlease print [y/n]\n\nGPT-6 · /repo\n? for shortcuts";
    let grid = captured_grid(screen);
    assert!(
        !has_input_controls(&grid.text()),
        "draft after an interior model line is still composer text: {screen}"
    );
    assert!(blocker(&grid, false).is_none(), "{screen}");
}

#[tokio::test]
async fn driver_blocker_recognizes_saved_foreground_and_pending_controls() {
    // The six non-native Grok files are synthetic hypotheses, explicitly
    // identified by their README. They exercise generic key syntax, not live
    // authentication, provider provenance, or real keyboard acceptance.
    let cases = [
        (
            "claude-question/multi-select.txt",
            include_str!("../../../../tests/fixtures/claude-question/multi-select.txt"),
            true,
        ),
        (
            "claude-option-list/multi-select-custom-checked.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/multi-select-custom-checked.ansi"),
            true,
        ),
        (
            "claude-option-list/multi-select-empty-focused.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/multi-select-empty-focused.ansi"),
            true,
        ),
        (
            "claude-option-list/multi-select-rest.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/multi-select-rest.ansi"),
            true,
        ),
        (
            "claude-option-list/short-cursor3-scrolled.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/short-cursor3-scrolled.ansi"),
            true,
        ),
        (
            "claude-option-list/short-cursor4-scrolled.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/short-cursor4-scrolled.ansi"),
            true,
        ),
        (
            "claude-option-list/short-freetext-focused-empty.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/short-freetext-focused-empty.ansi"),
            true,
        ),
        (
            "claude-option-list/short-rest-top.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/short-rest-top.ansi"),
            true,
        ),
        (
            "claude-option-list/tabs-q1-freetext-focused.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tabs-q1-freetext-focused.ansi"),
            true,
        ),
        (
            "claude-option-list/tabs-q1-rest.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tabs-q1-rest.ansi"),
            true,
        ),
        (
            "claude-option-list/tabs-q1-typed.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tabs-q1-typed.ansi"),
            true,
        ),
        (
            "claude-option-list/tabs-q2-rest.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tabs-q2-rest.ansi"),
            true,
        ),
        (
            "claude-option-list/tabs-review.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tabs-review.ansi"),
            true,
        ),
        (
            "claude-option-list/tall-freetext-focused-empty.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tall-freetext-focused-empty.ansi"),
            true,
        ),
        (
            "claude-option-list/tall-freetext-with-text-focused.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tall-freetext-with-text-focused.ansi"),
            true,
        ),
        (
            "claude-option-list/tall-rest-freetext-holds-text.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tall-rest-freetext-holds-text.ansi"),
            true,
        ),
        (
            "claude-option-list/tall-rest.ansi",
            include_str!("../../../../tests/fixtures/claude-option-list/tall-rest.ansi"),
            true,
        ),
        (
            "codex-question/collapsed.txt",
            include_str!("../../../../tests/fixtures/codex-question/collapsed.txt"),
            true,
        ),
        (
            "codex-question/collapsed-warning.txt",
            include_str!("../../../../tests/fixtures/codex-question/collapsed-warning.txt"),
            true,
        ),
        (
            "codex-question/expanded.txt",
            include_str!("../../../../tests/fixtures/codex-question/expanded.txt"),
            true,
        ),
        (
            "codex-question/expanded-working.txt",
            include_str!("../../../../tests/fixtures/codex-question/expanded-working.txt"),
            true,
        ),
        (
            "codex-question/other-selected.txt",
            include_str!("../../../../tests/fixtures/codex-question/other-selected.txt"),
            true,
        ),
        (
            "codex-question/other-typed.txt",
            include_str!("../../../../tests/fixtures/codex-question/other-typed.txt"),
            true,
        ),
        (
            "codex-question/queued-message-hides-question.txt",
            include_str!("../../../../tests/fixtures/codex-question/queued-message-hides-question.txt"),
            false,
        ),
        (
            "grok-dialog/free-text.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/free-text.txt"),
            true,
        ),
        (
            "grok-dialog/native-1.0.70-command-approval.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/native-1.0.70-command-approval.txt"),
            true,
        ),
        (
            "grok-dialog/native-1.0.70-startup-trust.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/native-1.0.70-startup-trust.txt"),
            true,
        ),
        (
            "grok-dialog/parked-focus.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/parked-focus.txt"),
            true,
        ),
        (
            "grok-dialog/permission.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/permission.txt"),
            true,
        ),
        (
            "grok-dialog/question.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/question.txt"),
            true,
        ),
        (
            "grok-dialog/trust.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/trust.txt"),
            true,
        ),
        (
            "grok-dialog/wrapped.txt",
            include_str!("../../../../tests/fixtures/grok-dialog/wrapped.txt"),
            true,
        ),
    ];
    let mut failures = Vec::new();
    for (name, screen, expected) in cases {
        let grid = captured_grid(screen);
        let episode = blocker(&grid, false).is_some();
        let controls = has_input_controls(&grid.text());
        if episode != expected || controls != expected {
            failures.push(format!(
                "{name}: expected controls={expected}, episode={episode}, controls={controls}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn driver_working_episode_omits_only_pending_answer_shortcuts() {
    for (name, screen, blocked_while_working) in [
        (
            "collapsed pending question",
            include_str!("../../../../tests/fixtures/codex-question/collapsed.txt"),
            false,
        ),
        (
            "collapsed pending question with warning",
            include_str!("../../../../tests/fixtures/codex-question/collapsed-warning.txt"),
            false,
        ),
        (
            "foreground expanded question",
            include_str!("../../../../tests/fixtures/codex-question/expanded-working.txt"),
            true,
        ),
        (
            "foreground command approval",
            include_str!("../../../../tests/fixtures/kimi-steer/approval.txt"),
            true,
        ),
    ] {
        let grid = captured_grid(screen);
        assert!(
            has_input_controls(&grid.text()),
            "composer excludes all controls: {name}"
        );
        assert!(
            blocker(&grid, false).is_some(),
            "idle control episode: {name}"
        );
        assert_eq!(
            blocker(&grid, true).is_some(),
            blocked_while_working,
            "working episode: {name}"
        );
    }
}

#[tokio::test]
async fn captured_working_codex_screen_is_not_a_dialog() {
    let captured = include_str!("../../../../tests/fixtures/codex-activity/working-room-1.txt");
    let mut episodes = Vec::new();
    for seconds in ["2m 38s", "17m 40s"] {
        let screen = captured.replace("2m 38s", seconds);
        let grid = captured_grid(&screen);
        assert!(
            !has_input_controls(&grid.text()),
            "working chrome is not an input control: {seconds}"
        );
        let idle = blocker(&grid, false);
        let working = blocker(&grid, true);
        assert!(idle.is_none(), "idle read: {seconds} {idle:?}");
        assert!(working.is_none(), "working read: {seconds} {working:?}");
        episodes.push(working);
    }
    assert_eq!(episodes[0], episodes[1]);
}

#[tokio::test]
async fn foreground_dialog_episode_ignores_the_working_timer() {
    let screen = include_str!("../../../../tests/fixtures/codex-question/expanded-working.txt");
    let later = screen.replace("(51s", "(52s");
    let first = blocker(&captured_grid(screen), true);
    let second = blocker(&captured_grid(&later), true);
    assert!(first.is_some(), "the expanded question is one dialog");
    assert_eq!(first, second, "the timer is not a new episode");
}

#[tokio::test]
async fn a_trust_prompt_has_one_stable_episode() {
    let screen = include_str!("../../../../tests/fixtures/grok-dialog/trust.txt");
    let first = blocker(&captured_grid(screen), false);
    let second = blocker(&captured_grid(screen), false);
    assert!(first.is_some(), "a trust prompt is a dialog");
    assert_eq!(first, second);
}

#[tokio::test]
async fn driver_native_composer_mode_and_newline_keys_are_not_blocking_controls() {
    // Exact Grok 1.0.70 frame/footer contract from the real-frame submission
    // fixture in server/api/agents/tests/owned_draft_cleanup_test.rs. This is
    // a local VT regression, not a new native capture or live provider run.
    for body in [" ❯", " ❯ from bus"] {
        let screen = format!(
            "  ╭{}╮\n  │{body:<60}│\n  ╰──────── Grok 4.7 (high) · always-approve ─╯\n  Enter:send  │  Shift+Enter/Opt+Enter:newline  │  Shift+Tab:mode  │  Ctrl+.:shortcuts",
            "─".repeat(60),
        );
        let grid = captured_grid(&screen);
        assert!(
            !has_input_controls(&grid.text()),
            "normal composer footer: {screen}"
        );
        assert!(
            blocker(&grid, false).is_none(),
            "normal composer is not an input episode"
        );
        assert!(
            blocker(&grid, true).is_none(),
            "working composer is not an input episode"
        );
    }
}

#[test]
fn driver_keyboard_hints_require_input_actions_beyond_tab_or_space_keys() {
    for footer in ["Space:select", "Tab:next", "Space:toggle │ Tab:next option"] {
        let screen = format!("Choose fields\n❯ [ ] Alpha\n  [ ] Beta\n{footer}");
        assert!(has_input_controls(&screen), "input action: {footer}");
        assert!(
            blocker(&menu(&screen), false).is_some(),
            "input action: {footer}"
        );
    }
    for footer in ["Shift+Tab:mode", "Space:shortcut", "Tab:shortcuts"] {
        assert!(
            !has_input_controls(footer),
            "key name alone is not an input action: {footer}"
        );
    }
}

#[tokio::test]
async fn driver_descriptor_only_field_navigation_needs_current_checkbox_rows() {
    const HINT: &str = "Tab/Space: question │ Ctrl+x:shortcuts";
    // This remains a synthetic generic-form contract, matching the provenance
    // of grok-dialog/parked-focus.txt; no native card claim is made.
    for screen in [
        include_str!("../../../../tests/fixtures/grok-dialog/parked-focus.txt").to_owned(),
        format!("Fields\n│ [ ] Alpha │\n│ [✓] Beta │\n{HINT}"),
    ] {
        let grid = captured_grid(&screen);
        assert!(has_input_controls(&grid.text()), "checkbox form: {screen}");
        assert!(
            blocker(&grid, false).is_some(),
            "checkbox form episode: {screen}"
        );
    }
    for screen in [
        HINT.to_owned(),
        format!("Documentation lists - [ ] Alpha and - [x] Beta\n{HINT}"),
        format!("1. [ ] Only one field\n{HINT}"),
        format!("╭────────────────────────╮\n│ ❯ draft [ ] examples   │\n╰────────────────────────╯\n{HINT}"),
        format!("Old fields\n[ ] Alpha\n[x] Beta\n╭────────────────────────╮\n│ ❯ current owned draft │\n╰────────────────────────╯\n{HINT}"),
    ] {
        let grid = captured_grid(&screen);
        assert!(!has_input_controls(&grid.text()), "no current checkbox form: {screen}");
        assert!(blocker(&grid, false).is_none(), "no form episode: {screen}");
    }
}

#[test]
fn driver_blocker_does_not_promote_transcript_or_editable_list_to_controls() {
    for screen in [
        "❯ Please write this list:\n  1. Alpha\n  2. Beta",
        "Earlier menu\n❯ 1. Alpha\n  2. Beta\nEnter to toggle · Esc to cancel\n\n> Current editable draft",
        "Earlier menu\n❯ 1. Alpha\n  2. Beta\n\n> Current editable draft",
        "I will explain why Enter selects and Esc cancels.",
        "Queued follow-up inputs\n  ↳ queued via tab\n    ⇧← edit last queued message\n\n› Ask a question\n? for shortcuts",
        "• Working\n↑/↓ to scroll · pgup/pgdn to move · home/end to jump · q to quit · esc to edit prev",
    ] {
        let grid = menu(screen);
        assert!(blocker(&grid, false).is_none(), "{screen}");
        assert!(!has_input_controls(screen), "{screen}");
    }
}

// Independent semantic observation: the text/options were captured via the old
// native API; blank spacing and final hint in the .txt are replay scaffolding.
// See tests/fixtures/provider-ui/README.md. This is an opaque generic content
// identity consumed by policy, not a provider parser or caller revision guard.
const LATENCY: &str =
    include_str!("../../../../tests/fixtures/provider-ui/claude-extra-thought.txt");
const LATENCY_ID: &str = "33415108d1173ade22a8d5b1cf5b92e1e687caa413866bc2071184578a2b8b14";

#[test]
fn driver_blocker_menu_identity_matches_independent_content_oracle() {
    assert_eq!(blocker(&menu(LATENCY), false).as_deref(), Some(LATENCY_ID));
}

#[test]
fn driver_blocker_menu_identity_survives_optional_hint_wraps_and_focus() {
    let without_hint = LATENCY.replace("Enter to select · Esc to cancel", "");
    let wrapped = LATENCY
        .replace("rather not wait, retry", "rather not wait,\n retry")
        .replace("handling complex requests.", "handling\n complex requests.")
        .replace(
            "Retry with a faster model",
            "Retry with a faster\n     model",
        );
    let moved = LATENCY.replace("❯ 1.", "  1.").replace("  2.", "❯ 2.");
    let alternate_hint = LATENCY.replace(
        "Enter to select · Esc to cancel",
        "↑/↓ navigate · ↵ confirm · Esc cancel",
    );
    let wrapped_without_hint = wrapped.replace("Enter to select · Esc to cancel", "");
    let padded = format!(
        "│ {} │\n",
        without_hint.lines().collect::<Vec<_>>().join(" │\n│ ")
    );
    for (name, screen) in [
        ("no replay hint", without_hint),
        ("physical wraps", wrapped),
        ("physical wraps without replay hint", wrapped_without_hint),
        ("focus moved", moved),
        ("alternate keyboard hint", alternate_hint),
        ("border padding", padded),
    ] {
        assert_eq!(
            blocker(&menu(&screen), false).as_deref(),
            Some(LATENCY_ID),
            "{name}: {screen}"
        );
    }
}

#[test]
fn driver_blocker_menu_identity_excludes_unrelated_history_above_layout_boundary() {
    for prefix in [
        "Earlier request completed.\n\n",
        "• Working (17s • esc to interrupt)\n────────────────────────────────\n",
        "Earlier transcript paragraph\nwith a wrapped explanation.\n\n",
        "Old question\n  1. Alpha\n  2. Beta\n\n",
    ] {
        let screen = format!("{prefix}{LATENCY}");
        assert_eq!(
            blocker(&menu(&screen), false).as_deref(),
            Some(LATENCY_ID),
            "history above a blank/separator boundary is outside this menu: {screen}"
        );
    }
}

#[test]
fn driver_blocker_menu_identity_changes_with_title_body_or_options() {
    let original = blocker(&menu(LATENCY), false).expect("visible controls");
    for changed in [
        LATENCY.replace("little extra thought", "different extra thought"),
        LATENCY.replace("less capable", "more capable"),
        LATENCY.replace("Dismiss and keep waiting", "Dismiss and stop waiting"),
        LATENCY.replace("  3. Learn more\n", ""),
    ] {
        let next = blocker(&menu(&changed), false).expect("changed content still has controls");
        assert_ne!(
            next, original,
            "changed menu must be a distinct episode: {changed}"
        );
    }
}

#[test]
fn driver_blocker_non_menu_identity_keeps_prompt_and_pending_context() {
    for (first, second) in [
        ("Replace the cache? [y/n]", "Replace the project? [y/n]"),
        (
            "Which file should be used?\n? 1 question\n⇧← to answer",
            "Which model should be used?\n? 1 question\n⇧← to answer",
        ),
    ] {
        let first = blocker(&menu(first), false).expect("input controls");
        let second = blocker(&menu(second), false).expect("input controls");
        assert_ne!(
            first, second,
            "non-menu prompt context is part of its episode"
        );
    }
}

fn captured_manifest_controls() -> Vec<(&'static str, crate::agents::AgentKind, String)> {
    use crate::agents::AgentKind;
    // Source provenance: manifest/tests/codex_test.rs identifies these as live
    // Codex 0.154.0 startup/table captures; wording/counts are not invariants.
    let hooks_startup = concat!(
        "  Hooks need review\n",
        "  3 hooks are new or changed.\n",
        "  Hooks can run outside the sandbox after you trust them.\n\n",
        "› 1. Review hooks\n",
        "  2. Trust all and continue\n",
        "  3. Continue without trusting (hooks won't run)\n\n",
        "  Press enter to confirm or esc to go back\n",
    );
    let hooks_table = concat!(
        "  Hooks\n",
        "  Lifecycle hooks from config and enabled plugins.\n\n",
        "  ⚠ 3 hooks need review before they can run.\n\n",
        "  Event                 Installed   Active      Review      Description\n",
        "  SessionStart          3           2           1           When a new session starts\n",
        "  Stop                  4           3           1           Right before Codex ends its turn\n\n",
        "  Press t to trust all; enter to review hooks; esc to close\n",
    );
    let trusted_hooks = concat!(
        "  Hooks\n",
        "  Lifecycle hooks from config and enabled plugins.\n\n",
        "  Event                 Installed   Active      Description\n",
        "  SessionStart          3           3           When a new session starts\n",
        "  Stop                  4           4           Right before Codex ends its turn\n\n",
        "  Press enter to view hooks; esc to close\n",
    );
    // manifest/tests/others_test.rs records these live Cursor 2026.09.10
    // control lines. No provider heading or exact answer word is required.
    let cursor_question = concat!(
        " │   › [ ] Continue │\n",
        " │     [ ] Cancel │\n",
        " │     [ ] Other: (type to answer) │\n",
        " │ │\n",
        " │ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n",
        " └────────────────────────────────────────────────────────────────────────────────┘\n",
    );
    let mut cases = vec![
        (
            "codex hooks startup",
            AgentKind::Codex,
            hooks_startup.to_owned(),
        ),
        (
            "codex hooks review table",
            AgentKind::Codex,
            hooks_table.to_owned(),
        ),
        (
            "codex trusted hooks table",
            AgentKind::Codex,
            trusted_hooks.to_owned(),
        ),
        (
            "cursor captured checkbox controls",
            AgentKind::Cursor,
            cursor_question.to_owned(),
        ),
        (
            "codex captured update chooser",
            AgentKind::Codex,
            include_str!("../../../../tests/fixtures/codex-update/chooser-0.162.txt").to_owned(),
        ),
        (
            "kimi captured shell approval",
            AgentKind::Kimi,
            include_str!("../../../../tests/fixtures/kimi-steer/approval.txt").to_owned(),
        ),
    ];
    // manifest/tests/claude_test.rs records the live MCP control shape with
    // curly quotes and its issue transcription with straight quotes.
    for (name, title) in [
        (
            "claude captured MCP",
            "MCP server “my-server” requests your input",
        ),
        (
            "claude MCP transcription",
            "MCP server \"my-server\" requests your input",
        ),
    ] {
        cases.push((name, AgentKind::Claude, format!("{title}\n\nGrant temporary access to the demo gateway for 15 minutes?\n\n❯ Accept    Decline\n\nEsc to cancel · ↑/↓ to navigate\n")));
    }
    // Captured Bash permission layout from manifest/tests/claude_test.rs:
    // preserve both numbered-option layouts at every focused position.
    for (name, options) in [
        ("claude two-option permission", vec!["1. Yes", "2. No"]),
        (
            "claude three-option permission",
            vec!["1. Yes", "2. Yes, and don't ask again for: curl *", "3. No"],
        ),
    ] {
        for focused in 0..options.len() {
            let rows = options
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    format!("{} {label}", if index == focused { "❯" } else { " " })
                })
                .collect::<Vec<_>>()
                .join("\n");
            cases.push((name, AgentKind::Claude, format!("────────────────────────────────\n Bash command\n\n curl -sS https://example.com\n\n Do you want to proceed?\n{rows}\n\nEsc to cancel · Tab to amend · ctrl+e to explain\n  ⏵⏵ auto mode on · 1 shell · ← for agents\n")));
        }
    }
    cases
}

#[tokio::test]
async fn driver_blocker_preserves_captured_control_contracts_retired_from_manifests() {
    let mut failures = Vec::new();
    for (name, _, screen) in captured_manifest_controls() {
        let grid = captured_grid(&screen);
        if blocker(&grid, false).is_none() || !has_input_controls(&grid.text()) {
            failures.push(name);
        }
    }
    assert!(
        failures.is_empty(),
        "captured controls not recognized: {failures:?}"
    );
}

#[tokio::test]
async fn driver_captured_foreground_controls_override_native_idle_projection() {
    use crate::agents::{AgentKind, AgentState};
    for (name, kind, screen) in captured_manifest_controls() {
        // 4;3 is stale indeterminate progress during the captured permission
        // controls; it must not restore Working over the foreground form.
        let progress = ["4;0;", "4;3;"];
        for progress in progress
            .iter()
            .take(if kind == AgentKind::Claude { 2 } else { 1 })
        {
            let (runtime, mut writes) =
                crate::terminal::TerminalRuntime::test_with_channel(180, 64);
            if kind == AgentKind::Claude {
                runtime.test_process_pty_bytes(
                    format!("\x1b]2;✳ Task title\x1b\\\x1b]9;{progress}\x1b\\").as_bytes(),
                );
            }
            runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
            assert_eq!(
                runtime
                    .test_scan_detection_context(kind, AgentState::Idle)
                    .await,
                (AgentState::Blocked, true),
                "current captured controls outrank native idle/stale {progress}: {name}"
            );
            assert!(
                writes.try_recv().is_err(),
                "detection sends no input: {name}"
            );
        }
    }
}

#[tokio::test]
async fn driver_previous_captured_question_controls_yield_to_current_composer_or_work() {
    for (name, _, screen) in captured_manifest_controls()
        .into_iter()
        .filter(|(_, kind, _)| {
            matches!(
                kind,
                crate::agents::AgentKind::Codex | crate::agents::AgentKind::Cursor
            )
        })
    {
        for suffix in ["\n> Current editable draft\n", "\nctrl+c to stop\n"] {
            let screen = format!("{screen}{suffix}");
            assert!(
                !has_input_controls(&captured_grid(&screen).text()),
                "historical controls: {name}"
            );
        }
    }
}

fn reduced_manifest_controls() -> [(&'static str, crate::agents::AgentKind, &'static str); 6] {
    use crate::agents::AgentKind;
    // These are reduced/constructed control layouts copied from
    // agents/manifest/tests/others_test.rs. They are preservation contracts,
    // not raw native captures or proof of current provider keyboard behavior.
    [
        (
            "reduced kimi cache chooser with hint above menu",
            AgentKind::Kimi,
            "↑↓ navigate · Enter select · Esc cancel\nCache expired — the next message re-sends the entire history at full price.\n ❯ Compact and continue    one-time compact cost\n   Start a new session     zero context cost\n   Continue as-is          full history kept\n   Don't ask me again",
        ),
        (
            "reduced devin unpunctuated-number trust menu",
            AgentKind::Devin,
            "Do you trust the authors of this directory?\nFor security, devin should not be run in directories\nwith untrusted content.\n❭ 1 Yes, trust /private/tmp/devin-hook-probe\n· 2 No, exit",
        ),
        (
            "reduced devin command approval",
            AgentKind::Devin,
            "⏺ Running command\n  └ $ sleep 30\n\n❭ 1 Yes  (Approve once)\n· 2 Yes, allow `sleep` commands\n· 3 Yes, always allow `sleep` commands\n· 4 No\n↑↓ select · ↵ confirm · esc cancel",
        ),
        (
            "reduced muse picker before empty composer and model footer",
            AgentKind::Muse,
            "Which option should I use?\n\n› 1. Alpha\n  2. Beta\n\nEnter to select · ↑/↓ to move · Tab for an optional note · Esc to interrupt\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
        ),
        (
            "reduced muse command hotkey menu before model footer",
            AgentKind::Muse,
            "Would you like to run the following command?\n\n$ printf muse-safe-probe\n\n› 1. Allow this stage once (y)\n  2. Always allow in this workspace: printf muse-safe-probe ... (p)\n  3. Abort the entire command (esc)\n────────────────\ngpt-5.4 · minimal · /workspace",
        ),
        (
            "reduced muse network hotkey menu before model footer",
            AgentKind::Muse,
            "network: example.com:443 https\nrequested by:\n$ curl -fsS https://example.com\n\n› 1. Yes, proceed (y)\n  2. Yes, don't ask again this session (p)  example.com:443 (https)\n  3. No, and tell Muse Code what to do differently (esc)\n────────────────\ngpt-5.4 · minimal · /workspace",
        ),
    ]
}

#[tokio::test]
async fn driver_reduced_manifest_control_layouts_have_generic_episode_evidence() {
    let mut failures = Vec::new();
    for (name, _, screen) in reduced_manifest_controls() {
        let grid = captured_grid(screen);
        if blocker(&grid, false).is_none() || !has_input_controls(&grid.text()) {
            failures.push(name);
        }
    }
    assert!(
        failures.is_empty(),
        "reduced controls not recognized: {failures:?}"
    );
}

#[tokio::test]
async fn driver_reduced_foreground_layouts_publish_blocked_without_provider_ui_rules() {
    use crate::agents::AgentState;
    for (name, kind, screen) in reduced_manifest_controls() {
        let (runtime, mut writes) = crate::terminal::TerminalRuntime::test_with_channel(180, 64);
        runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
        assert_eq!(
            runtime
                .test_scan_detection_context(kind, AgentState::Idle)
                .await,
            (AgentState::Blocked, true),
            "reduced foreground controls: {name}"
        );
        assert!(
            writes.try_recv().is_err(),
            "state detection sends no input: {name}"
        );
    }
}

#[test]
fn driver_select_reads_unpunctuated_numbered_options_with_generic_focus_markers() {
    let grid = menu("Choose\n❭ 1 Yes, trust /private/tmp/project\n· 2 No, exit");
    assert_eq!(
        selection_step(&grid, "No, exit"),
        Ok(SelectionStep::Move("down"))
    );
    let next = menu("Choose\n· 1 Yes, trust /private/tmp/project\n❭ 2 No, exit");
    assert_eq!(
        selection_step(&next, "No, exit"),
        Ok(SelectionStep::Confirm)
    );
    let dotted_label = menu("Choose\n❭ 1 First\n· 2 · 2 remains part of this label");
    assert_eq!(
        selection_step(&dotted_label, "· 2 remains part of this label"),
        Ok(SelectionStep::Move("down"))
    );
}

#[tokio::test]
async fn driver_reduced_ordinary_reply_and_theme_viewer_are_not_input_episodes() {
    use crate::agents::{AgentKind, AgentState};
    for (screen, expected) in [
        (
            "⟩ say the phrase\n\n◆ Yes, proceed\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
            AgentState::Idle,
        ),
        (
            "Theme\n\n⟩ Default (active)\n  Dynamic\n\n↑↓ move · enter save · esc go back",
            AgentState::Unknown,
        ),
    ] {
        let (runtime, mut writes) = crate::terminal::TerminalRuntime::test_with_channel(180, 64);
        runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
        let (grid, _) = observe(&runtime).unwrap();
        assert!(blocker(&grid, false).is_none(), "{screen}");
        assert!(!has_input_controls(&grid.text()), "{screen}");
        assert_eq!(runtime.test_scan_detection_context(AgentKind::Muse, AgentState::Unknown).await, (expected, false), "{screen}");
        assert!(writes.try_recv().is_err());
    }
}

#[tokio::test]
async fn quoted_marker_inside_numbered_composer_is_not_a_control() {
    let screen = "history\n────────────────────\n❯ 1. First choice\n> 2. Second choice\n────────────────────\n? for shortcuts";
    let grid = captured_grid(screen);
    assert!(
        !has_input_controls(&grid.text()),
        "editable composer text became a control: {screen}\n{:?}",
        grid.text()
    );
    assert!(
        blocker(&grid, false).is_none(),
        "editable composer text became a blocker: {screen}"
    );
}

#[tokio::test]
async fn a_control_sharing_the_closing_chrome_row_stays_visible() {
    let mut missed = Vec::new();
    for screen in [
        "history\n────────────────\n❯ hello\n──────── [y/n]",
        "history\n› hello\nGPT-6 [y/n]",
        "history\n› hello\n\n? for shortcuts [y/n]",
    ] {
        let grid = captured_grid(screen);
        if !has_input_controls(&grid.text()) {
            missed.push(screen);
        }
    }
    assert!(
        missed.is_empty(),
        "closing chrome blanked a control on that row: {missed:?}"
    );
}
