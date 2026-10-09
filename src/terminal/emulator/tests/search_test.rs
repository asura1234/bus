use super::*;

#[test]
fn retained_text_search_crosses_soft_wraps_but_not_hard_lines() {
    let buffer = RetainedTextBuffer::new(
        5,
        vec![
            text_row("abcde".chars().map(|ch| text_cell(&ch.to_string())), true),
            text_row("fgh  ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("abc  ".chars().map(|ch| text_cell(&ch.to_string())), false),
        ],
    );

    let matches = search_primary(&buffer, "def", true);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start, TerminalTextPoint { row: 0, col: 3 });
    assert_eq!(matches[0].end, TerminalTextPoint { row: 1, col: 0 });
    assert!(search_primary(&buffer, "hab", true).is_empty());
}

#[test]
fn retained_text_search_maps_wide_and_combining_graphemes_to_cells() {
    let mut cells = vec![text_cell("A")];
    cells.extend(wide_text_cells("界"));
    cells.push(text_cell("e\u{301}"));
    cells.push(text_cell("Z"));
    let buffer = RetainedTextBuffer::new(5, vec![text_row(cells, false)]);

    let matches = search_primary(&buffer, "界e\u{301}", true);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start, TerminalTextPoint { row: 0, col: 1 });
    assert_eq!(matches[0].end, TerminalTextPoint { row: 0, col: 3 });
    assert!(search_primary(&buffer, "\u{301}", true).is_empty());
}

#[test]
fn retained_text_search_skips_wide_spacer_heads_at_soft_wraps() {
    let mut first = "abcd"
        .chars()
        .map(|ch| text_cell(&ch.to_string()))
        .collect::<Vec<_>>();
    first.push(crate::terminal::vt::ScreenTextCell {
        wide: crate::terminal::vt::CellWide::SpacerHead,
        graphemes: Vec::new(),
    });
    let mut second = wide_text_cells("界").to_vec();
    second.extend("xyz".chars().map(|ch| text_cell(&ch.to_string())));
    let buffer = RetainedTextBuffer::new(5, vec![text_row(first, true), text_row(second, false)]);

    let matches = search_primary(&buffer, "d界", true);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].start, TerminalTextPoint { row: 0, col: 3 });
    assert_eq!(matches[0].end, TerminalTextPoint { row: 1, col: 1 });
}

#[test]
fn retained_text_search_is_literal_and_unicode_case_aware() {
    let buffer = RetainedTextBuffer::new(
        12,
        vec![text_row(
            "CAFÉ a.b    ".chars().map(|ch| text_cell(&ch.to_string())),
            false,
        )],
    );

    assert_eq!(search_primary(&buffer, "café", false).len(), 1);
    assert!(search_primary(&buffer, "café", true).is_empty());
    assert_eq!(search_primary(&buffer, "a.b", true).len(), 1);
    assert!(search_primary(&buffer, "a?b", true).is_empty());
}
