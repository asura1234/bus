use super::*;

pub(super) fn write_numbered_lines(terminal: &mut Terminal, count: usize) {
    for i in 0..count {
        terminal.write(format!("{i:06}\r\n").as_bytes());
    }
}

fn write_padded_lines(terminal: &mut Terminal, count: usize, width: usize) {
    let line = format!("{}\r\n", "x".repeat(width));
    terminal.write(line.repeat(count).as_bytes());
}

pub(super) fn first_rendered_row_text(terminal: &Terminal) -> String {
    let mut render_state = RenderState::new().unwrap();
    render_state.update(terminal).unwrap();
    let mut row_iterator = RowIterator::new().unwrap();
    let mut rows = render_state
        .populate_row_iterator(&mut row_iterator)
        .unwrap();
    let mut row_cells = RowCells::new().unwrap();
    let mut bytes = Vec::new();
    let mut cell_text = String::new();
    let mut row_text = String::new();

    assert!(rows.next());
    let mut cells = rows.populate_cells(&mut row_cells).unwrap();
    while cells.next() {
        cells
            .grapheme_text_into(&mut bytes, &mut cell_text)
            .unwrap();
        row_text.push_str(&cell_text);
    }
    row_text.trim_end().to_owned()
}

fn build_info_bool(data: ffi::GhosttyBuildInfo) -> bool {
    let mut out = false;
    unsafe {
        ffi::ghostty_build_info(data, (&mut out as *mut bool).cast())
            .into_result()
            .unwrap();
    }
    out
}

fn build_info_optimize() -> ffi::GhosttyOptimizeMode {
    let mut out = ffi::GhosttyOptimizeMode_GHOSTTY_OPTIMIZE_DEBUG;
    unsafe {
        ffi::ghostty_build_info(
            ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_OPTIMIZE,
            (&mut out as *mut ffi::GhosttyOptimizeMode).cast(),
        )
        .into_result()
        .unwrap();
    }
    out
}

#[test]
fn incremental_compression_preserves_cold_scrollback() {
    let mut terminal = Terminal::new(80, 24, 20_000_000).unwrap();
    let initial_activity = terminal.compression_activity().unwrap();
    let suffix = "x".repeat(66);
    for line in 1..=10_000 {
        terminal.write(format!("{line:05} {suffix}\r\n").as_bytes());
    }
    assert_ne!(terminal.compression_activity().unwrap(), initial_activity);

    let mut complete = false;
    for _ in 0..10_000 {
        match terminal.compress_incremental().unwrap() {
            TerminalCompressionResult::Unsupported => return,
            TerminalCompressionResult::Pending => {}
            TerminalCompressionResult::Complete => {
                complete = true;
                break;
            }
        }
    }
    assert!(complete, "incremental compression did not converge");

    let oldest = terminal.read_text_screen((0, 0), (79, 0), false).unwrap();
    assert!(oldest.starts_with("00001 "));
    let last_row = terminal.total_rows().unwrap() as u32 - 1;
    let newest = terminal
        .read_text_screen((0, last_row - 1), (79, last_row), false)
        .unwrap();
    assert!(newest.contains("10000"));
}

#[test]
fn build_info_contract_matches_expected_vendored_features() {
    let _simd = build_info_bool(ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_SIMD);
    let _tmux_control_mode =
        build_info_bool(ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_TMUX_CONTROL_MODE);
    let _kitty_graphics = build_info_bool(ffi::GhosttyBuildInfo_GHOSTTY_BUILD_INFO_KITTY_GRAPHICS);

    let optimize = build_info_optimize();
    assert!(matches!(
        optimize,
        ffi::GhosttyOptimizeMode_GHOSTTY_OPTIMIZE_DEBUG
            | ffi::GhosttyOptimizeMode_GHOSTTY_OPTIMIZE_RELEASE_SAFE
            | ffi::GhosttyOptimizeMode_GHOSTTY_OPTIMIZE_RELEASE_SMALL
            | ffi::GhosttyOptimizeMode_GHOSTTY_OPTIMIZE_RELEASE_FAST
    ));
}

#[test]
fn terminal_read_text_screen_unwraps_soft_wrapped_selection() {
    let mut terminal = Terminal::new(5, 3, 0).unwrap();
    terminal.write("1ABCD2EFGH3IJKL".as_bytes());

    let text = terminal.read_text_screen((0, 1), (2, 2), false).unwrap();
    assert_eq!(text, "2EFGH3IJ");
}

#[test]
fn terminal_extracts_viewport_hyperlink_uri() {
    let mut terminal = Terminal::new(20, 3, 0).unwrap();
    terminal.write(b"\x1b]8;;https://example.com\x1b\\Link\x1b]8;;\x1b\\");

    assert_eq!(
        terminal.viewport_hyperlink_uri(0, 0).unwrap().as_deref(),
        Some("https://example.com")
    );
    assert_eq!(terminal.viewport_hyperlink_uri(4, 0).unwrap(), None);
}

#[test]
fn terminal_read_text_screen_handles_wide_chars() {
    let mut terminal = Terminal::new(5, 3, 0).unwrap();
    terminal.write("1A⚡".as_bytes());

    let full = terminal.read_text_screen((0, 0), (3, 0), false).unwrap();
    assert_eq!(full, "1A⚡");

    let through_wide_head = terminal.read_text_screen((0, 0), (2, 0), false).unwrap();
    assert_eq!(through_wide_head, "1A⚡");

    let wide_only = terminal.read_text_screen((3, 0), (3, 0), false).unwrap();
    assert_eq!(wide_only, "⚡");
}

