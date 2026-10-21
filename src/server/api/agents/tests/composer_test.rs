use super::*;

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
        .queue_observed_input_submission(
            bytes::Bytes::from_static(b"first room message"),
            bytes::Bytes::from_static(b"\r"),
            vec![bytes::Bytes::from_static(b"\x01")],
            std::time::Duration::ZERO,
            None,
            observer(AgentKind::Codex, "first room message".into()),
        )
        .unwrap();
    let error = completion
        .recv_timeout(std::time::Duration::from_secs(3))
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

#[test]
fn codex_and_cursor_keep_a_pasted_image_path_as_text() {
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
    let completion = runtime
        .queue_observed_input_submission(
            bytes::Bytes::from_static(b"1. run the tests\n2. open the PR"),
            bytes::Bytes::from_static(b"\r"),
            vec![bytes::Bytes::from_static(b"\x01")],
            std::time::Duration::ZERO,
            Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
            observer(AgentKind::Codex, text.into()),
        )
        .unwrap();
    let paste = tokio::time::timeout(std::time::Duration::from_secs(2), writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(paste.starts_with(b"1. run"), "{paste:?}");
    runtime.test_process_pty_bytes(
        "\x1b[2J\x1b[Hhistory\r\n› 1. run the tests\r\n  2. open the PR\r\n\r\n  gpt-5.6-sol · /repo"
            .as_bytes(),
    );
    let enter = tokio::time::timeout(std::time::Duration::from_secs(3), writes.recv()).await;
    assert!(
        matches!(&enter, Ok(Some(bytes)) if bytes.as_ref() == b"\r"),
        "the rendered numbered-list draft must receive Enter: {enter:?}; result {:?}",
        completion.recv_timeout(std::time::Duration::from_secs(1))
    );
    runtime.test_process_pty_bytes(
        "\x1b[2J\x1b[Hhistory\r\n›\r\n\r\n  gpt-5.6-sol · /repo".as_bytes(),
    );
    assert!(completion
        .recv_timeout(std::time::Duration::from_secs(2))
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
    runtime.test_process_pty_bytes(include_bytes!("fixtures/cursor_20261014_empty.ansi"));
    let text = "first line\n\nGPT-6 and Claude\nlast line";
    let completion = super::super::queue_prompt_input(
        &runtime,
        AgentKind::Cursor,
        text.into(),
        true,
        std::time::Duration::ZERO,
        Some(std::time::Instant::now() + std::time::Duration::from_secs(2)),
    )
    .unwrap();
    let paste = tokio::time::timeout(std::time::Duration::from_secs(1), writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        paste.as_ref(),
        text.as_bytes(),
        "an empty composer must receive the prompt before editing keys"
    );
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    runtime.test_process_pty_bytes(include_bytes!("fixtures/cursor_20261014_draft.ansi"));
    let enter = tokio::time::timeout(std::time::Duration::from_secs(1), writes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        enter.as_ref(),
        b"\r",
        "only the matching rendered draft receives Enter"
    );
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
    runtime.test_process_pty_bytes(include_bytes!("fixtures/cursor_20261014_empty.ansi"));
    assert!(completion
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap()
        .is_ok());
}
