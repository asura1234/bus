use super::*;

#[test]
fn kimi_only_accepts_the_current_idle_framed_composer() {
    let frame = "╭────────────────────╮\n│ >                │\n╰────────────────────╯\nK2.8 Preview thinking: max";
    assert_eq!(
        observe(AgentKind::Kimi, frame, "Bus task"),
        crate::terminal::runtime::InputObservation::Empty
    );
    let draft = "╭────────────────────╮\n│ > Bus task       │\n╰────────────────────╯\nK2.8 Preview thinking: max";
    assert_eq!(
        observe(AgentKind::Kimi, draft, "Bus task"),
        crate::terminal::runtime::InputObservation::Pending
    );
    assert_eq!(
        observe(AgentKind::Kimi, draft, "Other task"),
        crate::terminal::runtime::InputObservation::Unavailable
    );
    let busy = format!("⠋ Working…\n{frame}");
    assert_eq!(
        observe(AgentKind::Kimi, &busy, "Bus task"),
        crate::terminal::runtime::InputObservation::Unavailable
    );
}

#[test]
fn kimi_multiline_paste_with_soft_wrapped_words_keeps_exact_bus_ownership() {
    let frame = "╭──────────────────────────────╮\n│ > Bus message 1              │\n│   Please answer with YOUR    │\n│   ANSWER.                    │\n╰──────────────────────────────╯\nAsk When Needed";
    assert_eq!(
        observe(
            AgentKind::Kimi,
            frame,
            "Bus message 1\nPlease answer with YOUR ANSWER."
        ),
        InputObservation::Pending,
    );
    assert_eq!(
        observe(
            AgentKind::Kimi,
            frame,
            "Bus message 1\nPlease answer with DIFFERENT ANSWER."
        ),
        InputObservation::Unavailable,
    );
}

#[test]
fn kimi_clipped_frame_cannot_attest_an_unseen_prefix() {
    // Captured from Kimi 2.4.0 in a 53-column scratch Bus pane.
    let frame = "╭─────────────────── ↑ 14 more ───────────────────╮\n│ > turn after assignment so the worker reply     │\n│   wakes you. When its captured reply            │\n│   arrives, inspect the assignment status and    │\n│   answer this developer message once with       │\n│   R1_ORCH_CONFIRMED and the worker result.      │\n│   Do not use a waiting-on-you tag or contact    │\n│   anything outside this scratch Bus.            │\n╰─────────────────────────────────────────────────╯\nAsk When Needed";
    let visible_suffix = "turn after assignment so the worker reply wakes you. When its captured reply arrives, inspect the assignment status and answer this developer message once with R1_ORCH_CONFIRMED and the worker result. Do not use a waiting-on-you tag or contact anything outside this scratch Bus.";
    for prefix in ["Bus message that was pasted. ", "Unrelated edited draft. "] {
        assert_eq!(
            observe(AgentKind::Kimi, frame, &format!("{prefix}{visible_suffix}")),
            InputObservation::Unavailable
        );
    }
}

#[test]
fn kimi_clipped_blank_frame_is_not_an_empty_composer() {
    let frame = "╭──────────── ↑ 14 more ─────────────╮\n│ >                                  │\n╰────────────────────────────────────╯\nAsk When Needed";
    assert_eq!(
        observe(AgentKind::Kimi, frame, "new Bus task"),
        InputObservation::Unavailable
    );
}

#[test]
fn kimi_editor_allows_command_menu_repaint_without_trusting_a_changed_composer() {
    use crate::terminal::runtime::SubmissionInspector;
    let inspector = preserving_observer(
        AgentKind::Kimi,
        "/new".into(),
        std::path::PathBuf::new(),
        vec![],
    );
    let before = "╭─────────────────────────────────────────────────╮\n│ > /new                                          │\n╰─────────────────────────────────────────────────╯\nAsk When Needed";
    let menu = "╭─────────────────────────────────────────────────╮\n│ > /new                                          │\n╰─────────────────────────────────────────────────╯\n│   → new         Start a fresh session in        │\n│                 the current workspace           │\nAsk When Needed";
    assert!(inspector.editor_frame_unchanged(before, menu));
    assert!(!inspector.editor_frame_unchanged(before, &menu.replace("/new", "/exit")));
    assert!(!inspector.editor_frame_unchanged(before, &format!("Working…\n{menu}")));
    assert!(!inspector.editor_frame_unchanged(before, &format!("{menu}\nRun this command?")));
    assert!(!inspector.editor_frame_unchanged(
        before,
        &menu.replace("the current workspace", "another command")
    ));
    let ordinary = preserving_observer(
        AgentKind::Kimi,
        "ordinary task".into(),
        std::path::PathBuf::new(),
        vec![],
    );
    assert!(!ordinary.editor_frame_unchanged(before, menu));
}

