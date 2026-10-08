use super::*;

#[test]
fn recent_reads_include_viewport_before_scrollback_exists() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal =
        crate::ghostty::Terminal::new(20, 20, crate::config::DEFAULT_SCROLLBACK_LIMIT_BYTES)
            .unwrap();
    terminal.write(b"hello123");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert_eq!(pane.recent_text(3), "hello123\n");
    assert_eq!(pane.recent_unwrapped_text(3), "hello123");
}

#[test]
fn alternate_screen_recent_reads_keep_physical_row_ranges() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(20, 20, 100).unwrap();
    terminal.write(b"\x1b[?1049hhello123");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert_eq!(pane.recent_text(3), "");
    assert_eq!(pane.recent_unwrapped_text(3), "");
}

#[test]
fn recent_unwrapped_text_ignores_soft_wraps() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(5, 3, 100).unwrap();
    terminal.write(b"ABCDEFGHIJ");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert_eq!(pane.recent_text(3), "ABCDE\nFGHIJ\n");
    assert_eq!(pane.recent_unwrapped_text(3), "ABCDEFGHIJ");
}

#[test]
fn room_orchestrator_core_recent_text_snapshot_honors_requests_above_one_thousand() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(80, 3, 10_000_000).unwrap();
    write_numbered_lines(&mut terminal, 1500);
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let snapshot = pane.recent_text_snapshot(5000);
    let returned = snapshot
        .text
        .split_inclusive('\n')
        .filter(|line| !line.is_empty())
        .count();
    assert!(
        returned > 1000,
        "expected more than the old 1000-line clamp, got {returned}"
    );
    assert!(
        snapshot.text.contains("000000"),
        "honored 5000-line window should include the oldest retained row"
    );
    assert!(
        !snapshot.truncated,
        "fewer rows than requested means available history is exhausted"
    );
}

#[test]
fn room_orchestrator_core_recent_snapshots_report_omitted_rendered_rows() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
    terminal.write(b"one\r\ntwo\r\nthree\r\nfour");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert!(pane.recent_text_snapshot(2).truncated);
    assert!(pane.recent_ansi_snapshot(2).truncated);
    assert!(pane.recent_unwrapped_text_snapshot(2).truncated);
    assert!(pane.recent_unwrapped_ansi_snapshot(2).truncated);
    assert!(!pane.recent_text_snapshot(100).truncated);
}

#[test]
fn recent_ansi_can_read_styled_scrollback() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
    terminal.write(b"\x1b[34mblue\x1b[0m\r\nline2\r\nline3\r\nline4");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let ansi = pane.recent_ansi(4);
    assert!(ansi.contains("blue"));
    assert!(ansi.contains("line4"));
    assert!(ansi.contains("\x1b["));
}

#[test]
fn trim_trailing_blank_rows_drops_empty_viewport_tail() {
    let mut rows = vec!["hello".to_string(), "".to_string(), "   ".to_string()];
    trim_trailing_blank_rows(&mut rows);
    assert_eq!(rows, vec!["hello".to_string()]);
}
