use super::*;

use ratatui::{layout::Rect, style::Color};

use tokio::sync::mpsc;

use support::*;

fn text_cell(text: &str) -> crate::terminal::vt::ScreenTextCell {
    crate::terminal::vt::ScreenTextCell {
        wide: crate::terminal::vt::CellWide::Narrow,
        graphemes: text.chars().map(u32::from).collect(),
    }
}

fn wide_text_cells(text: &str) -> [crate::terminal::vt::ScreenTextCell; 2] {
    [
        crate::terminal::vt::ScreenTextCell {
            wide: crate::terminal::vt::CellWide::Wide,
            graphemes: text.chars().map(u32::from).collect(),
        },
        crate::terminal::vt::ScreenTextCell {
            wide: crate::terminal::vt::CellWide::SpacerTail,
            graphemes: Vec::new(),
        },
    ]
}

fn text_row(
    cells: impl IntoIterator<Item = crate::terminal::vt::ScreenTextCell>,
    soft_wrapped: bool,
) -> crate::terminal::vt::ScreenTextRow {
    crate::terminal::vt::ScreenTextRow {
        cells: cells.into_iter().collect(),
        soft_wrapped,
        wrap_continuation: false,
    }
}

fn search_primary(
    buffer: &RetainedTextBuffer,
    query: &str,
    case_sensitive: bool,
) -> Vec<TerminalTextMatch> {
    buffer
        .search_window(
            query,
            case_sensitive,
            crate::terminal::vt::ActiveScreen::Primary,
            TerminalSearchDirection::Forward,
            TerminalTextPoint { row: 0, col: 0 },
            None,
            usize::MAX,
        )
        .matches
}

fn write_numbered_lines(terminal: &mut crate::terminal::vt::Terminal, count: usize) {
    for i in 0..count {
        terminal.write(format!("{i:06}\r\n").as_bytes());
    }
}

fn write_wrapped_contract_lines(terminal: &mut crate::terminal::vt::Terminal, count: usize) {
    for i in 0..count {
        terminal.write(format!("WRAP-{i:03}-abcdefghijklmnopqrstuvwxyz\r\n").as_bytes());
    }
    terminal.write(b"END");
}

fn ghostty_normalize_buffer_symbol(symbol: &str, wide: crate::terminal::vt::CellWide) -> String {
    if ghostty_symbol_fits_cell(symbol, wide) {
        symbol.to_string()
    } else {
        ghostty_blank_symbol_for_width(wide).to_string()
    }
}

fn render_cells_to_symbols(
    terminal: &mut crate::terminal::vt::Terminal,
) -> Vec<(crate::terminal::vt::CellWide, String)> {
    let mut render_state = crate::terminal::vt::RenderState::new().unwrap();
    render_state.update(terminal).unwrap();

    let mut row_iterator = crate::terminal::vt::RowIterator::new().unwrap();
    let mut rows = render_state
        .populate_row_iterator(&mut row_iterator)
        .unwrap();
    let mut row_cells = crate::terminal::vt::RowCells::new().unwrap();
    let mut grapheme_bytes = Vec::new();
    let mut symbol_scratch = String::new();
    let mut out = Vec::new();

    if rows.next() {
        let mut cells = rows.populate_cells(&mut row_cells).unwrap();
        while cells.next() {
            let wide = cells
                .wide()
                .unwrap_or(crate::terminal::vt::CellWide::Narrow);
            let symbol = ghostty_buffer_symbol_into(
                &cells,
                wide,
                false,
                &mut grapheme_bytes,
                &mut symbol_scratch,
            )
            .unwrap()
            .to_string();
            out.push((wide, symbol));
        }
    }

    out
}

#[test]
fn dirty_full_collects_bounded_viewport_patch() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(4, 3, 200).unwrap();
    terminal.write(b"one\r\ntwo\r\nthree");
    let pane = PaneTerminal::new(GhosttyPaneTerminal::new(terminal, tx).unwrap());

    let patch = match pane.collect_dirty_patch(4, 3) {
        TerminalDirtyPatchOutcome::Patch(patch) => patch,
        outcome => panic!("expected viewport patch, got {outcome:?}"),
    };

    assert_eq!(patch.rows.len(), 3);
    assert_eq!(
        patch.rows.iter().map(|(row, _)| *row).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert!(patch.rows.iter().all(|(_, cells)| cells.len() == 4));
    assert!(matches!(
        pane.collect_dirty_patch(4, 3),
        TerminalDirtyPatchOutcome::Clean
    ));
}