#[tokio::test]
async fn kimi_editor_never_touches_a_clipped_foreign_draft() {
    use bytes::Bytes;
    use std::time::Duration;
    let directory = crate::utils::test_temp::unique_temp_path("kimi-foreign-draft");
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(53, 24, 16);
    let frame = "╭──────────── ↑ 14 more ─────────────╮\n│ >                                  │\n╰────────────────────────────────────╯\nAsk When Needed";
    runtime
        .test_process_pty_bytes(format!("\x1b[2J\x1b[H{}", frame.replace('\n', "\r\n")).as_bytes());
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::from_static(b"new Bus task"),
            Bytes::from_static(b"\r"),
            vec![Bytes::from_static(b"\x01")],
            Duration::ZERO,
            None,
            Box::new(preserving_observer(
                AgentKind::Kimi,
                "new Bus task".into(),
                directory.clone(),
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    assert!(completion
        .recv_timeout(Duration::from_secs(8))
        .unwrap()
        .is_err());
    assert!(writes.try_recv().is_err(), "foreign input received a key");
    assert!(!directory.exists());
}

// Exercise the native editor exchange and the submission transaction together.
#[tokio::test]
async fn kimi_editor_submits_only_after_complete_draft_readback() {
    kimi_editor_case(0).await;
}

#[tokio::test]
async fn kimi_editor_rejects_a_changed_hidden_prefix() {
    kimi_editor_case(1).await;
}

#[tokio::test]
async fn kimi_editor_does_not_submit_when_the_developer_types_during_transfer() {
    kimi_editor_case(2).await;
}

async fn kimi_editor_case(tamper: u8) {
    use bytes::Bytes;
    use std::time::Duration;
    let directory = crate::utils::test_temp::unique_temp_path("kimi-editor-submit");
    std::fs::create_dir_all(&directory).unwrap();
    let exchange = directory.join("exchange");
    let file = directory.join("native-editor.txt");
    let prompt = format!("long Bus message {}\nlast line\n", "0123456789".repeat(140));
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(53, 24, 32);
    let draw = |draft: &str| {
        let flat = draft.replace('\n', " ");
        let tail = &flat[flat.len().saturating_sub(160)..];
        let chunks: Vec<_> = tail.as_bytes().chunks(40).collect();
        let mut frame = if draft.is_empty() {
            String::from("╭───────────────────────────────────────╮\n")
        } else {
            String::from("╭────────────── ↑ 14 more ──────────────╮\n")
        };
        if chunks.is_empty() {
            frame.push_str("│ >  │\n");
        } else {
            for (index, chunk) in chunks.iter().enumerate() {
                frame.push_str(if index == 0 { "│ > " } else { "│   " });
                frame.push_str(std::str::from_utf8(chunk).unwrap());
                frame.push_str(" │\n");
            }
        }
        frame.push_str("╰───────────────────────────────────────╯\nAsk When Needed");
        runtime.test_process_pty_bytes(
            format!("\x1b[2J\x1b[H{}", frame.replace('\n', "\r\n")).as_bytes(),
        );
    };
    async fn next(writes: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
        tokio::time::timeout(Duration::from_secs(8), writes.recv())
            .await
            .unwrap()
            .unwrap()
    }
    draw("");
    let completion = runtime
        .queue_observed_input_with_inspector(
            Bytes::copy_from_slice(prompt.as_bytes()),
            Bytes::from_static(b"\r"),
            vec![
                Bytes::from_static(b"\x01"),
                Bytes::from_static(b"\x0b"),
                Bytes::from_static(b"\x15"),
            ],
            Duration::ZERO,
            None,
            Box::new(preserving_observer(
                AgentKind::Kimi,
                prompt.clone(),
                exchange.clone(),
                vec![
                    Bytes::from_static(b"\x13"),
                    Bytes::from_static(b"\x07"),
                    Bytes::from_static(b"\x7f"),
                ],
            )),
        )
        .unwrap();
    let mut draft = String::new();
    for round in 0..2 {
        assert_eq!(
            next(&mut writes).await.as_ref(),
            b"\x07",
            "round {round}: no paste or Enter before editor verification"
        );
        std::fs::write(&file, &draft).unwrap();
        if round == 1 && tamper == 1 {
            let changed = format!("foreign prefix{}", &draft[4..]);
            std::fs::write(&file, &changed).unwrap();
            assert!(crate::platform::draft_editor::test_run_at(&exchange, &file).is_err());
            assert_eq!(std::fs::read_to_string(&file).unwrap(), changed);
            draw(&changed);
            assert!(completion
                .recv_timeout(Duration::from_secs(8))
                .unwrap()
                .is_err());
            assert!(
                writes.try_recv().is_err(),
                "changed draft must not receive Enter or deletion"
            );
            std::fs::remove_dir_all(directory).unwrap();
            return;
        }
        crate::platform::draft_editor::test_run_at(&exchange, &file).unwrap();
        draft = std::fs::read_to_string(&file).unwrap();
        let marker = draft
            .strip_prefix(&prompt)
            .expect("editor must restore exact full prompt")
            .to_owned();
        assert!(!marker.is_empty());
        draw(&draft);
        let delete = next(&mut writes).await;
        assert_eq!(delete.as_ref(), vec![0x7f; marker.len()]);
        draft.truncate(prompt.len());
        if round == 0 && tamper == 2 {
            runtime
                .try_send_developer_bytes(Bytes::from_static(b"typed meanwhile"))
                .unwrap();
            draw(&draft);
            assert!(completion
                .recv_timeout(Duration::from_secs(8))
                .unwrap()
                .is_err());
            assert!(
                writes.try_recv().is_err(),
                "no second editor key or Enter after typing"
            );
            std::fs::remove_dir_all(directory).unwrap();
            return;
        }
        draw(&draft);
    }
    assert_eq!(draft, prompt);
    assert_eq!(next(&mut writes).await.as_ref(), b"\r");
    draw("");
    completion
        .recv_timeout(Duration::from_secs(8))
        .unwrap()
        .unwrap();
    assert!(writes.try_recv().is_err(), "must submit once only");
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn grok_login_heading_above_composer_padding_receives_no_input() {
    use std::time::{Duration, Instant};

    let empty = include_str!("../../../../../tests/fixtures/grok-composer/empty.txt");
    for (heading, gap) in [
        ("Log in to Grok", 0),
        ("Log in to Grok", 13),
        ("Login with Grok", 13),
    ] {
        let screen = format!("{heading}\n{}{empty}", "\n".repeat(gap));
        let (runtime, mut writes) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 30, 16);
        runtime.test_process_pty_bytes(
            format!("\x1b[2J\x1b[H{}", screen.replace('\n', "\r\n")).as_bytes(),
        );
        let completion = runtime
            .queue_observed_input_with_inspector(
                bytes::Bytes::from_static(b"from bus"),
                bytes::Bytes::from_static(b"\r"),
                vec![bytes::Bytes::from_static(b"\x01")],
                Duration::ZERO,
                Some(Instant::now() + Duration::from_millis(200)),
                Box::new(observer(AgentKind::Grok, "from bus".into())),
            )
            .unwrap();
        let error = completion
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err();
        assert!(
            writes.try_recv().is_err(),
            "login composer with {gap} padding lines received editing keys, paste or Enter"
        );
        assert!(
            crate::terminal::runtime::PromptNotShown::is(&error),
            "{error}"
        );
    }
}

#[test]
fn captured_claude_subagent_footer_is_an_empty_composer() {
    let captured = include_str!("fixtures/claude_subagent_footer.txt");
    for footer in [
        "⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent",
        "⏵⏵ auto mode on (shift+tab to cycle)",
        "⏵⏵ bypass permissions on (shift+tab to cycle) · ← 2 agents",
        "shift+tab to cycle · ← 1 agent",
    ] {
        let screen = captured.replace("⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent", footer);
        assert_eq!(
            observe(AgentKind::Claude, &screen, "next message"),
            InputObservation::Empty,
            "{footer}"
        );
        let draft = screen.replacen("\n❯\n", "\n❯ unowned draft\n", 1);
        assert_eq!(
            observe(AgentKind::Claude, &draft, "next message"),
            InputObservation::Other,
            "{footer}"
        );
    }
}

#[test]
fn ownership_refusal_diagnostics_explain_checks_without_content() {
    const CHILD: &str = "COMPOSER_OWNERSHIP_LOG_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "server::api::agents::prompt::composer::tests::ownership_refusal_diagnostics_explain_checks_without_content", "--nocapture"])
            .env(CHILD, "1").output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout
                .lines()
                .any(|line| line.starts_with("test result: ok. 1 passed; 0 failed;")),
            "{stdout}"
        );
        return;
    }
    use crate::terminal::runtime::SubmissionInspector;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run_filtered(crate::utils::logging::DEV_FILTER, || {
        let screen = include_str!("fixtures/claude_subagent_footer.txt").replacen(
            "\n❯\n",
            "\n❯ private-unowned-draft\n",
            1,
        );
        let inspector = observer(AgentKind::Claude, "private-payload".into());
        assert_eq!(inspector.observe(&screen), InputObservation::Other);
        inspector.diagnose_refusal(&screen, "initial_refusal", Some((2, 1)));
    });
    let logs = capture.text();
    assert!(logs.contains("bus.composer.ownership"), "{logs}");
    assert_eq!(
        logs.matches("event=\"bus.composer.ownership\"").count(),
        1,
        "{logs}"
    );
    for field in [
        "composer_frame=true",
        "body_present=true",
        "body_empty=false",
        "normalized_match=false",
        "collapsed_match=false",
        "image_match=false",
        "expected_bytes=15",
        "body_bytes=21",
    ] {
        assert!(logs.contains(field), "missing {field}: {logs}");
    }
    assert!(!logs.contains("private-unowned-draft"));
    assert!(!logs.contains("private-payload"));
    assert!(!logs.contains("libtv-app"));
}

