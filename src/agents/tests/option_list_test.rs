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
    // The free-text row is scrolled out of view, so its field cannot be checked.
    assert_eq!(top.free_text_keys(), None);

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
