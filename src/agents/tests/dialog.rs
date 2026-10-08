use super::*;

fn labels(dialog: &Dialog) -> Vec<(u32, &str, bool)> {
    dialog
        .options
        .iter()
        .map(|option| (option.number, option.label.as_str(), option.selected))
        .collect()
}

const CODEX_TEXT_QUESTION: &str = concat!(
    "• Working (1m 28s • esc to interrupt)\n\n",
    "• Queued follow-up inputs\n\n",
    "  What token should Bus use?\n\n",
    "  Type your answer\n\n",
    "  enter submit   ctrl+] skip   shift+→ main prompt\n",
);

const CURSOR_OTHER: &str = concat!(
    "┌──────────────────────────────────────────────────────────┐\n",
    "│ Clarifying Questions                                    │\n",
    "│ Question 1 of 1                                         │\n",
    "│ 1. What token should Bus use?                            │\n",
    "│     [ ] Alpha                                           │\n",
    "│     [ ] Beta                                            │\n",
    "│   › [ ] Other: (type to answer)                          │\n",
    "│ ↑/↓ option · ←/→ question · Space select · Enter next/submit · Esc to skip │\n",
    "└──────────────────────────────────────────────────────────┘\n",
);

#[test]
fn codex_free_text_question_has_no_options_and_tracks_input_edits() {
    let dialog = parse(CODEX_TEXT_QUESTION).unwrap();
    assert_eq!(dialog.kind, DialogKind::Question);
    assert_eq!(dialog.text, "What token should Bus use?");
    assert!(dialog.options.is_empty());
    assert!(dialog.keys_for(1).is_none());
    assert_eq!(dialog.input.as_ref().unwrap().skip_key, "ctrl+]");
    let edited = parse(&CODEX_TEXT_QUESTION.replace("Type your answer", "MY_TOKEN")).unwrap();
    assert_eq!(dialog.id(), edited.id());
    assert_ne!(dialog.digest(), edited.digest());
    assert_eq!(edited.input.unwrap().value, "MY_TOKEN");
    let glyphs = CODEX_TEXT_QUESTION
        .replace("ctrl+]", "⌃]")
        .replace("shift+→", "⇧→");
    assert_eq!(parse(&glyphs).unwrap().kind, DialogKind::Question);
    let counted = CODEX_TEXT_QUESTION.replace("What token", "1 of 2\n\n  What token");
    assert_eq!(parse(&counted).unwrap().text, "What token should Bus use?");
    let stale_choices = format!("› 1. Old\n  2. Options\n\n{CODEX_TEXT_QUESTION}");
    assert_eq!(parse(&stale_choices).unwrap().kind, DialogKind::Question);
}

#[test]
fn text_question_controls_are_live_only_at_the_bottom() {
    for screen in [
        format!("{CODEX_TEXT_QUESTION}\n› Ask Codex to do anything\n"),
        CODEX_TEXT_QUESTION.replace("ctrl+] skip", "esc cancel"),
        CODEX_TEXT_QUESTION.replace("• Queued follow-up inputs", "Transcript excerpt"),
        "What token should Bus use?\nType your answer\n".to_owned(),
        "• Queued follow-up inputs\n? 1 question\nshift+← to answer\n› Ask Codex to do anything\n"
            .to_owned(),
        format!("{CURSOR_OTHER}\n⬢ Working · ctrl+c to stop\n"),
    ] {
        assert!(parse(&screen).is_none(), "{screen}");
    }
}

