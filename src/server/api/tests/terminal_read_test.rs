use super::limit_snapshot_lines;

#[test]
fn line_limit_preserves_endings_and_reports_omitted_lines() {
    let snapshot = limit_snapshot_lines("one\ntwø\n三\n".into(), Some(2));
    assert_eq!(snapshot.text, "twø\n三\n");
    assert!(snapshot.truncated);

    let snapshot = limit_snapshot_lines("one\ntwo\nthree".into(), Some(1));
    assert_eq!(snapshot.text, "three");
    assert!(snapshot.truncated);

    let snapshot = limit_snapshot_lines("one\ntwo".into(), Some(0));
    assert_eq!(snapshot.text, "");
    assert!(snapshot.truncated);

    let snapshot = limit_snapshot_lines("".into(), Some(2));
    assert_eq!(snapshot.text, "");
    assert!(!snapshot.truncated);
}

#[test]
fn omitted_line_limit_returns_the_complete_snapshot() {
    let snapshot = limit_snapshot_lines("one\ntwo\n".into(), None);
    assert_eq!(snapshot.text, "one\ntwo\n");
    assert!(!snapshot.truncated);
}

#[tokio::test]
async fn recent_read_honors_line_requests_above_one_thousand() {
    let terminal =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(80, 3, 10_000_000, &[]);
    for index in 0..1500 {
        terminal.test_process_pty_bytes(format!("{index:06}\r\n").as_bytes());
    }
    let snapshot = super::read_terminal_snapshot(
        &terminal,
        crate::protocol::api::schema::ReadSource::Recent,
        crate::protocol::api::schema::ReadFormat::Text,
        Some(5000),
    );
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

#[tokio::test]
async fn room_orchestrator_core_recent_read_reports_exact_range_facts() {
    let terminal =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(12, 4, 1_000_000, &[]);
    for index in 0..30 {
        terminal.test_process_pty_bytes(format!("row-{index:02}\r\n").as_bytes());
    }
    let snapshot = super::read_terminal_snapshot(
        &terminal,
        crate::protocol::api::schema::ReadSource::Recent,
        crate::protocol::api::schema::ReadFormat::Text,
        Some(17),
    );
    assert_eq!(snapshot.requested_lines, Some(17));
    assert_eq!(snapshot.returned_lines, 17);
    assert!(snapshot
        .available_lines
        .is_some_and(|available| available >= 17));
    assert_eq!(snapshot.exhausted, Some(false));
    assert!(snapshot.revision > 0);
}

#[tokio::test]
async fn room_orchestrator_core_visible_read_reports_complete_viewport_facts() {
    let terminal =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(12, 4, 1_000_000, b"a\r\nb");
    let snapshot = super::read_terminal_snapshot(
        &terminal,
        crate::protocol::api::schema::ReadSource::Visible,
        crate::protocol::api::schema::ReadFormat::Text,
        None,
    );
    assert_eq!(snapshot.viewport_rows, Some(4));
    assert_eq!(snapshot.viewport_columns, Some(12));
    assert_eq!(snapshot.requested_lines, None);
    assert_eq!(snapshot.available_lines, None);
    assert_eq!(snapshot.exhausted, None);
    assert!(!snapshot.truncated);
}
