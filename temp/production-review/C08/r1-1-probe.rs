// Archived after the user accepted option A.
// Blank-separated question and answer paragraphs can produce identical plain-text screens.
// The original unresolved probe below is preserved verbatim outside the active test suite.

#[test]
fn codex_free_text_answer_with_a_blank_line_keeps_the_question_and_whole_value() {
    let screen = CODEX_TEXT_QUESTION.replace(
        "  Type your answer\n",
        "  first paragraph\n\n  second paragraph\n",
    );
    let dialog = parse(&screen).unwrap();
    assert_eq!(dialog.text, "What token should Bus use?");
    assert_eq!(dialog.id(), parse(CODEX_TEXT_QUESTION).unwrap().id());
    let edited = parse(&screen.replace("first paragraph", "edited paragraph")).unwrap();
    assert_eq!(dialog.id(), edited.id());
    assert_ne!(dialog.digest(), edited.digest());
    assert_eq!(
        dialog.input.unwrap().value,
        "first paragraph\n\nsecond paragraph"
    );
}