#[test]
fn palette_overrides_are_none_without_an_osc4_write() {
    let default = [rgb(1, 2, 3); 256];
    assert!(PaletteOverrides::new(&default, &default).is_none());
}

#[test]
fn redefined_palette_entries_render_as_rgb_and_others_stay_indexed() {
    let default = [rgb(1, 2, 3); 256];
    let mut active = default;
    active[18] = rgb(169, 177, 214);
    let overrides = PaletteOverrides::new(&active, &default).expect("index 18 differs");

    assert_eq!(
        ghostty_cell_color(
            crate::terminal::vt::CellColor::Palette(18),
            Some(&overrides)
        ),
        Color::Rgb(169, 177, 214)
    );
    // Untouched entries keep being forwarded, so they still follow the host theme.
    assert_eq!(
        ghostty_cell_color(
            crate::terminal::vt::CellColor::Palette(19),
            Some(&overrides)
        ),
        Color::Indexed(19)
    );
    // ...and so does everything when the program never wrote a palette at all.
    assert_eq!(
        ghostty_cell_color(crate::terminal::vt::CellColor::Palette(18), None),
        Color::Indexed(18)
    );
}

#[test]
fn direct_rgb_cells_are_unaffected_by_palette_overrides() {
    let default = [rgb(1, 2, 3); 256];
    let mut active = default;
    active[18] = rgb(169, 177, 214);
    let overrides = PaletteOverrides::new(&active, &default).expect("index 18 differs");
    assert_eq!(
        ghostty_cell_color(
            crate::terminal::vt::CellColor::Rgb(rgb(122, 162, 247)),
            Some(&overrides)
        ),
        Color::Rgb(122, 162, 247)
    );
}

#[test]
fn decscusr_cursor_shape_preserves_blinking_variants() {
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::Block, true),
        1
    );
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::Block, false),
        2
    );
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::Underline, true),
        3
    );
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::Underline, false),
        4
    );
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::Bar, true),
        5
    );
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::Bar, false),
        6
    );
    assert_eq!(
        decscusr_cursor_shape(crate::terminal::vt::CursorVisualStyle::BlockHollow, false),
        2
    );
}

#[test]
fn cursor_state_uses_terminal_default_until_child_sets_shape() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert_eq!(pane.cursor_state().unwrap().shape, 0);

    pane.process_pty_bytes(pane_id, 0, b"\x1b[6 q", &tx, |_| None);

    assert_eq!(pane.cursor_state().unwrap().shape, 6);
}

#[test]
fn cursor_state_returns_terminal_default_after_decscusr_reset() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"\x1b[2 q", &tx, |_| None);
    assert_eq!(pane.cursor_state().unwrap().shape, 2);

    pane.process_pty_bytes(pane_id, 0, b"\x1b[0 q", &tx, |_| None);

    assert_eq!(pane.cursor_state().unwrap().shape, 0);
}

#[test]
fn cursor_state_returns_terminal_default_after_ris() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();

    pane.process_pty_bytes(PaneId::from_raw(1), 0, b"\x1b[6 q", &tx, |_| None);
    assert_eq!(pane.cursor_state().unwrap().shape, 6);

    pane.process_pty_bytes(PaneId::from_raw(1), 0, b"\x1bc", &tx, |_| None);
    assert_eq!(pane.cursor_state().unwrap().shape, 0);
}

#[test]
fn cursor_shape_tracker_ignores_oversized_decscusr_parameter() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();

    pane.process_pty_bytes(
        PaneId::from_raw(1),
        0,
        b"\x1b[65537 qstill alive",
        &tx,
        |_| None,
    );

    assert!(pane.visible_text().contains("still alive"));
    assert_eq!(pane.cursor_state().unwrap().shape, 0);
}

#[test]
fn cursor_shape_tracker_handles_split_decscusr_sequences() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"\x1b[", &tx, |_| None);
    pane.process_pty_bytes(pane_id, 0, b"5 ", &tx, |_| None);
    pane.process_pty_bytes(pane_id, 0, b"q", &tx, |_| None);

    assert_eq!(pane.cursor_state().unwrap().shape, 5);
}

#[test]
#[cfg(windows)]
fn cursor_state_holds_pty_position_change_until_settle_window() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"x", &tx, |_| None);
    assert_eq!(
        pane.cursor_state()
            .map(|cursor| (cursor.x, cursor.y, cursor.visible)),
        Some((1, 0, true))
    );

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[6;21H", &tx, |_| None);

    assert_eq!(result.render_delay, Some(CURSOR_POSITION_SETTLE));
    assert_eq!(
        pane.cursor_state()
            .map(|cursor| (cursor.x, cursor.y, cursor.visible)),
        Some((1, 0, true))
    );
}