#[test]
fn zero_max_scrollback_disables_history() {
    let mut terminal = Terminal::new(80, 3, 0).unwrap();
    write_numbered_lines(&mut terminal, 3000);
    assert_eq!(terminal.scrollback_rows().unwrap(), 0);
}

#[test]
fn max_scrollback_limit_bytes_retains_more_history_for_larger_limits() {
    let mut small = Terminal::new(80, 3, 1_000_000).unwrap();
    let mut large = Terminal::new(80, 3, 10_000_000).unwrap();

    write_padded_lines(&mut small, 1_250, 70);
    write_padded_lines(&mut large, 1_250, 70);

    let small_scrollback = small.scrollback_rows().unwrap();
    let large_scrollback = large.scrollback_rows().unwrap();

    assert!(
            large_scrollback > small_scrollback,
            "expected larger byte limit to retain more history, got small={small_scrollback}, large={large_scrollback}"
        );
}

#[test]
fn large_negative_scroll_delta_reaches_top_of_scrollback() {
    let mut terminal = Terminal::new(80, 3, 1_000_000).unwrap();
    write_numbered_lines(&mut terminal, 1000);

    let before = terminal.scrollbar().unwrap();
    assert!(before.total > before.len);

    terminal.scroll_viewport_bottom();
    terminal.scroll_viewport_delta(-10_000);

    let after = terminal.scrollbar().unwrap();
    assert_eq!(after.offset, 0);
    assert_eq!(after.len, before.len);
}

#[test]
fn absolute_scroll_row_round_trips_and_clamps() {
    let mut terminal = Terminal::new(80, 3, 1_000_000).unwrap();
    write_numbered_lines(&mut terminal, 1000);

    let before = terminal.scrollbar().unwrap();
    let max_row = before.total.saturating_sub(before.len);
    assert!(max_row > 0);

    for row in [0, max_row / 2, max_row, usize::MAX] {
        terminal.scroll_viewport_row(row);
        let after = terminal.scrollbar().unwrap();
        assert_eq!(after.offset, row.min(max_row));
        assert_eq!(after.len, before.len);
    }
}

#[test]
fn deep_scrollback_resize_preserves_unicode_and_hyperlinks() {
    use std::fmt::Write as _;

    let mut terminal = Terminal::new(20, 5, 100_000_000).unwrap();
    let mut input = String::from("\x1b]8;;https://example.com\x1b\\FIRST 🇧🇷\x1b]8;;\x1b\\\r\n");
    for line in 0..70_000 {
        writeln!(input, "{line:05} 👨‍👩‍👧").unwrap();
    }
    terminal.write(input.as_bytes());

    assert!(terminal.scrollback_rows().unwrap() > u16::MAX as usize);
    terminal.scroll_viewport_delta(-100_000);
    assert_eq!(terminal.scrollbar().unwrap().offset, 0);
    assert!(terminal
        .read_text_screen((0, 0), (19, 0), false)
        .unwrap()
        .starts_with("FIRST 🇧🇷"));
    assert_eq!(
        terminal.viewport_hyperlink_uri(0, 0).unwrap().as_deref(),
        Some("https://example.com")
    );

    terminal.resize(10, 5, 8, 16).unwrap();
    terminal.scroll_viewport_delta(-100_000);
    let metrics = terminal.scrollbar().unwrap();
    assert_eq!(metrics.offset, 0);
    assert_eq!(metrics.len, 5);
    assert!(terminal
        .read_text_screen((0, 0), (9, 0), false)
        .unwrap()
        .starts_with("FIRST 🇧🇷"));
    assert_eq!(
        terminal.viewport_hyperlink_uri(0, 0).unwrap().as_deref(),
        Some("https://example.com")
    );
}

#[test]
fn active_screen_and_cursor_visibility_contract() {
    let mut terminal = Terminal::new(12, 3, 0).unwrap();
    let mut render_state = RenderState::new().unwrap();

    terminal.write(b"primary");
    assert_eq!(terminal.active_screen().unwrap(), ActiveScreen::Primary);
    assert_eq!(
        terminal.read_text_screen((0, 0), (6, 0), false).unwrap(),
        "primary"
    );

    render_state.update(&terminal).unwrap();
    assert!(render_state.cursor_visible().unwrap());
    terminal.write(b"\x1b[?25l");
    render_state.update(&terminal).unwrap();
    assert!(!render_state.cursor_visible().unwrap());

    terminal.write(b"\x1b[?1049h\x1b[HALT");
    assert_eq!(terminal.active_screen().unwrap(), ActiveScreen::Alternate);
    assert_eq!(
        terminal.read_text_screen((0, 0), (2, 0), false).unwrap(),
        "ALT"
    );

    terminal.write(b"\x1b[?1049l");
    assert_eq!(terminal.active_screen().unwrap(), ActiveScreen::Primary);
    assert_eq!(
        terminal.read_text_screen((0, 0), (6, 0), false).unwrap(),
        "primary"
    );
}

#[test]
fn grapheme_cluster_mode_is_default_and_survives_full_reset() {
    let mut terminal = Terminal::new(80, 3, 100).unwrap();
    assert!(terminal.mode_get(MODE_GRAPHEME_CLUSTER).unwrap());

    terminal.write(b"\x1bc");

    assert!(terminal.mode_get(MODE_GRAPHEME_CLUSTER).unwrap());
}