#[test]
fn codex_approval_with_model_footer_is_not_an_editable_composer() {
    let screen = "Would you like to run the following command?\n\n$ printf muse-safe-probe\n\n› 1. Allow this stage once (y)\n  2. Always allow in this workspace: printf muse-safe-probe ... (p)\n  3. Abort the entire command (esc)\n────────────────\ngpt-5.4 · minimal · /workspace";
    assert_eq!(
        observe(AgentKind::Codex, screen, "first message"),
        InputObservation::Unavailable
    );
}

#[test]
fn codex_trust_controls_are_never_owned_composer_input() {
    for screen in [
        "Hooks need review\n3 hooks are new or changed.\n› 1. Review hooks\n2. Trust all and continue\n3. Continue without trusting (hooks won't run)\nPress enter to confirm or esc to go back\n",
        "> You are in /tmp/project\nDo you trust the contents of this directory?\n› 1. Yes, continue\n2. No, quit\nPress enter to continue\n",
        include_str!("../../../../../tests/fixtures/codex-question/collapsed.txt"),
    ] {
        assert_eq!(observe(AgentKind::Codex, screen, "from bus"), InputObservation::Unavailable, "{screen}");
    }
}

#[tokio::test]
async fn startup_controls_visible_after_validation_receive_no_input() {
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 32);
    runtime.test_submission_clock().unwrap().pause();
    runtime.test_process_pty_bytes(
        concat!(
            "Hooks need review\r\n3 hooks are new or changed.\r\n",
            "› 1. Review hooks\r\n2. Trust all and continue\r\n",
            "3. Continue without trusting (hooks won't run)\r\n",
            "Press enter to confirm or esc to go back\r\n",
        )
        .as_bytes(),
    );
    // The dialog appeared after native identity/readiness validation but
    // before the asynchronous writer looked at the current screen.
    let completion = runtime
        .queue_observed_input_with_inspector(
            bytes::Bytes::from_static(b"first room message"),
            bytes::Bytes::from_static(b"\r"),
            vec![bytes::Bytes::from_static(b"\x01")],
            std::time::Duration::ZERO,
            None,
            Box::new(observer(AgentKind::Codex, "first room message".into())),
        )
        .unwrap();
    let error = completion
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .unwrap()
        .unwrap_err();
    assert!(
        writes.try_recv().is_err(),
        "trust chooser must receive no editing keys, paste or Enter"
    );
    assert!(
        crate::terminal::runtime::PromptNotShown::is(&error),
        "{error}"
    );
}

#[test]
fn cursor_current_idle_and_steering_placeholders_are_empty_composers() {
    for placeholder in [
        "Plan, search, build anything",
        "Add a follow-up",
        "Add a follow-up — /plan to review and build",
    ] {
        let screen = format!(" > {placeholder}\n / commands · @ files");
        assert_eq!(
            observe(AgentKind::Cursor, &screen, "from bus"),
            InputObservation::Empty,
            "{placeholder}"
        );
    }
    assert_eq!(
        observe(
            AgentKind::Cursor,
            " > Add a follow-up              ctrl+c to stop",
            "from bus"
        ),
        InputObservation::Empty
    );
    assert_eq!(
        observe(
            AgentKind::Cursor,
            " > from bus              ctrl+c to stop",
            "from bus"
        ),
        InputObservation::Pending
    );
}

#[test]
fn dim_collapsed_pastes_are_owned_input_not_empty_suggestions() {
    let text = "paragraph one\nparagraph two\nparagraph three";
    let cases = [
        (
            AgentKind::Codex,
            format!(
                "› \x1b[2m[Pasted Content {} chars]\x1b[0m\n\nGPT-6",
                text.chars().count()
            ),
        ),
        (
            AgentKind::Claude,
            "────────────────\n❯ \x1b[2m[Pasted text #7 +2 lines]\x1b[0m\n────────────────".into(),
        ),
        (
            AgentKind::Cursor,
            " > \x1b[2m[Pasted text #1 +3 lines]\x1b[0m\n / commands · @ files".into(),
        ),
    ];
    for (kind, screen) in cases {
        assert_eq!(
            observe(kind, &screen, text),
            InputObservation::Pending,
            "{kind:?}"
        );
        assert_eq!(
            observe(kind, &screen, "different text"),
            InputObservation::Other,
            "{kind:?}"
        );
    }
}

