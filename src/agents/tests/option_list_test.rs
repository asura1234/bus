//! Claude Code 2.1.308 single-question option lists, captured from a real
//! `claude` under a scratch Bus (unwrapped visible screen, ANSI kept).
use super::*;

const SHORT_CURSOR3_SCROLLED: &str =
    include_str!("../../../tests/fixtures/claude-option-list/short-cursor3-scrolled.ansi");
const SHORT_CURSOR4_SCROLLED: &str =
    include_str!("../../../tests/fixtures/claude-option-list/short-cursor4-scrolled.ansi");
const SHORT_FREETEXT_FOCUSED_EMPTY: &str =
    include_str!("../../../tests/fixtures/claude-option-list/short-freetext-focused-empty.ansi");
const SHORT_REST_TOP: &str =
    include_str!("../../../tests/fixtures/claude-option-list/short-rest-top.ansi");
const TALL_FREETEXT_FOCUSED_EMPTY: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tall-freetext-focused-empty.ansi");
const TALL_FREETEXT_WITH_TEXT_FOCUSED: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tall-freetext-with-text-focused.ansi");
const TALL_REST: &str = include_str!("../../../tests/fixtures/claude-option-list/tall-rest.ansi");
const TALL_REST_FREETEXT_HOLDS_TEXT: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tall-rest-freetext-holds-text.ansi");

/// (number, selected, free text) for each option Bus offers.
fn rows(dialog: &Dialog) -> Vec<(u32, bool, bool)> {
    dialog
        .options
        .iter()
        .map(|option| (option.number, option.selected, option.free_text))
        .collect()
}

#[test]
fn resting_option_list_marks_its_free_text_row() {
    let dialog = parse(TALL_REST).unwrap();
    assert_eq!(dialog.kind, DialogKind::Choice);
    assert_eq!(dialog.text, "☐ Fix\n\nWhich fix should I implement for M2?");
    assert_eq!(
        rows(&dialog),
        [
            (1, true, false),
            (2, false, false),
            (3, false, false),
            (4, false, false),
            (5, false, true),
            (6, false, false),
        ]
    );
    assert_eq!(dialog.options[4].label, "Type something.");
    assert_eq!(dialog.keys_for(2), Some(vec!["down", "enter"]));
    // Enter on the empty field declines the whole question in Claude Code.
    assert_eq!(dialog.keys_for(5), None);
    assert_eq!(dialog.free_text_keys(), Some(vec!["down"; 4]));
}

#[test]
fn a_scrolled_option_list_in_a_short_pane_is_still_the_dialog() {
    let top = parse(SHORT_REST_TOP).unwrap();
    assert_eq!(top.kind, DialogKind::Choice);
    assert_eq!(
        rows(&top),
        [
            (1, true, false),
            (2, false, false),
            (3, false, false),
            (6, false, false)
        ]
    );
    assert_eq!(
        top.options[2].label,
        "C: drop subtitles Keep the user node, discard cues."
    );
    assert_eq!(top.options[3].label, "Chat about this");
    // Hidden options still count: Claude scrolls as the selection moves.
    assert_eq!(
        top.keys_for(6),
        Some(vec!["down", "down", "down", "down", "down", "enter"])
    );
    // The free-text row is scrolled out of view, but its number is known.
    assert_eq!(top.free_text_option(), Some(5));
    assert_eq!(top.free_text_keys(), Some(vec!["down"; 4]));
    assert_eq!(top.keys_for(5), None);

    let middle = parse(SHORT_CURSOR3_SCROLLED).unwrap();
    assert_eq!(
        rows(&middle),
        [
            (2, false, false),
            (3, true, false),
            (4, false, false),
            (6, false, false)
        ]
    );
    assert_eq!(middle.text, "☐ Fix\n\nWhich fix should I implement for M2?");
    assert_eq!(
        middle.id(),
        top.id(),
        "scrolling keeps the dialog's identity"
    );

    let bottom = parse(SHORT_CURSOR4_SCROLLED).unwrap();
    assert_eq!(
        rows(&bottom),
        [
            (2, false, false),
            (3, false, false),
            (4, true, false),
            (6, false, false)
        ]
    );
    assert_eq!(bottom.keys_for(2), Some(vec!["up", "up", "enter"]));
}