#[test]
fn cursor_other_text_question_requires_the_focused_other_row() {
    let dialog = parse(CURSOR_OTHER).unwrap();
    assert_eq!(dialog.kind, DialogKind::Question);
    assert_eq!(dialog.text, "What token should Bus use?");
    assert_eq!(dialog.input.as_ref().unwrap().skip_key, "esc");
    let edited = parse(&CURSOR_OTHER.replace("(type to answer)", "MY_TOKEN")).unwrap();
    assert_eq!(dialog.id(), edited.id());
    assert_ne!(dialog.digest(), edited.digest());
    let chooser = CURSOR_OTHER
        .replace("› [ ] Other:", "  [ ] Other:")
        .replace("    [ ] Alpha", "  › [ ] Alpha");
    assert!(parse(&chooser).is_none());
    let wrapped = CURSOR_OTHER.replace(
        "1. What token should Bus use?",
        "1. What token\n│    should Bus use?",
    );
    assert_eq!(parse(&wrapped).unwrap().text, "What token\nshould Bus use?");
    let wrapped_input = CURSOR_OTHER.replace("(type to answer)", "first line\n│       second line");
    let wrapped_input = parse(&wrapped_input).unwrap();
    assert_eq!(wrapped_input.id(), dialog.id());
    assert_eq!(
        wrapped_input.input.unwrap().value,
        "first line\nsecond line"
    );
}

#[test]
fn claude_focused_custom_answer_is_a_text_question_before_enter() {
    let screen = CLAUDE_QUESTION
        .replace("❯ 1. Redis", "  1. Redis")
        .replace("  3. Type something.", "❯ 3. Type something.")
        .replace("Esc to cancel", "ctrl+g to edit in Vim · Esc to cancel");
    let dialog = parse(&screen).unwrap();
    assert_eq!(dialog.kind, DialogKind::Question);
    assert_eq!(dialog.text, "Which storage should the cache use?");
    assert_eq!(dialog.input.as_ref().unwrap().skip_key, "esc");
    let edited = parse(&screen.replace("Type something.", "MY_TOKEN")).unwrap();
    assert_eq!(dialog.id(), edited.id());
    assert_ne!(dialog.digest(), edited.digest());
    assert!(parse(&format!("{screen}\n❯ A new prompt\n")).is_none());
}

// Captured from a Claude Code Bash approval at its resting cursor position.
const CLAUDE_BASH: &str = concat!(
    "────────────────────────────────────────────────────────────────\n",
    " Bash command\n\n",
    "   curl -sS -o /tmp/probe.html https://example.com\n",
    "   Download example.com to /tmp/probe.html\n\n",
    " This command requires approval\n\n",
    " Do you want to proceed?\n",
    " ❯ 1. Yes\n",
    "   2. Yes, and don't ask again for: curl *\n",
    "   3. No\n\n",
    " Esc to cancel · Tab to amend · ctrl+e to explain\n",
);

const CLAUDE_TRUST: &str = concat!(
    "────────────────────────────────────────────────────────────────\n",
    " Accessing workspace:\n\n",
    " /Users/dev/work/bus\n\n",
    " Quick safety check: Is this a project you created or one you trust?\n\n",
    " Claude Code'll be able to read, edit, and execute files here.\n\n",
    " ❯ 1. Yes, I trust this folder\n",
    "   2. No, exit\n\n",
    " Enter to confirm · Esc to cancel\n",
);

const CLAUDE_QUESTION: &str = concat!(
    "☐ Approach\n\n",
    "Which storage should the cache use?\n\n",
    "❯ 1. Redis\n",
    "     Shared across workers\n",
    "  2. In memory\n",
    "     Fastest, lost on restart\n",
    "  3. Type something.\n",
    "────────────────────────────────\n",
    "  4. Chat about this\n\n",
    "Enter to select · ↑/↓ to navigate · Esc to cancel\n",
);

// Codex and Codex-based TUIs, from the detection fixtures.
const CODEX_COMMAND: &str = "Would you like to run the following command?\n\n$ printf muse-safe-probe\n\n› 1. Allow this stage once (y)\n  2. Always allow in this workspace: printf muse-safe-probe ... (p)\n  3. Abort the entire command (esc)\n────────────────\ngpt-5.4 · minimal · /workspace";