#[test]
#[cfg(not(windows))]
fn cursor_state_uses_live_position_when_settle_policy_disabled() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"x", &tx, |_| None);
    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[6;21H", &tx, |_| None);

    assert_eq!(result.render_delay, None);
    assert_eq!(
        pane.cursor_state()
            .map(|cursor| (cursor.x, cursor.y, cursor.visible)),
        Some((20, 5, true))
    );
}

#[test]
fn cursor_settle_policy_controls_render_delay() {
    assert_eq!(
        render_delay_after_pty_write(false, false, true, true),
        Some(CURSOR_POSITION_SETTLE)
    );
    assert_eq!(
        render_delay_after_pty_write(false, false, true, false),
        None
    );
    assert_eq!(
        render_delay_after_pty_write(false, true, true, false),
        Some(KITTY_GRAPHICS_REDRAW_SETTLE)
    );
    assert_eq!(render_delay_after_pty_write(true, false, true, true), None);
}

#[test]
fn host_terminal_theme_restore_probe_skips_when_no_transient_override() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let core = pane.core.lock().unwrap();

    assert!(!should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn host_terminal_theme_restore_probe_skips_when_host_theme_unknown() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.transient_default_color_owner_pgid = Some(42);
    }
    let core = pane.core.lock().unwrap();

    assert!(!should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn host_terminal_theme_restore_probe_skips_on_alternate_screen() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1049h");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.transient_default_color_owner_pgid = Some(42);
        core.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
            foreground: Some(crate::utils::theme::color::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::utils::theme::color::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
    }
    let core = pane.core.lock().unwrap();

    assert!(!should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn host_terminal_theme_restore_probe_runs_when_restore_is_pending() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.transient_default_color_owner_pgid = Some(42);
        core.host_terminal_theme = crate::utils::theme::color::TerminalTheme {
            foreground: Some(crate::utils::theme::color::RgbColor {
                r: 0xaa,
                g: 0xbb,
                b: 0xcc,
            }),
            background: Some(crate::utils::theme::color::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }),
            ..Default::default()
        };
    }
    let core = pane.core.lock().unwrap();

    assert!(should_probe_host_terminal_theme_restore(&core));
}

#[test]
fn ghostty_render_can_suppress_cursor_position() {
    let (tx, _rx) = mpsc::channel(4);
    let mut first_terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    first_terminal.write(b"left");
    let first = GhosttyPaneTerminal::new(first_terminal, tx.clone()).unwrap();

    let mut second_terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    second_terminal.write(b"r\r\nb");
    let second = GhosttyPaneTerminal::new(second_terminal, tx).unwrap();

    let backend = ratatui::backend::TestBackend::new(40, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            first.render(frame, Rect::new(0, 0, 20, 5), true);
            second.render(frame, Rect::new(20, 0, 20, 5), false);
        })
        .unwrap();

    terminal.backend_mut().assert_cursor_position((4, 0));
}

#[test]
fn ghostty_normalize_buffer_symbol_prefers_grapheme_width_when_metadata_disagrees() {
    const WIDE_GRAPHEME: &str = "🙂";
    const FLAG_GRAPHEME: &str = "🇧🇷";
    const FAMILY_GRAPHEME: &str = "👨‍👩‍👧";
    const VS16_GRAPHEME: &str = "⚠️";
    const EMOJI_GRAPHEME: &str = "💳";

    assert_eq!(
        ghostty_normalize_buffer_symbol(WIDE_GRAPHEME, crate::terminal::vt::CellWide::Wide),
        WIDE_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("a", crate::terminal::vt::CellWide::Wide),
        "  "
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(FLAG_GRAPHEME, crate::terminal::vt::CellWide::Wide),
        FLAG_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(FAMILY_GRAPHEME, crate::terminal::vt::CellWide::Wide),
        FAMILY_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("⌨️", crate::terminal::vt::CellWide::Narrow),
        "⌨️"
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(VS16_GRAPHEME, crate::terminal::vt::CellWide::Narrow),
        VS16_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(EMOJI_GRAPHEME, crate::terminal::vt::CellWide::Narrow),
        EMOJI_GRAPHEME
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol(" ", crate::terminal::vt::CellWide::SpacerTail),
        ""
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("xx", crate::terminal::vt::CellWide::SpacerHead),
        " "
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("ｶ\u{ff9e}", crate::terminal::vt::CellWide::Wide),
        "ｶ\u{ff9e}"
    );
    assert_eq!(
        ghostty_normalize_buffer_symbol("ﾊ\u{ff9f}", crate::terminal::vt::CellWide::Wide),
        "ﾊ\u{ff9f}"
    );
}