#[test]
fn multiline_input_preserves_blank_lines_and_wrapped_text() {
    let text = "first paragraph\n\nsecond paragraph";
    for (kind, screen) in [
        (
            AgentKind::Codex,
            "› first paragraph\n\n  second paragraph\n\n  GPT-6 · /repo\n  ? for shortcuts",
        ),
        (
            AgentKind::Claude,
            "────────────────\n❯ first paragraph\n\n  second paragraph\n────────────────",
        ),
        (
            AgentKind::Cursor,
            " > first paragraph\n\n  second paragraph\n / commands · @ files",
        ),
    ] {
        assert_eq!(
            observe(kind, screen, text),
            InputObservation::Pending,
            "{kind:?}"
        );
    }
}

#[test]
fn codex_composer_check_accepts_only_empty_prompt_states() {
    for screen in [
        "history\n›\n  gpt-5.6-sol",
        "history\n› Ask Codex to do anything\n  gpt-5.6-sol",
        "history\n› Use /skills to list available skills\n  gpt-5.6-sol",
    ] {
        assert_eq!(
            observe(AgentKind::Codex, screen, ""),
            InputObservation::Empty,
            "{screen}"
        );
    }
    for screen in [
        "",
        "history without a prompt",
        "history\n› draft already present\n  gpt-5.6-sol",
        "history\n› check the logs\n  hello?\n  gpt-5.6-sol",
    ] {
        assert_ne!(
            observe(AgentKind::Codex, screen, ""),
            InputObservation::Empty,
            "{screen}"
        );
    }
}

#[test]
fn claude_input_check_ignores_suggestions_and_finds_typed_text() {
    let rule = "─".repeat(20);
    let screen = |input: &str| format!("● done\n\n{rule}\n{input}\n{rule}\n  ⏵⏵ auto mode on");
    for empty in [
        screen("❯\u{a0}"),
        // Claude's dimmed prompt suggestion, as captured from a live pane.
        screen("❯\u{a0}\x1b[0m\x1b[2madd the just linux-lint recipe\x1b[0m"),
        // The cursor cell drawn inverse over the suggestion's first letter.
        screen("❯ \x1b[7ma\x1b[0m\x1b[2mdd the recipe\x1b[0m"),
        // An earlier prompt in the history above the box is not the box.
        format!("❯ ok is it merged?\n\n{rule}\n❯\u{a0}\n{rule}"),
    ] {
        assert_eq!(
            observe(AgentKind::Claude, &empty, ""),
            InputObservation::Empty,
            "{empty:?}"
        );
    }
    for typed in [
        screen("❯\u{a0}master is where I coordinate"),
        // A 24-bit color's "2" is not dim.
        screen("❯ \x1b[38;2;200;200;200mtyped\x1b[0m"),
        // A wrapped second line holds the typed text.
        format!("{rule}\n❯\u{a0}\n  second line\n{rule}"),
        // No visible input box: Bus cannot tell, so it waits.
        "● working".to_owned(),
    ] {
        assert_ne!(
            observe(AgentKind::Claude, &typed, ""),
            InputObservation::Empty,
            "{typed:?}"
        );
    }
}

#[test]
fn wrapped_cursor_prompt_keeps_model_name_at_start_of_continuation() {
    let screen = " > Compare outputs for\n   GPT-6 and Claude\n / commands · @ files";
    assert_eq!(
        observe(
            AgentKind::Cursor,
            screen,
            "Compare outputs for GPT-6 and Claude"
        ),
        InputObservation::Pending
    );
}

/// Screens captured from the installed providers after Bus's bracketed paste
/// of `text\n"<image path>"` (Claude Code 2.1.296, codex-cli 0.162.0, Cursor
/// Agent 2026.10.01): a real 120x32 PTY rendered by a VT emulator, with no
/// Enter ever sent. The capture directory is kept verbatim in the paths.
const CAPTURE: &str =
    "/private/tmp/claude-501/-Users-dylanliu-work-bus/d274b864-9483-4f60-b030-d05748dd5124/scratchpad/capture";

fn captured_payload(text: &str, dir: &str, images: usize) -> String {
    let quoted = (0..images)
        .map(|index| {
            let suffix = if index == 0 {
                String::new()
            } else {
                format!("-{index}")
            };
            format!("\"{CAPTURE}/{dir}/paste-0123456789abcdef{suffix}.png\"")
        })
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        quoted
    } else {
        format!("{text}\n{quoted}")
    }
}

#[test]
fn claude_shows_a_pasted_image_path_as_a_leading_image_chip_that_is_still_our_draft() {
    for (screen, text, dir) in [
        (
            include_str!("fixtures/claude_image_paste.txt"),
            "please review this screenshot",
            "work-claude",
        ),
        (
            include_str!("fixtures/claude_image_multiline_paste.txt"),
            "first line\nsecond line\nthird line",
            "work-claude-multiline",
        ),
        (
            include_str!("fixtures/claude_image_only_paste.txt"),
            "",
            "work-claude-image-only",
        ),
    ] {
        let expected = captured_payload(text, dir, 1);
        assert_eq!(
            observe(AgentKind::Claude, screen, &expected),
            InputObservation::Pending,
            "{screen}"
        );
    }
    // Two paths on one line stay literal text in Claude's composer.
    assert_eq!(
        observe(
            AgentKind::Claude,
            include_str!("fixtures/claude_two_images_paste.txt"),
            &captured_payload("please review this screenshot", "work-claude-two", 2)
        ),
        InputObservation::Pending
    );
}

/// Captured like the screens above, from Claude Code 2.1.306 in a 120x45 PTY:
/// image lines lifted beside text long enough to collapse into one paste chip.
const CAPTURE_COLLAPSED: &str =
    "/private/tmp/claude-501/-Users-dylanliu-work-bus/0442051f-5cb4-4c5e-85b0-2f3af02e3a36/scratchpad/capture";

fn collapsed_image(dir: &str, suffix: &str) -> String {
    format!("\"{CAPTURE_COLLAPSED}/{dir}/paste-0123456789abcdef{suffix}.png\"")
}