const CODEX_QUESTION: &str = "Which option should I use?\n\n› 1. Alpha\n  2. Beta\n\nEnter to select · ↑/↓ to move · Tab for an optional note · Esc to interrupt\n\n────────────────\n⟩\n────────────────\ngpt-5.4 · minimal · /workspace";

const CODEX_TRUST: &str = "> You are in /Users/dev/project\n\n\
    Do you trust the contents of this\n\
    directory? Working with untrusted\n\
    contents comes with higher risk of\n\
    prompt injection.\n\n\
    › 1. Yes, continue\n\
      2. No, quit\n\n\
    Press enter to continue\n";

const CODEX_UPDATE_WRAPPED: &str = concat!(
    "✨ Update available! 0.153.0\n\n",
    "Release notes: https://example\n\n",
    "› 1. Update now (runs `npm\n",
    "     install -g\n",
    "     @openai/codex`)\n",
    "  2. Skip\n",
    "  3. Skip until next\n",
    "     version\n\n",
    "Press enter to continue\n",
);

// Captured live from codex-dev (Codex, GPT-6) mid-turn.
const CODEX_WORKING: &str = concat!(
    "› Orchestrator: thanks. NEXT: early review. Run the review-pr skill (read-\n",
    "  only; proving tests allowed but leave them uncommitted) on feat/simplify-\n",
    "  bus-orchestration for the range 161553f7..HEAD only.\n\n\n",
    "• I'll use the review-pr skill on the committed range.\n\n",
    "• Working (24s • esc to interrupt)\n\n\n",
    "› Ask Codex to do anything\n\n",
    "  GPT-6-Astra xhigh · ~/work/bus · Proceed with rebase plan\n",
    "  ? for shortcuts\n",
);

#[test]
fn claude_permission_trust_and_question_dialogs() {
    let bash = parse(CLAUDE_BASH).unwrap();
    assert_eq!(
        labels(&bash),
        [
            (1, "Yes", true),
            (2, "Yes, and don't ask again for: curl *", false),
            (3, "No", false)
        ]
    );
    assert!(bash.text.starts_with("Bash command\n\ncurl -sS"));
    assert!(bash
        .text
        .ends_with("This command requires approval\n\nDo you want to proceed?"));
    assert_eq!(
        bash.hint.as_deref(),
        Some("Esc to cancel · Tab to amend · ctrl+e to explain")
    );

    let trust = parse(CLAUDE_TRUST).unwrap();
    assert_eq!(
        labels(&trust),
        [
            (1, "Yes, I trust this folder", true),
            (2, "No, exit", false)
        ]
    );
    assert!(trust.text.contains("/Users/dev/work/bus"));

    let question = parse(CLAUDE_QUESTION).unwrap();
    assert_eq!(
        labels(&question),
        [
            (1, "Redis Shared across workers", true),
            (2, "In memory Fastest, lost on restart", false),
            (3, "Type something.", false),
            (4, "Chat about this", false)
        ]
    );
    assert_eq!(
        question.text,
        "☐ Approach\n\nWhich storage should the cache use?"
    );
}

#[test]
fn codex_permission_question_trust_and_wrapped_dialogs() {
    let command = parse(CODEX_COMMAND).unwrap();
    assert_eq!(command.options.len(), 3);
    assert_eq!(command.selected(), Some(1));
    assert_eq!(command.options[2].label, "Abort the entire command (esc)");
    assert_eq!(command.hint, None);
    assert!(command.text.contains("$ printf muse-safe-probe"));

    let question = parse(CODEX_QUESTION).unwrap();
    assert_eq!(labels(&question), [(1, "Alpha", true), (2, "Beta", false)]);
    assert_eq!(question.text, "Which option should I use?");
    assert!(question.hint.unwrap().starts_with("Enter to select"));

    let trust = parse(CODEX_TRUST).unwrap();
    assert_eq!(
        labels(&trust),
        [(1, "Yes, continue", true), (2, "No, quit", false)]
    );
    assert!(trust.text.starts_with("> You are in /Users/dev/project"));

    let update = parse(CODEX_UPDATE_WRAPPED).unwrap();
    assert_eq!(
        labels(&update),
        [
            (1, "Update now (runs `npm install -g @openai/codex`)", true),
            (2, "Skip", false),
            (3, "Skip until next version", false)
        ]
    );
}

