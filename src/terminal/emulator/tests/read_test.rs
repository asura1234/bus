use super::*;

#[test]
fn pane_scrollback_controls_round_trip_and_clamp_without_ui_interference() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(80, 3, 100).unwrap();
    write_numbered_lines(&mut terminal, 1000);
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let before = pane.scroll_metrics().expect("scroll metrics before scroll");
    assert!(before.max_offset_from_bottom > 0);
    assert_eq!(before.offset_from_bottom, 0);

    for offset in [
        0,
        before.max_offset_from_bottom / 2,
        before.max_offset_from_bottom,
        usize::MAX,
    ] {
        pane.set_scroll_offset_from_bottom(offset);
        let after = pane.scroll_metrics().expect("scroll metrics after scroll");
        assert_eq!(
            after.offset_from_bottom,
            offset.min(after.max_offset_from_bottom)
        );
    }

    assert!(pane.visible_text().contains("000000"));
}

#[test]
fn empty_or_short_resize_keeps_following_bottom_when_output_creates_scrollback() {
    for initial in [b"".as_slice(), b"seed\r\n".as_slice()] {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::ghostty::Terminal::new(10, 3, 100).unwrap();
        terminal.write(initial);
        let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
        let pane_id = PaneId::from_raw(1);

        pane.resize(3, 10, 0, 0);
        pane.process_pty_bytes(
            pane_id,
            0,
            b"000000\r\n000001\r\n000002\r\n000003\r\n000004",
            &tx,
        );

        let metrics = pane.scroll_metrics().expect("scroll metrics after output");
        assert_eq!(metrics.offset_from_bottom, 0);
        assert!(pane.visible_text().contains("000004"));
    }
}

#[test]
fn resize_that_removes_scrollback_restores_live_follow() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(10, 3, 100).unwrap();
    terminal.write(b"000000\r\n000001\r\n000002\r\n000003\r\n000004");
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.set_scroll_offset_from_bottom(1);
    pane.resize(5, 10, 0, 0);
    let resized = pane.scroll_metrics().expect("scroll metrics after resize");
    assert_eq!(resized.max_offset_from_bottom, 0);

    pane.process_pty_bytes(pane_id, 0, b"\r\n000005\r\n000006", &tx);

    let metrics = pane.scroll_metrics().expect("scroll metrics after output");
    assert_eq!(metrics.offset_from_bottom, 0);
    assert!(pane.visible_text().contains("000006"));
}

#[test]
fn detection_text_stays_at_bottom_when_viewport_is_scrolled() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(80, 3, 100).unwrap();
    write_numbered_lines(&mut terminal, 10);
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let bottom_snapshot = pane.detection_text();
    assert_eq!(bottom_snapshot, pane.recent_text(3));
    assert!(bottom_snapshot.contains("000009"));

    let before = pane.scroll_metrics().expect("scroll metrics before scroll");
    pane.set_scroll_offset_from_bottom(before.max_offset_from_bottom);

    assert!(pane.visible_text().contains("000000"));
    assert_eq!(pane.detection_text(), bottom_snapshot);
}

#[test]
fn extract_selection_reads_screen_rows_not_current_viewport() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(8, 3, 1024).unwrap();
    write_numbered_lines(&mut terminal, 8);
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    pane.set_scroll_offset_from_bottom(3);
    let metrics = pane
        .scroll_metrics()
        .expect("scroll metrics after initial scroll");
    let mut selection =
        crate::selection::Selection::anchor(PaneId::from_raw(1), 0, 0, Some(metrics));
    selection.drag(5, 2, Rect::new(0, 0, 8, 3), Some(metrics));

    pane.scroll_reset();

    let text = pane
        .extract_selection(&selection)
        .expect("selection should extract text");
    assert_eq!(text, "000003\n000004\n000005");
}

#[test]
fn plain_text_reads_skip_wide_character_spacer_cells() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(40, 3, 100).unwrap();
    terminal.write("日本語テスト ABC 123".as_bytes());
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert_eq!(pane.visible_text(), "日本語テスト ABC 123\n");
    assert_eq!(pane.recent_text(3), "日本語テスト ABC 123\n");
    assert_eq!(pane.recent_unwrapped_text(3), "日本語テスト ABC 123");
    assert_eq!(pane.detection_text(), "日本語テスト ABC 123\n");
}

#[test]
fn visible_ansi_preserves_cell_style_sequences() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 100).unwrap();
    terminal.write(b"\x1b[31;1mred\x1b[0m plain");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let ansi = pane.visible_ansi();
    assert!(ansi.contains("red"));
    assert!(ansi.contains("plain"));
    assert!(ansi.contains("\x1b["));
}