#[test]
fn claude_shows_lifted_images_beside_a_collapsed_paste_that_is_still_our_draft() {
    for (screen, expected) in [
        (
            include_str!("fixtures/claude_image_long_paste.txt"),
            format!(
                "first line\nsecond line\nthird line\nfourth line\n{}",
                collapsed_image("work-claude-long", "")
            ),
        ),
        (
            include_str!("fixtures/claude_image_long_line_paste.txt"),
            format!(
                "{}\n{}",
                ["review"; 200].join(" "),
                collapsed_image("work-claude-long-line", "")
            ),
        ),
        // Image lines between text lines; the chips are joined by a space.
        (
            include_str!("fixtures/claude_interleaved_images_long_paste.txt"),
            format!(
                "first line\n{}\nsecond line\n\nthird line\n{}\nfourth line",
                collapsed_image("work-claude-interleaved", ""),
                collapsed_image("work-claude-interleaved", "-1")
            ),
        ),
    ] {
        assert_eq!(
            observe(AgentKind::Claude, screen, &expected),
            InputObservation::Pending,
            "{screen}"
        );
    }
}

#[test]
fn a_collapsed_paste_beside_image_chips_is_ours_only_with_our_line_and_image_counts() {
    let screen = include_str!("fixtures/claude_image_long_paste.txt");
    let image = collapsed_image("work-claude-long", "");
    for expected in [
        format!("first line\nsecond line\nthird line\n{image}"),
        format!("first line\nsecond line\nthird line\nfourth line\nfifth line\n{image}"),
        "first line\nsecond line\nthird line\nfourth line".to_owned(),
        format!("first line\nsecond line\nthird line\nfourth line\n{image}\n{image}"),
        "first line\nsecond line\nthird line\nfourth line\n\"/tmp/notes.diff\"".to_owned(),
    ] {
        assert_eq!(
            observe(AgentKind::Claude, screen, &expected),
            InputObservation::Other,
            "{expected}"
        );
    }
    let one_line = include_str!("fixtures/claude_image_long_line_paste.txt");
    assert_eq!(
        observe(
            AgentKind::Claude,
            one_line,
            &format!("first line\nsecond line\n{image}")
        ),
        InputObservation::Other
    );
}

#[test]
fn an_image_chip_never_makes_someone_elses_draft_ours() {
    let screen = include_str!("fixtures/claude_image_paste.txt");
    for expected in [
        captured_payload("please review this", "work-claude", 1),
        captured_payload("please review this screenshot", "work-claude", 0),
        "please review this screenshot\n\"/tmp/notes.diff\"".to_owned(),
        captured_payload("please review this screenshot", "work-claude", 2),
    ] {
        assert_eq!(
            observe(AgentKind::Claude, screen, &expected),
            InputObservation::Other,
            "{expected}"
        );
    }
}

/// codex-cli 0.163.0, captured beside the Claude screens above: a paste that
/// is only one quoted image path becomes an attachment chip. With text, or with
/// two paths, Codex keeps the paste as text.
#[test]
fn codex_shows_a_lone_pasted_image_path_as_an_image_chip_that_is_still_our_draft() {
    let screen = include_str!("fixtures/codex_image_only_paste.txt");
    let image = collapsed_image("work-codex-image-only", "");
    assert_eq!(
        observe(AgentKind::Codex, screen, &image),
        InputObservation::Pending
    );
    for expected in [
        format!("please review this screenshot\n{image}"),
        format!("{image} {}", collapsed_image("work-codex-image-only", "-1")),
        "\"/tmp/notes.diff\"".to_owned(),
    ] {
        assert_eq!(
            observe(AgentKind::Codex, screen, &expected),
            InputObservation::Other,
            "{expected}"
        );
    }
}

#[test]
fn codex_image_chips_and_reply_instruction_are_owned_only_together() {
    let text = "worker:\n[Bus message 17; answer with bus send --reply-to 17 --as 3 --text 'YOUR ANSWER']\nreview\n\nall pixels";
    for (chips, images) in [
        ("[Image #1]", "\"/tmp/a.png\""),
        ("[Image #1] [Image #2]", "\"/tmp/a.png\"\n\"/tmp/b.png\""),
    ] {
        let expected = format!("{text}\n{images}");
        assert_eq!(
            observe(
                AgentKind::Codex,
                &format!("› {expected}\n\n GPT-6 · /repo"),
                &expected
            ),
            InputObservation::Pending,
            "a literal draft can be Codex's image-decode fallback"
        );
        for body in [
            format!("{chips}{text}"),
            format!("{chips}[Pasted Content {} chars]", text.chars().count()),
        ] {
            let screen = format!("› {body}\n\n GPT-6 · /repo\n ? for shortcuts");
            assert_eq!(
                observe(AgentKind::Codex, &screen, &expected),
                InputObservation::Pending
            );
            for wrong in [
                text.to_owned(),
                format!("{expected}\n\"/tmp/extra.png\""),
                expected.replace("reply-to 17", "reply-to 1700"),
            ] {
                assert_eq!(
                    observe(AgentKind::Codex, &screen, &wrong),
                    InputObservation::Other,
                    "{wrong}"
                );
            }
        }
    }
}

#[test]
fn combined_image_path_paste_is_owned_literal_text_in_codex_and_cursor() {
    let expected = |dir| captured_payload("please review this screenshot", dir, 1);
    assert_eq!(
        observe(
            AgentKind::Codex,
            include_str!("fixtures/codex_image_paste.txt"),
            &expected("work-codex")
        ),
        InputObservation::Pending
    );
    assert_eq!(
        observe(
            AgentKind::Cursor,
            include_str!("fixtures/cursor_image_paste.txt"),
            &expected("work-cursor")
        ),
        InputObservation::Pending
    );
}

#[test]
fn current_cursor_composer_uses_an_arrow_prompt_inside_half_block_borders() {
    assert_eq!(
        observe(
            AgentKind::Cursor,
            include_str!("fixtures/cursor_empty.txt"),
            "from bus"
        ),
        InputObservation::Empty
    );
}

#[test]
fn codex_draft_quoting_approval_footer_text_is_still_our_pending_draft() {
    for text in [
        "Reply when you see Allow command? in your pane",
        "If it says enter to submit answer, stop",
    ] {
        let screen = format!("history\n› {text}\n\n  gpt-5.6-sol · /repo");
        assert_eq!(
            observe(AgentKind::Codex, &screen, text),
            InputObservation::Pending,
            "{screen}"
        );
    }
}