#[test]
fn codex_expanded_question_form_is_read_on_the_first_parse() {
    let screen = concat!(
        "• Working (12s • esc to interrupt)\n\n",
        "• Queued follow-up inputs\n\n",
        "  Which storage?\n\n",
        "  › 1. Redis\n",
        "    2. Memory\n",
        "    3. Other\n\n",
        "  enter submit   ⌃] skip   ⇧→ main prompt\n",
    );
    let dialog = parse(screen).expect("expanded question");
    assert_eq!(
        labels(&dialog),
        [
            (1, "Redis", true),
            (2, "Memory", false),
            (3, "Other", false)
        ]
    );
    assert_eq!(dialog.text.lines().last(), Some("Which storage?"));
}

#[test]
fn codex_question_option_wrapped_under_its_number_is_still_the_dialog() {
    let screen = concat!(
        "• Queued follow-up inputs\n\n",
        "  What next?\n\n",
        "  › 1.\n",
        "       ship the parser but do not deploy\n",
        "    2. Other\n\n",
        "  enter submit   ⌃] skip   ⇧→ main prompt\n",
    );
    let dialog = parse(screen).expect("wrapped option");
    assert_eq!(
        labels(&dialog),
        [
            (1, "ship the parser but do not deploy", true),
            (2, "Other", false)
        ]
    );
    assert_eq!(dialog.text.lines().last(), Some("What next?"));
}

#[test]
fn codex_collapsed_question_banner_has_no_numbered_dialog() {
    let screen = concat!(
        "• Working (12s • esc to interrupt)\n\n",
        "• Queued follow-up inputs\n\n",
        "  ? 1 question\n",
        "    ⇧← to answer\n\n",
        "› Ask Codex to do anything\n",
    );
    assert_eq!(parse(screen), None, "{screen}");
    let shift_left = screen.replace('⇧', "shift+").replace('←', "left");
    assert_eq!(parse(&shift_left), None, "{shift_left}");
}

#[test]
fn boxed_dialogs_parse_inside_their_borders() {
    let boxed = concat!(
        "╭──────────────────────────────╮\n",
        "│ Run this command?            │\n",
        "│                              │\n",
        "│ ❯ 1. Run once                │\n",
        "│   2. Skip                    │\n",
        "│                              │\n",
        "│ Enter to confirm · Esc       │\n",
        "╰──────────────────────────────╯\n",
    );
    let dialog = parse(boxed).unwrap();
    assert_eq!(labels(&dialog), [(1, "Run once", true), (2, "Skip", false)]);
    assert_eq!(dialog.text, "Run this command?");
}

#[test]
fn transcripts_and_idle_screens_have_no_dialog() {
    let numbered_reply = concat!(
        "⏺ Two fixes are needed:\n",
        "  1. Retry the request\n",
        "  2. Log the failure\n\n",
        "────────────────\n",
        "❯ \n",
        "────────────────\n",
        "  ⏵⏵ auto mode on\n",
    );
    let echoed_list = "› 1. fix the parser\n  2. add tests\n\n• Working (3s • esc to interrupt)\n\n› Ask Codex to do anything\n";
    let answered = "› 1. Yes, continue\n  2. No, quit\n\nPress enter to continue\n\n› New prompt\n";
    for screen in [
        "",
        CODEX_WORKING,
        numbered_reply,
        echoed_list,
        answered,
        "❯ 1. Only one option\n\nEnter to confirm\n",
        "  1. Yes\n  2. No\n",
        "❯ 1. Yes\n❯ 2. No\n\nEnter to confirm\n",
        "❯ 2. Yes\n  3. No\n\nEnter to confirm\n",
    ] {
        assert_eq!(parse(screen), None, "{screen}");
    }
}

