use super::*;

#[test]
fn retained_text_word_motion_does_not_split_at_a_wide_spacer_head() {
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

    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextStart),
        None
    );
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextEnd),
        Some(TerminalTextPoint { row: 1, col: 4 })
    );
}

#[test]
fn retained_text_word_motions_use_tmux_separators_across_rows() {
    let buffer = RetainedTextBuffer::new(
        6,
        vec![
            text_row("a_b.c ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("—d    ".chars().map(|ch| text_cell(&ch.to_string())), false),
        ],
    );

    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 3 })
    );
    assert_eq!(
        buffer.word_motion(0, 3, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 4 })
    );
    assert_eq!(
        buffer.word_motion(0, 4, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 1, col: 0 })
    );
    assert_eq!(
        buffer.word_motion(1, 1, TerminalWordMotion::PreviousStart),
        Some(TerminalTextPoint { row: 1, col: 0 })
    );
}

#[test]
fn retained_text_big_word_motions_treat_only_whitespace_as_separators() {
    let buffer = RetainedTextBuffer::new(
        20,
        vec![text_row(
            "foo.bar baz qux/quux"
                .chars()
                .map(|ch| text_cell(&ch.to_string())),
            false,
        )],
    );

    // `W` skips punctuation-separated segments and lands on the next
    // whitespace-delimited run.
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigStart),
        Some(TerminalTextPoint { row: 0, col: 8 })
    );
    assert_eq!(
        buffer.word_motion(0, 8, TerminalWordMotion::NextBigStart),
        Some(TerminalTextPoint { row: 0, col: 12 })
    );
    // `E` lands on the last character of the current/next run.
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 6 })
    );
    assert_eq!(
        buffer.word_motion(0, 6, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 10 })
    );
    assert_eq!(
        buffer.word_motion(0, 12, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 19 })
    );
    // `B` returns to the beginning of the previous run.
    assert_eq!(
        buffer.word_motion(0, 19, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 12 })
    );
    assert_eq!(
        buffer.word_motion(0, 12, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 8 })
    );
    assert_eq!(
        buffer.word_motion(0, 8, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 0 })
    );

    // Lowercase motions keep their punctuation-aware behavior.
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 3 })
    );
    assert_eq!(
        buffer.word_motion(0, 3, TerminalWordMotion::NextStart),
        Some(TerminalTextPoint { row: 0, col: 4 })
    );
    assert_eq!(
        buffer.word_motion(0, 4, TerminalWordMotion::PreviousStart),
        Some(TerminalTextPoint { row: 0, col: 3 })
    );
}

#[test]
fn retained_text_big_word_motions_cross_rows_and_blank_lines() {
    let buffer = RetainedTextBuffer::new(
        6,
        vec![
            text_row("a.b-c ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("      ".chars().map(|ch| text_cell(&ch.to_string())), false),
            text_row("d_e   ".chars().map(|ch| text_cell(&ch.to_string())), false),
        ],
    );

    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigStart),
        Some(TerminalTextPoint { row: 2, col: 0 })
    );
    assert_eq!(
        buffer.word_motion(2, 0, TerminalWordMotion::PreviousBigStart),
        Some(TerminalTextPoint { row: 0, col: 0 })
    );
    assert_eq!(
        buffer.word_motion(0, 0, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 0, col: 4 })
    );
    assert_eq!(
        buffer.word_motion(0, 4, TerminalWordMotion::NextBigEnd),
        Some(TerminalTextPoint { row: 2, col: 2 })
    );
}

#[test]
fn live_terminal_word_motion_expands_across_long_blank_history() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(10, 3, 200).unwrap();
    terminal.write(b"origin\r\n");
    for _ in 0..80 {
        terminal.write(b"\r\n");
    }
    let last_row = terminal.total_rows().unwrap().saturating_sub(1) as u32;
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal, tx).unwrap());

    assert_eq!(
        pane.word_motion_target(last_row, 0, TerminalWordMotion::PreviousStart),
        Some(TerminalTextPoint { row: 0, col: 0 })
    );
}

#[test]
fn live_terminal_word_end_expands_through_a_long_soft_wrap() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(2, 3, 200).unwrap();
    let word = "a".repeat(132);
    terminal.write(word.as_bytes());
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal, tx).unwrap());
    let text_match = pane
        .search_text_window(
            &word,
            true,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            1,
        )
        .matches[0];

    assert_eq!(
        pane.word_motion_target(
            text_match.start.row,
            text_match.start.col,
            TerminalWordMotion::NextEnd,
        ),
        Some(text_match.end)
    );
}

#[test]
fn live_terminal_word_end_expands_through_a_long_wide_soft_wrap() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(2, 3, 200).unwrap();
    let word = "界".repeat(66);
    terminal.write(word.as_bytes());
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal, tx).unwrap());
    let text_match = pane
        .search_text_window(
            &word,
            true,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            1,
        )
        .matches[0];

    // The word end sits on the head cell of the final wide glyph, past the
    // initial read window, so the window has to expand to reach it.
    assert_eq!(
        pane.word_motion_target(
            text_match.start.row,
            text_match.start.col,
            TerminalWordMotion::NextEnd,
        ),
        Some(TerminalTextPoint {
            row: text_match.end.row,
            col: 0,
        })
    );
}

#[test]
fn live_terminal_previous_word_start_expands_through_a_long_wide_soft_wrap() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(2, 3, 200).unwrap();
    let word = "界".repeat(130);
    terminal.write(word.as_bytes());
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal, tx).unwrap());
    let text_match = pane
        .search_text_window(
            &word,
            true,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            1,
        )
        .matches[0];

    for motion in [
        TerminalWordMotion::PreviousStart,
        TerminalWordMotion::PreviousBigStart,
    ] {
        assert_eq!(
            pane.word_motion_target(text_match.end.row, text_match.end.col, motion),
            Some(text_match.start),
            "motion {motion:?} stopped inside the wrapped word"
        );
    }
}