#[test]
fn codex_draft_with_a_numbered_list_is_still_our_pending_draft() {
    let text = "1. run the tests\n2. open the PR";
    let screen = "history\n› 1. run the tests\n  2. open the PR\n\n  gpt-5.6-sol · /repo";
    assert_eq!(
        observe(AgentKind::Codex, screen, text),
        InputObservation::Pending,
        "{screen}"
    );
}

#[test]
fn claude_draft_quoting_dynamic_workflow_prompt_is_still_our_pending_draft() {
    let rule = "─".repeat(20);
    let text = "If you see Run a dynamic workflow? press esc to cancel";
    let screen = format!("● done\n\n{rule}\n❯\u{a0}{text}\n{rule}\n  ⏵⏵ auto mode on");
    assert_eq!(
        observe(AgentKind::Claude, &screen, text),
        InputObservation::Pending,
        "{screen}"
    );
}

#[tokio::test]
async fn codex_numbered_list_message_is_submitted_after_its_draft_renders() {
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(100, 24, 32);
    runtime.test_process_pty_bytes("history\r\n›\r\n\r\n  gpt-5.6-sol · /repo".as_bytes());
    let text = "1. run the tests\n2. open the PR";
    runtime.test_submission_clock().unwrap().pause();
    let completion = runtime
        .queue_observed_input_with_inspector(
            bytes::Bytes::from_static(b"1. run the tests\n2. open the PR"),
            bytes::Bytes::from_static(b"\r"),
            vec![bytes::Bytes::from_static(b"\x01")],
            std::time::Duration::ZERO,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
            Box::new(observer(AgentKind::Codex, text.into())),
        )
        .unwrap();
    let paste = tokio::time::timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT, writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(paste.starts_with(b"1. run"), "{paste:?}");
    runtime.test_process_pty_bytes(
        "\x1b[2J\x1b[Hhistory\r\n› 1. run the tests\r\n  2. open the PR\r\n\r\n  gpt-5.6-sol · /repo"
            .as_bytes(),
    );
    let enter =
        tokio::time::timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT, writes.recv()).await;
    assert!(
        matches!(&enter, Ok(Some(bytes)) if bytes.as_ref() == b"\r"),
        "the rendered numbered-list draft must receive Enter: {enter:?}; result {:?}",
        completion.recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
    );
    runtime.test_process_pty_bytes(
        "\x1b[2J\x1b[Hhistory\r\n›\r\n\r\n  gpt-5.6-sol · /repo".as_bytes(),
    );
    assert!(completion
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .unwrap()
        .is_ok());
}

#[test]
fn claude_transcript_quoting_dynamic_workflow_prompt_leaves_an_empty_composer() {
    let rule = "─".repeat(20);
    let screen = format!(
        "● Claude showed: Run a dynamic workflow? (esc to cancel)\n\n{rule}\n❯\u{a0}\n{rule}\n  ⏵⏵ auto mode on"
    );
    assert_eq!(
        observe(AgentKind::Claude, &screen, ""),
        InputObservation::Empty,
        "{screen}"
    );
}

#[test]
fn claude_draft_with_a_numbered_list_is_still_our_pending_draft() {
    let rule = "─".repeat(20);
    let text = "1. run the tests\n2. open the PR";
    let screen = format!(
        "● done\n\n{rule}\n❯\u{a0}1. run the tests\n  2. open the PR\n{rule}\n  ⏵⏵ auto mode on"
    );
    assert_eq!(
        observe(AgentKind::Claude, &screen, text),
        InputObservation::Pending,
        "{screen}"
    );
}

#[test]
fn captured_dialogs_are_never_editable_composers() {
    let rule = "─".repeat(64);
    let claude_bash = format!(
        "{rule}\n Bash command\n\n   curl -sS -o /tmp/probe.html https://example.com\n   Download example.com to /tmp/probe.html\n\n This command requires approval\n\n Do you want to proceed?\n ❯ 1. Yes\n   2. Yes, and don't ask again for: curl *\n   3. No\n\n Esc to cancel · Tab to amend · ctrl+e to explain\n"
    );
    let claude_trust = format!(
        "{rule}\n Accessing workspace:\n\n /Users/dev/work/bus\n\n Quick safety check: Is this a project you created or one you trust?\n\n Claude Code'll be able to read, edit, and execute files here.\n\n ❯ 1. Yes, I trust this folder\n   2. No, exit\n\n Enter to confirm · Esc to cancel\n"
    );
    let claude_question = "☐ Approach\n\nWhich storage should the cache use?\n\n❯ 1. Redis\n     Shared across workers\n  2. In memory\n     Fastest, lost on restart\n  3. Type something.\n────────────────────────────────\n  4. Chat about this\n\nEnter to select · ↑/↓ to navigate · Esc to cancel\n";
    for screen in [claude_bash.as_str(), claude_trust.as_str(), claude_question] {
        assert_eq!(
            observe(AgentKind::Claude, screen, "from bus"),
            InputObservation::Unavailable,
            "{screen}"
        );
    }
    for screen in [
        "Would you like to run the following command?\n\n$ printf muse-safe-probe\n\n› 1. Allow this stage once (y)\n  2. Always allow in this workspace: printf muse-safe-probe ... (p)\n  3. Abort the entire command (esc)\n────────────────\ngpt-5.4 · minimal · /workspace",
        "Which option should I use?\n\n› 1. Alpha\n  2. Beta\n\nEnter to select · ↑/↓ to move · Tab for an optional note · Esc to interrupt\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace",
        include_str!("../../../../../tests/fixtures/codex-question/expanded.txt"),
        include_str!("../../../../../tests/fixtures/codex-question/other-selected.txt"),
        include_str!("../../../../../tests/fixtures/codex-question/other-typed.txt"),
        include_str!("../../../../../tests/fixtures/codex-update/chooser-0.162.txt"),
    ] {
        assert_eq!(
            observe(AgentKind::Codex, screen, "from bus"),
            InputObservation::Unavailable,
            "{screen}"
        );
    }
}