#[test]
fn the_focused_free_text_row_is_a_question_even_with_a_wrapped_hint() {
    let tall = parse(TALL_FREETEXT_FOCUSED_EMPTY).unwrap();
    assert_eq!(tall.kind, DialogKind::Question);
    assert_eq!(tall.text, "Which fix should I implement for M2?");
    assert_eq!(tall.input.as_ref().unwrap().value, "");

    // At 72 columns Claude wraps `Esc to cancel` onto its own line.
    let short = parse(SHORT_FREETEXT_FOCUSED_EMPTY).unwrap();
    assert_eq!(short.kind, DialogKind::Question);
    assert_eq!(short.text, tall.text);
    assert_eq!(short.input.as_ref().unwrap().value, "");

    let typed = parse(TALL_FREETEXT_WITH_TEXT_FOCUSED).unwrap();
    assert_eq!(typed.kind, DialogKind::Question);
    assert_eq!(
        typed.input.as_ref().unwrap().value,
        "Combined write: arrows then paste"
    );
    assert_eq!(typed.id(), tall.id());
    assert_ne!(typed.digest(), tall.digest());
}

#[test]
fn a_free_text_row_holding_a_draft_is_not_answered_over() {
    let dialog = parse(TALL_REST_FREETEXT_HOLDS_TEXT).unwrap();
    assert_eq!(dialog.kind, DialogKind::Choice);
    let row = &dialog.options[4];
    assert_eq!((row.number, row.free_text), (5, true));
    assert_eq!(row.label, "Combined write: arrows then paste");
    // Re-entering the field puts the cursor at an unknown place in the draft.
    assert_eq!(dialog.free_text_keys(), None);
}

const TABS_Q1_REST: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tabs-q1-rest.ansi");
const TABS_Q1_FREETEXT_FOCUSED: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tabs-q1-freetext-focused.ansi");
const TABS_Q1_TYPED: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tabs-q1-typed.ansi");
const TABS_Q2_REST: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tabs-q2-rest.ansi");
const TABS_REVIEW: &str =
    include_str!("../../../tests/fixtures/claude-option-list/tabs-review.ansi");

fn tabs(dialog: &Dialog) -> Vec<(&str, bool, bool)> {
    dialog
        .tabs
        .iter()
        .map(|tab| (tab.label.as_str(), tab.answered, tab.active))
        .collect()
}

#[test]
fn a_tabbed_form_reports_every_question_tab_and_marks_its_free_text_row() {
    let first = parse(TABS_Q1_REST).unwrap();
    assert_eq!(first.kind, DialogKind::Choice);
    assert_eq!(
        tabs(&first),
        [
            ("Fresh files", false, true),
            ("Helper home", false, false),
            ("Submit", false, false)
        ]
    );
    assert_eq!(
        rows(&first),
        [
            (1, true, false),
            (2, false, false),
            (3, false, true),
            (4, false, false),
            (5, false, false),
            (6, false, false)
        ]
    );
    // Enter on the empty field declines every question of the form.
    assert_eq!(first.keys_for(3), None);
    assert_eq!(first.free_text_keys(), Some(vec!["down"; 2]));
    assert_eq!(first.keys_for(6), Some(vec!["right"]));

    let second = parse(TABS_Q2_REST).unwrap();
    assert_eq!(
        tabs(&second),
        [
            ("Fresh files", true, false),
            ("Helper home", false, true),
            ("Submit", false, false)
        ]
    );
    assert_ne!(second.id(), first.id());

    let review = parse(TABS_REVIEW).unwrap();
    assert_eq!(review.options[0].label, "Submit answers");
    assert_eq!(
        tabs(&review),
        [
            ("Fresh files", true, false),
            ("Helper home", true, false),
            ("Submit", false, true)
        ]
    );
    assert!(parse(TALL_REST).unwrap().tabs.is_empty());
}