#[test]
fn resize_shrinks_both_axes_with_cursor_at_old_bottom() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(8, 4, 10_000).unwrap();
    terminal.write(b"alpha\r\nbeta\r\ngamma\r\ndelta");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    pane.resize(3, 7, 8, 16);

    assert_eq!(pane.visible_text(), "beta\ngamma\ndelta\n");
    assert_eq!(pane.detection_text(), "beta\ngamma\ndelta\n");
    assert_eq!(
        pane.scroll_metrics(),
        Some(ScrollMetrics {
            offset_from_bottom: 0,
            max_offset_from_bottom: 1,
            viewport_rows: 3,
        })
    );
}

#[test]
fn resize_reflow_keeps_scrolled_viewport_and_bottom_detection_sane() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(12, 4, 10_000).unwrap();
    write_wrapped_contract_lines(&mut terminal, 40);
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let bottom_snapshot = pane.detection_text();
    assert!(bottom_snapshot.contains("END"));

    let initial = pane.scroll_metrics().expect("initial scroll metrics");
    assert!(initial.max_offset_from_bottom > 0);
    pane.set_scroll_offset_from_bottom(initial.max_offset_from_bottom / 2);
    assert!(!pane.visible_text().trim().is_empty());

    for (rows, cols) in [(4, 10), (4, 7), (6, 18), (3, 9), (5, 12)] {
        let before_resize = pane.scroll_metrics().expect("scroll metrics before resize");
        pane.resize(rows, cols, 0, 0);

        let metrics = pane.scroll_metrics().expect("scroll metrics after resize");
        assert_eq!(metrics.viewport_rows, rows as usize);
        assert_eq!(
            metrics.offset_from_bottom,
            before_resize
                .offset_from_bottom
                .min(metrics.max_offset_from_bottom)
        );
        assert!(
            metrics.offset_from_bottom > 0,
            "resize should preserve a scrolled viewport instead of jumping to bottom"
        );
        assert!(metrics.max_offset_from_bottom > 0);
        let visible = pane.visible_text();
        assert!(
                !visible.trim().is_empty(),
                "visible text should not be empty after resize to {rows}x{cols}; metrics={metrics:?}; detection={:?}; recent={:?}",
                pane.detection_text(),
                pane.recent_text(6)
            );
        assert!(
            pane.detection_text().contains("END"),
            "bottom detection should remain independent from the scrolled viewport after resize"
        );
    }
}

#[test]
fn resize_recovery_does_not_replay_history_when_visible_screen_was_blank() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 10_000).unwrap();
    terminal.write(b"old history\r\n\x1b[2J\x1b[H");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert!(pane.visible_text().trim().is_empty());
    assert!(pane.detection_text().trim().is_empty());

    pane.resize(3, 20, 0, 0);

    assert!(pane.visible_text().trim().is_empty());
    assert!(pane.detection_text().trim().is_empty());
    assert!(pane.recent_text(3).trim().is_empty());
}

#[test]
fn resize_recovery_does_not_replay_scrolled_history_over_blank_bottom() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(20, 3, 10_000).unwrap();
    write_numbered_lines(&mut terminal, 20);
    terminal.write(b"\x1b[2J\x1b[H");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert!(pane.detection_text().trim().is_empty());
    let metrics = pane.scroll_metrics().expect("scroll metrics");
    pane.set_scroll_offset_from_bottom(metrics.max_offset_from_bottom);
    assert!(!pane.visible_text().trim().is_empty());

    pane.resize(3, 20, 0, 0);

    assert!(pane.detection_text().trim().is_empty());
    assert!(pane.recent_text(3).trim().is_empty());
}

#[test]
fn process_pty_bytes_answers_xtwinops_size_queries() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.resize(24, 80, 9, 18);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[14t\x1b[16t\x1b[18t", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b[4;432;720t"),
            Bytes::from_static(b"\x1b[6;18;9t"),
            Bytes::from_static(b"\x1b[8;24;80t"),
        ]
    );
}

#[test]
fn xtwinops_size_queries_follow_successful_resize() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.resize(24, 80, 9, 18);
    pane.resize(30, 100, 10, 20);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[14t\x1b[16t\x1b[18t", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b[4;600;1000t"),
            Bytes::from_static(b"\x1b[6;20;10t"),
            Bytes::from_static(b"\x1b[8;30;100t"),
        ]
    );
}

#[test]
fn xtwinops_size_queries_stay_silent_without_pixel_geometry() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    for (cell_width_px, cell_height_px) in [(0, 0), (0, 18), (9, 0)] {
        pane.resize(24, 80, cell_width_px, cell_height_px);
        let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[14t\x1b[16t\x1b[18t", &tx);
        assert!(result.terminal_responses.is_empty());
    }
}

#[test]
fn resize_returns_in_band_size_report_response() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
    terminal.mode_set(2048, true).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let responses = pane.resize(40, 100, 9, 18);

    assert_eq!(
        responses,
        vec![Bytes::from_static(b"\x1B[48;40;100;720;900t")]
    );
}