// Captured from real cursor-agent 2026.10.14-0f9edde at 131x45.
// Its composer uses background-only rows, with no half-block border glyphs.
#[test]
fn cursor_background_composer_excludes_model_and_directory_footer() {
    assert_eq!(
        observe(
            AgentKind::Cursor,
            include_str!("fixtures/cursor_20261014_empty.ansi"),
            "from bus",
        ),
        InputObservation::Empty,
    );
    let screen = include_str!("fixtures/cursor_20261014_draft.ansi");
    assert_eq!(
        observe(
            AgentKind::Cursor,
            screen,
            "first line\n\nGPT-6 and Claude\nlast line"
        ),
        InputObservation::Pending,
    );
    assert_eq!(
        observe(AgentKind::Cursor, screen, "first line"),
        InputObservation::Other,
        "wrapped input remains part of the draft",
    );
}

#[tokio::test]
async fn cursor_background_composer_receives_paste_then_enter() {
    let (runtime, mut writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(131, 45, 32);
    let show = |screen: &str| {
        runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
        runtime.test_process_pty_bytes(
            screen
                .replace("\r\n", "\n")
                .replace('\n', "\r\n")
                .as_bytes(),
        );
    };
    show(include_str!("fixtures/cursor_20261014_empty.ansi"));
    let text = "first line\n\nGPT-6 and Claude\nlast line";
    runtime.test_submission_clock().unwrap().pause();
    let completion = super::super::queue_prompt_input(
        &runtime,
        AgentKind::Cursor,
        text.into(),
        true,
        std::time::Duration::ZERO,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
        None,
    )
    .unwrap();
    let paste = tokio::time::timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT, writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        paste.as_ref(),
        text.as_bytes(),
        "an empty composer must receive the prompt before editing keys"
    );
    show(include_str!("fixtures/cursor_20261014_draft.ansi"));
    let enter = tokio::time::timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT, writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        enter.as_ref(),
        b"\r",
        "only the matching rendered draft receives Enter"
    );
    show(include_str!("fixtures/cursor_20261014_empty.ansi"));
    assert!(completion
        .recv_timeout(crate::terminal::runtime::NATIVE_TEST_TIMEOUT)
        .unwrap()
        .is_ok());
}

#[test]
fn cursor_background_composer_keeps_directory_tree_lines_in_the_draft() {
    let screen =
        include_str!("fixtures/cursor_20261014_draft.ansi").replace("GPT-6 and Claude", "└── src");
    assert_eq!(
        observe(
            AgentKind::Cursor,
            &screen,
            "first line\n\n└── src\nlast line"
        ),
        InputObservation::Pending,
        "a directory-tree continuation inside the shaded composer is draft text",
    );
}

#[test]
fn cursor_background_composer_ignores_changed_model_and_workspace_footers() {
    for model in [
        "Auto",
        "Claude Sonnet",
        "another model · memory 0%",
        "\x1b[48;5;2mAuto\x1b[49m",
    ] {
        let screen = include_str!("fixtures/cursor_20261014_empty.ansi")
            .replace("Grok 4.7 256K High", model)
            .replace("Run Everything", "Ask Before Running")
            .replace(
                "/private/tmp/bus-tui/cursor-capture/work",
                "/workspace/project",
            );
        assert_eq!(
            observe(AgentKind::Cursor, &screen, "from bus"),
            InputObservation::Empty
        );
    }
}

#[test]
fn background_color_tracking_keeps_claude_and_codex_composer_text_visible() {
    for style in ["\x1b[41m", "\x1b[104m", "\x1b[48;5;2m", "\x1b[48;2;2;7;22m"] {
        for (kind, screen) in [
            (
                AgentKind::Claude,
                format!("────────────────\n❯ {style}from bus\x1b[49m\x1b[0m\n────────────────"),
            ),
            (
                AgentKind::Codex,
                format!("› {style}from bus\x1b[49m\x1b[0m\nGPT-6 · /repo"),
            ),
        ] {
            assert_eq!(
                observe(kind, &screen, "from bus"),
                InputObservation::Pending,
                "{kind:?} {style:?}"
            );
        }
    }
}

#[test]
fn cursor_background_composer_ends_at_the_box_even_without_a_blank_separator() {
    let screen = include_str!("fixtures/cursor_20261014_draft.ansi");
    let lines: Vec<&str> = screen.lines().collect();
    let model = lines
        .iter()
        .position(|line| line.contains("Grok 4.7"))
        .expect("fixture footer");
    assert!(
        lines[model - 1].trim().is_empty(),
        "fixture has a separator"
    );
    let mut adjacent = lines.clone();
    adjacent.remove(model - 1);
    assert_eq!(
        observe(
            AgentKind::Cursor,
            &adjacent.join("\n"),
            "first line\n\nGPT-6 and Claude\nlast line"
        ),
        InputObservation::Pending,
    );
}

#[tokio::test]
async fn cursor_background_composer_keeps_blank_rows_painted_by_erase_line() {
    // A blank shaded row may be painted with erase-in-line under a background
    // color instead of explicit spaces; it is still inside the composer box.
    let runtime = crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 8).0;
    let bg = "\x1b[48;2;18;18;18m";
    let screen = [
        format!(" {bg}\x1b[K\x1b[0m"),
        format!(" {bg} → first line\x1b[K\x1b[0m"),
        format!(" {bg}\x1b[K\x1b[0m"),
        format!(" {bg}   last line\x1b[K\x1b[0m"),
        format!(" {bg}\x1b[K\x1b[0m"),
        String::new(),
        "  \x1b[2mGrok 4.7 256K High\x1b[0m".into(),
        "  \x1b[2m/workspace/project\x1b[0m".into(),
    ]
    .join("\r\n");
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    runtime.test_process_pty_bytes(screen.as_bytes());
    let rendered = runtime.visible_ansi();
    assert_eq!(
        observe(AgentKind::Cursor, &rendered, "first line\n\nlast line"),
        InputObservation::Pending,
        "{rendered:?}"
    );
}

