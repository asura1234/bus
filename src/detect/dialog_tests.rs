use super::*;

fn labels(dialog: &Dialog) -> Vec<(u32, &str, bool)> {
    dialog
        .options
        .iter()
        .map(|option| (option.number, option.label.as_str(), option.selected))
        .collect()
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