// Captured live from Claude Code 2.1.291, which no longer numbers this prompt.
const CLAUDE_TRUST_UNNUMBERED: &str = concat!(
    "─────────────────────────────────────────────────\n",
    " Accessing workspace:\n\n",
    " /Users/dev/work/bus/temp/e2e/workspace\n\n",
    " Quick safety check: Is this a project you created or one you trust?\n\n",
    " Claude Code'll be able to read, edit, and execute files here.\n\n",
    " Security guide\n\n",
    " ❯ No, exit\n",
    "   Yes, I trust this folder\n\n",
    " Enter to confirm · Esc to cancel\n",
);

#[test]
fn unnumbered_options_parse_when_a_confirm_hint_follows() {
    let trust = parse(CLAUDE_TRUST_UNNUMBERED).unwrap();
    assert_eq!(
        labels(&trust),
        [
            (1, "No, exit", true),
            (2, "Yes, I trust this folder", false)
        ]
    );
    assert!(trust
        .text
        .contains("/Users/dev/work/bus/temp/e2e/workspace"));
    assert_eq!(
        trust.hint.as_deref(),
        Some("Enter to confirm · Esc to cancel")
    );
    assert_eq!(trust.keys_for(2), Some(vec!["down", "enter"]));

    let below_transcript = format!("❯ 1. Old\n  2. Older\n\n{CLAUDE_TRUST_UNNUMBERED}");
    assert_eq!(parse(&below_transcript).unwrap().options.len(), 2);

    for screen in [
        // A composer: one line, or wrapped input without a confirm hint.
        "❯ fix the parser\n\nEnter to confirm\n",
        "› fix the parser\n  and add tests\n\n• Working (3s • esc to interrupt)\n",
        "❯ fix the parser\n  and add tests\n────────────────\n  ⏵⏵ auto mode on\n",
        // A list with a live composer below it.
        "❯ No, exit\n  Yes, I trust this folder\n\nEnter to confirm\n\n❯ \n",
    ] {
        assert_eq!(parse(screen), None, "{screen}");
    }
}

#[test]
fn only_the_last_numbered_block_is_live() {
    let screen = format!("Earlier plan:\n❯ 1. Old\n  2. Older\n\nsome output\n\n{CLAUDE_TRUST}");
    assert_eq!(
        parse(&screen).unwrap().options[0].label,
        "Yes, I trust this folder"
    );
}

#[test]
fn digest_tracks_text_options_and_selection() {
    let dialog = parse(CLAUDE_BASH).unwrap();
    assert_eq!(dialog.digest(), parse(CLAUDE_BASH).unwrap().digest());
    let moved = CLAUDE_BASH
        .replace(" ❯ 1. Yes", "   1. Yes")
        .replace("   3. No", " ❯ 3. No");
    assert_ne!(parse(&moved).unwrap().digest(), dialog.digest());
    let other_command = CLAUDE_BASH.replace("curl -sS", "curl -fsS");
    assert_ne!(parse(&other_command).unwrap().digest(), dialog.digest());
}