#[test]
fn cursor_half_block_composer_with_shaded_interior_still_ends_at_its_border() {
    let bg = "\x1b[48;2;18;18;18m";
    let fg = "\x1b[38;2;18;18;18m";
    let frame = |rows: &[&str]| {
        let mut lines = vec![format!(" {fg}▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄\x1b[0m")];
        lines.extend(
            rows.iter()
                .map(|row| format!(" {bg} {row}          \x1b[0m")),
        );
        lines.push(format!(" {fg}▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀\x1b[0m"));
        lines.push("  Grok 4.7 500K Extra High · MAX          Run Everything".into());
        lines.push("  /workspace/project".into());
        lines.join("\n")
    };
    assert_eq!(
        observe(
            AgentKind::Cursor,
            &frame(&["→ Plan, search, build anything"]),
            "from bus"
        ),
        InputObservation::Empty,
    );
    assert_eq!(
        observe(
            AgentKind::Cursor,
            &frame(&["→ first line", "", "  second line"]),
            "first line\n\nsecond line"
        ),
        InputObservation::Pending,
    );
}

#[test]
fn cursor_background_composer_collapsed_paste_is_owned_input() {
    let bg = "\x1b[48;2;18;18;18m";
    let screen = [
        format!(" {bg}                              \x1b[0m"),
        format!(" {bg} → [Pasted text #1 +4 lines]  \x1b[0m"),
        format!(" {bg}                              \x1b[0m"),
        String::new(),
        "  \x1b[2mGrok 4.7 256K High\x1b[0m".into(),
        "  \x1b[2m/workspace/project\x1b[0m".into(),
    ]
    .join("\n");
    assert_eq!(
        observe(AgentKind::Cursor, &screen, "a\nb\nc\nd"),
        InputObservation::Pending,
    );
    assert_eq!(
        observe(AgentKind::Cursor, &screen, "a\nb"),
        InputObservation::Other,
    );
}

#[test]
fn cursor_background_composer_keeps_border_and_footer_like_draft_lines() {
    for line in [
        "─ separator",
        "╰ nested",
        "▀ literal",
        "? for clarification",
        "/ commands to inspect",
    ] {
        let screen =
            include_str!("fixtures/cursor_20261014_draft.ansi").replace("GPT-6 and Claude", line);
        let expected = format!("first line\n\n{line}\nlast line");
        assert_eq!(
            observe(AgentKind::Cursor, &screen, &expected),
            InputObservation::Pending,
            "{line}"
        );
    }
}

#[test]
fn cursor_background_composer_keeps_quoted_markers_in_continuations() {
    for line in ["> quoted line", "→ quoted line"] {
        let screen =
            include_str!("fixtures/cursor_20261014_draft.ansi").replace("GPT-6 and Claude", line);
        let expected = format!("first line\n\n{line}\nlast line");
        assert_eq!(
            observe(AgentKind::Cursor, &screen, &expected),
            InputObservation::Pending,
            "{line}"
        );
        assert_eq!(
            observe(AgentKind::Cursor, &screen, "quoted line\nlast line"),
            InputObservation::Other,
            "a continuation is not a complete draft"
        );
    }
}

#[tokio::test]
async fn cursor_newer_legacy_composer_is_not_shadowed_by_shaded_history() {
    let history = "Earlier assistant output:\n\x1b[48;2;18;18;18m> quoted example from the transcript\x1b[0m\n\n";
    let legacy = include_str!("fixtures/cursor_empty.txt");
    let (runtime, _writes) =
        crate::terminal::TerminalRuntime::test_with_channel_capacity(200, 45, 8);
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    runtime.test_process_pty_bytes(
        format!("{history}{legacy}")
            .replace('\n', "\r\n")
            .as_bytes(),
    );
    let screen = runtime.visible_ansi();
    assert_eq!(
        observe(AgentKind::Cursor, &screen, "from bus"),
        InputObservation::Empty,
        "the captured later legacy composer is empty; shaded history must not replace it",
    );
}

#[test]
fn cursor_composer_format_order_preserves_the_newest_draft() {
    let legacy_text = "legacy draft from bus";
    let legacy = include_str!("fixtures/cursor_empty.txt")
        .replace("Plan, search, build anything", legacy_text);
    let background_text = "first line\n\nGPT-6 and Claude\nlast line";
    let background = include_str!("fixtures/cursor_20261014_draft.ansi");
    for (screen, current, previous) in [
        (
            format!("{background}\n{legacy}"),
            legacy_text,
            background_text,
        ),
        (
            format!("{legacy}\n{background}"),
            background_text,
            legacy_text,
        ),
    ] {
        assert_eq!(
            observe(AgentKind::Cursor, &screen, current),
            InputObservation::Pending,
            "the newest composer owns the current draft"
        );
        assert_eq!(
            observe(AgentKind::Cursor, &screen, previous),
            InputObservation::Other,
            "an older composer cannot own the current draft"
        );
    }
}

#[test]
fn queued_cleanup_keys_allow_their_intermediate_rendered_remainders() {
    use crate::terminal::runtime::SubmissionInspector;
    let inspector = observer(AgentKind::Codex, "header\nmiddle\nlast".into());
    let render = |body: &str| format!("› {body}\n\n GPT-6 · /repo");
    assert_eq!(
        inspector.observe(&render("header\nmiddle\nlast")),
        InputObservation::Pending
    );
    // A delayed renderer can show an earlier queued edit before the latest one.
    for index in [0, 1, 2, 0, 1] {
        inspector.clearing(index);
    }
    assert_eq!(
        inspector.observe(&render("header\nmiddle")),
        InputObservation::Pending
    );
    assert_eq!(
        inspector.observe(&render("header")),
        InputObservation::Pending
    );
}

#[path = "stale_own_paste_test.rs"]
mod stale_own_paste;

#[path = "late_own_paste_test.rs"]
mod late_own_paste;

#[test]
fn grok_reply_quoting_a_login_heading_does_not_hide_the_live_composer() {
    use crate::terminal::runtime::SubmissionInspector;
    // A reply line far above the composer is transcript, not the login
    // surface; the live composer below it must stay observable for delivery.
    let empty = include_str!("../../../../../tests/fixtures/grok-composer/empty.txt");
    for body_lines in [0, 1, 14] {
        let screen = format!(
            "◆ The deploy step failed with:\n\nLogin required\n\n{}\n{empty}",
            "Re-authenticate the CLI, then retry.\n".repeat(body_lines)
        );
        assert_eq!(
            observer(AgentKind::Grok, "next task".into()).observe(&screen),
            crate::terminal::runtime::InputObservation::Empty,
            "{body_lines} body lines"
        );
    }
}
