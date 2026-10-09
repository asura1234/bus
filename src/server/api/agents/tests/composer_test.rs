use super::*;

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