#[test]
fn grapheme_cluster_mode_renders_flag_emoji_in_single_wide_cell() {
    let mut terminal = crate::terminal::vt::Terminal::new(40, 1, 0).unwrap();
    terminal.write("🇧🇷".as_bytes());

    let cells = render_cells_to_symbols(&mut terminal);

    assert!(
        cells
            .iter()
            .any(|(wide, symbol)| *wide == crate::terminal::vt::CellWide::Wide && symbol == "🇧🇷"),
        "expected a wide cell containing the full flag grapheme, got {cells:?}"
    );
}

#[test]
fn grapheme_cluster_mode_renders_zwj_family_in_single_wide_cell() {
    let mut terminal = crate::terminal::vt::Terminal::new(40, 1, 0).unwrap();
    terminal.write("👨\u{200d}👩\u{200d}👧".as_bytes());

    let cells = render_cells_to_symbols(&mut terminal);

    assert!(
        cells.iter().any(
            |(wide, symbol)| *wide == crate::terminal::vt::CellWide::Wide
                && symbol == "👨\u{200d}👩\u{200d}👧"
        ),
        "expected a wide cell containing the full ZWJ grapheme, got {cells:?}"
    );
}

#[test]
fn halfwidth_katakana_voiced_marks_render() {
    let mut terminal = crate::terminal::vt::Terminal::new(40, 1, 0).unwrap();
    terminal.write("ｱｲｳｴｵ ｶﾞｷﾞｸﾞｹﾞｺﾞ ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ".as_bytes());

    let cells = render_cells_to_symbols(&mut terminal);
    let rendered: String = cells.iter().map(|(_, symbol)| symbol.as_str()).collect();

    assert!(
        rendered.contains("ｱｲｳｴｵ ｶﾞｷﾞｸﾞｹﾞｺﾞ ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ"),
        "expected halfwidth katakana with voiced marks to survive, got {cells:?}"
    );
}

#[test]
fn render_keeps_halfwidth_katakana_voiced_tail_empty() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(20, 1, 0).unwrap();
    terminal.write("ｶﾞZ".as_bytes());
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let backend = ratatui::backend::TestBackend::new(20, 1);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            pane.render(frame, Rect::new(0, 0, 20, 1), false);
            assert_eq!(
                frame.buffer_mut()[(1, 0)].symbol(),
                "",
                "pane rendering keeps the wide spacer tail empty"
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer();

    assert_eq!(buffer[(0, 0)].symbol(), "ｶ\u{ff9e}");
    assert_eq!(
        buffer[(1, 0)].symbol(),
        " ",
        "Ratatui skips the covered tail instead of sending the empty cell to the backend"
    );
    assert_eq!(buffer[(2, 0)].symbol(), "Z");
}

#[test]
fn seeded_history_is_rendered_on_next_draw() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    pane.seed_history_ansi("restored history");

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let row = (0..16).map(|x| buffer[(x, 0)].symbol()).collect::<String>();
    assert_eq!(row, "restored history");
}

#[test]
fn render_leaves_unknown_host_default_background_transparent() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
}

#[test]
fn render_blanks_kitty_unicode_placeholders_when_graphics_enabled() {
    crate::protocol::kitty::set_enabled(true);
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal
            .write("before\u{10eeee}\u{0305}\u{0305}after".as_bytes());
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();
    crate::protocol::kitty::set_enabled(false);

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "b");
    assert_eq!(buffer[(6, 0)].symbol(), " ");
    assert_eq!(buffer[(7, 0)].symbol(), "a");
    assert_eq!(pane.visible_text().lines().next(), Some("before after"));
    assert_eq!(pane.recent_text(5), "before after\n");
}