#[test]
fn keys_move_with_arrows_from_the_selected_option_then_enter() {
    let bash = parse(CLAUDE_BASH).unwrap();
    assert_eq!(bash.keys_for(1), Some(vec!["enter"]));
    assert_eq!(bash.keys_for(3), Some(vec!["down", "down", "enter"]));
    assert_eq!(bash.keys_for(4), None);
    let moved = parse(
        &CLAUDE_BASH
            .replace(" ❯ 1. Yes", "   1. Yes")
            .replace("   3. No", " ❯ 3. No"),
    )
    .unwrap();
    assert_eq!(moved.keys_for(1), Some(vec!["up", "up", "enter"]));

    let codex = parse(CODEX_COMMAND).unwrap();
    assert_eq!(codex.keys_for(2), Some(vec!["down", "enter"]));

    // Number shortcuts are never typed, even when the hint offers them.
    let numbered = parse(
        "Pick one\n› 1. Alpha\n  2. Beta\n  3. Gamma\n\nPress a number 1-3 · Esc to cancel\n",
    )
    .unwrap();
    assert_eq!(numbered.keys_for(3), Some(vec!["down", "down", "enter"]));

    // An unmarked, unstyled selection cannot be reached safely.
    let unmarked = parse("  1. Yes\n  2. No\n\nEnter to select · ↑/↓ to navigate\n").unwrap();
    assert_eq!(unmarked.selected(), None);
    assert_eq!(unmarked.keys_for(1), None);
}

#[test]
fn highlighted_options_are_selected_when_no_marker_is_drawn() {
    // Inverse video, as list pickers draw their cursor row.
    let inverse = "Choose a model\n\n  1. Fast\n\x1b[7m  2. Balanced\x1b[0m\n  3. Thorough\n\nEnter to select · Esc to cancel\n";
    let dialog = parse(inverse).unwrap();
    assert_eq!(dialog.selected(), Some(2));
    assert_eq!(dialog.options[1].label, "Balanced");
    assert_eq!(dialog.keys_for(3), Some(vec!["down", "enter"]));

    // One colored label among plain ones, in 256-color and truecolor forms.
    for color in ["\x1b[38;5;75m", "\x1b[38;2;120;180;255m", "\x1b[1;36m"] {
        let screen = format!(
            "Run this command?\n  1. \x1b[0m{color}Run once\x1b[0m\n  2. Skip\n\nEnter to confirm · Esc\n"
        );
        assert_eq!(parse(&screen).unwrap().selected(), Some(1), "{color:?}");
    }
    let three = "Pick\n\x1b[32m  1. A\x1b[0m\n\x1b[32m  2. B\x1b[0m\n\x1b[33m  3. C\x1b[39m\n\nEnter to select\n";
    assert_eq!(parse(three).unwrap().selected(), Some(3));

    // Two differently colored options, or two filled ones, are ambiguous.
    for ambiguous in [
        "Pick\n\x1b[32m  1. A\x1b[0m\n\x1b[33m  2. B\x1b[0m\n\nEnter to select\n",
        "Pick\n\x1b[7m  1. A\x1b[0m\n\x1b[44m  2. B\x1b[0m\n\nEnter to select\n",
    ] {
        assert_eq!(parse(ambiguous).unwrap().selected(), None);
    }
    // Without a key hint, highlight alone does not make a list a dialog.
    assert_eq!(parse("  1. A\n\x1b[7m  2. B\x1b[0m\n"), None);
}

#[test]
fn ansi_markers_and_hyperlinks_parse_like_plain_text() {
    let styled = CLAUDE_BASH
        .replace(" ❯ 1. Yes", "\x1b[38;5;153m ❯ 1. Yes\x1b[39m")
        .replace(
            "Bash command",
            "\x1b]8;;https://x\x07Bash command\x1b]8;;\x1b\\",
        );
    assert_eq!(parse(&styled), parse(CLAUDE_BASH));
}

#[test]
fn id_ignores_the_selection_but_not_the_question() {
    let dialog = parse(CLAUDE_BASH).unwrap();
    let moved = parse(
        &CLAUDE_BASH
            .replace(" ❯ 1. Yes", "   1. Yes")
            .replace("   3. No", " ❯ 3. No"),
    )
    .unwrap();
    assert_eq!(moved.id(), dialog.id());
    assert_ne!(moved.digest(), dialog.digest());
    let other = parse(&CLAUDE_BASH.replace("curl -sS", "curl -fsS")).unwrap();
    assert_ne!(other.id(), dialog.id());
}