#[test]
fn a_claude_free_text_question_has_no_skip_key_because_esc_cancels_the_form() {
    let focused = parse(TABS_Q1_FREETEXT_FOCUSED).unwrap();
    assert_eq!(focused.kind, DialogKind::Question);
    assert_eq!(
        focused.text,
        "Which fresh-file strategy should the test use?"
    );
    assert_eq!(focused.input.as_ref().unwrap().skip_key, None);
    assert_eq!(focused.tabs.len(), 3);
    let typed = parse(TABS_Q1_TYPED).unwrap();
    assert_eq!(typed.input.as_ref().unwrap().value, "Fresh dir per run");
    let single = parse(TALL_FREETEXT_FOCUSED_EMPTY).unwrap();
    assert_eq!(single.input.as_ref().unwrap().skip_key, None);
}

fn scrolled_tabbed_question(active: usize) -> String {
    // Both questions can legitimately have the same prompt/options. Preserve
    // the captured short-pane scrolling rows and vary only the active tab.
    let header = match active {
        0 => "← \x1b[7m ☐ Backend \x1b[0m ☐ Frontend  ✔ Submit  →",
        1 => "←  ☐ Backend \x1b[7m ☐ Frontend \x1b[0m ✔ Submit  →",
        _ => unreachable!(),
    };
    let old_header = SHORT_CURSOR3_SCROLLED.lines().nth(1).unwrap();
    SHORT_CURSOR3_SCROLLED.replace(old_header, header)
}

#[test]
fn scrolled_tabbed_question_fingerprint_tracks_the_active_tab() {
    let first = parse(&scrolled_tabbed_question(0)).unwrap();
    let second = parse(&scrolled_tabbed_question(1)).unwrap();
    assert!(first.tabs[0].active && !first.tabs[1].active);
    assert!(second.tabs[1].active && !second.tabs[0].active);
    assert_ne!(
        first.digest(),
        second.digest(),
        "a fingerprint for Backend must not authorize an answer to Frontend"
    );
}

#[tokio::test]
async fn scrolled_tabbed_question_rejects_a_choice_observed_on_another_tab() {
    use crate::terminal::runtime::{DialogChoice, TerminalRuntime};

    let (runtime, mut writes) = TerminalRuntime::test_with_channel(80, 24);
    let paint = |screen: String| {
        runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H");
        runtime.test_process_pty_bytes(screen.replace('\n', "\r\n").as_bytes());
    };
    paint(scrolled_tabbed_question(0));
    let observed = parse(&runtime.visible_ansi_snapshot_with_seq().unwrap().0).unwrap();
    assert!(observed.tabs[0].active);
    paint(scrolled_tabbed_question(1));
    let current = parse(&runtime.visible_ansi_snapshot_with_seq().unwrap().0).unwrap();
    assert!(current.tabs[1].active);
    assert_eq!(
        runtime
            .try_choose_dialog_option(&observed.digest(), 3)
            .unwrap(),
        DialogChoice::Stale,
        "switching to another tab invalidates the observed answer"
    );
    assert!(writes.try_recv().is_err());
}

#[test]
fn unscrolled_tabbed_question_fingerprint_tracks_the_active_tab() {
    let first = parse(TABS_Q1_REST).unwrap();
    let screen = TABS_Q1_REST.replace(
        "\x1b[38;2;0;0;0m\x1b[48;2;177;185;249m ☐ Fresh files  \x1b[0m☐ Helper home",
        " ☐ Fresh files  \x1b[38;2;0;0;0m\x1b[48;2;177;185;249m☐ Helper home\x1b[0m",
    );
    assert_ne!(screen, TABS_Q1_REST);
    let second = parse(&screen).unwrap();
    assert!(second.tabs[1].active);
    assert_ne!(first.digest(), second.digest());
}