#[test]
fn render_keeps_explicit_cell_foreground_when_host_is_unknown() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[38;2;68;85;102mhi\x1b[0m");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_fg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, expected_fg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
}

#[test]
fn render_keeps_explicit_cell_background_when_host_is_unknown() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[48;2;68;85;102mhi\x1b[0m");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_bg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().bg, expected_bg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
}

#[test]
fn render_preserves_palette_colors_instead_of_flattening_to_rgb() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(
            b"\x1b[31mR\x1b[0m \x1b[38;5;171mI\x1b[0m \x1b[48;5;4mB\x1b[0m \x1b[38;2;1;2;3mT",
        );
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "R");
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Indexed(1)));
    assert_eq!(buffer[(2, 0)].symbol(), "I");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Indexed(171)));
    assert_eq!(buffer[(4, 0)].symbol(), "B");
    assert_eq!(buffer[(4, 0)].style().bg, Some(Color::Indexed(4)));
    assert_eq!(buffer[(6, 0)].symbol(), "T");
    assert_eq!(buffer[(6, 0)].style().fg, Some(Color::Rgb(1, 2, 3)));
}

#[test]
fn render_preserves_palette_background_fill_cells() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[48;5;4m\x1b[K");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    for x in 0..20 {
        assert_eq!(buffer[(x, 0)].symbol(), " ");
        assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Indexed(4)));
    }
}

#[test]
fn render_preserves_rgb_background_fill_cells() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[48;2;17;34;51m\x1b[K");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    for x in 0..20 {
        assert_eq!(buffer[(x, 0)].symbol(), " ");
        assert_eq!(buffer[(x, 0)].style().bg, Some(Color::Rgb(17, 34, 51)));
    }
}

#[test]
fn render_preserves_underline_color() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[4m\x1b[58:2::17:34:51mU");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let style = terminal.backend().buffer()[(0, 0)].style();
    assert!(style.add_modifier.contains(Modifier::UNDERLINED));
    assert_eq!(style.underline_color, Some(Color::Rgb(17, 34, 51)));
}

#[test]
fn full_frame_preserves_curly_underline_style() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b[4:3mU");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let frame =
        crate::protocol::wire::FrameData::from_ratatui_buffer(terminal.backend().buffer(), None);
    assert_eq!(frame.cells[0].symbol, "U");
    assert_eq!(
        crate::protocol::wire::underline_style_from_modifier(frame.cells[0].modifier),
        3
    );
}

#[test]
fn render_leaves_host_default_background_transparent() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let host_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(0, 0)].style().bg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, Some(Color::Reset));
    assert_eq!(buffer[(2, 0)].style().bg, Some(Color::Reset));
}

#[test]
fn render_keeps_explicit_default_foreground_when_it_differs_from_host() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let host_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b]10;rgb:44/55/66\x1b\\hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_fg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().fg, expected_fg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().fg, expected_fg);
}

#[test]
fn render_keeps_explicit_default_background_when_it_differs_from_host() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let host_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        core.terminal.write(b"\x1b]11;rgb:44/55/66\x1b\\hi");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let expected_bg = Some(Color::Rgb(0x44, 0x55, 0x66));
    assert_eq!(buffer[(0, 0)].symbol(), "h");
    assert_eq!(buffer[(0, 0)].style().bg, expected_bg);
    assert_eq!(buffer[(2, 0)].symbol(), " ");
    assert_eq!(buffer[(2, 0)].style().bg, expected_bg);
}

#[test]
fn render_inverse_text_swaps_fg_and_resolved_bg_when_bg_is_transparent() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let host_theme = crate::utils::theme::color::TerminalTheme {
        foreground: Some(crate::utils::theme::color::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        background: Some(crate::utils::theme::color::RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        }),
        ..Default::default()
    };
    pane.apply_host_terminal_theme(host_theme);
    {
        let mut core = pane.core.lock().unwrap();
        // SGR 7 enables inverse/reverse video
        core.terminal.write(b"\x1b[7mhi\x1b[27m");
    }

    let backend = ratatui::backend::TestBackend::new(20, 5);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| pane.render(frame, Rect::new(0, 0, 20, 5), false))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let cell = &buffer[(0, 0)];
    assert_eq!(cell.symbol(), "h");
    // After inverse: fg should be the resolved bg, bg should be the original fg.
    // fg must NOT be Color::Reset (which would be the same hue as bg).
    assert_eq!(cell.style().fg, Some(Color::Rgb(0x11, 0x22, 0x33)));
    assert_eq!(cell.style().bg, Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
}

#[path = "ansi_test.rs"]
mod ansi;

#[path = "color_replies_test.rs"]
mod color_replies;

#[path = "encode_test.rs"]
mod encode;

#[path = "support_test.rs"]
mod support;

#[path = "write_test.rs"]
mod write;

#[path = "read_test.rs"]
mod read;
#[path = "recent_test.rs"]
mod recent;
#[path = "search_test.rs"]
mod search;
#[path = "text_motion_test.rs"]
mod text_motion;
