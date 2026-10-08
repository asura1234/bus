use super::terminal::first_rendered_row_text;
use super::*;

#[test]
fn unicode_codepoint_width_matches_terminal_layout_rules() {
    assert_eq!(unicode_codepoint_width('A' as u32), 1);
    assert_eq!(unicode_codepoint_width('\u{301}' as u32), 0);
    assert_eq!(unicode_codepoint_width('界' as u32), 2);
    assert_eq!(unicode_codepoint_width(0x11_0000), 1);
}

#[test]
fn terminal_and_render_state_smoke_test() {
    let mut terminal = Terminal::new(8, 3, 100).unwrap();
    assert_eq!(terminal.cols().unwrap(), 8);
    assert_eq!(terminal.rows().unwrap(), 3);

    terminal.write(b"hello\r\nworld");

    let mut render_state = RenderState::new().unwrap();
    render_state.update(&terminal).unwrap();
    assert_eq!(render_state.cols().unwrap(), 8);
    assert_eq!(render_state.rows().unwrap(), 3);
    assert_ne!(render_state.dirty().unwrap(), Dirty::Clean);

    let mut row_iterator = RowIterator::new().unwrap();
    let mut row_iter = render_state
        .populate_row_iterator(&mut row_iterator)
        .unwrap();
    let mut row_cells = RowCells::new().unwrap();

    let mut found_hello = false;
    let mut found_world = false;
    let mut row_index = 0usize;
    while row_iter.next() {
        let _ = row_iter.dirty().unwrap();
        let mut cells = row_iter.populate_cells(&mut row_cells).unwrap();
        let mut line = String::new();
        while cells.next() {
            let text = cells.grapheme_text().unwrap();
            if text.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&text);
            }
        }
        let trimmed = line.trim_end().to_string();
        if row_index == 0 {
            found_hello = trimmed.starts_with("hello");
        }
        if row_index == 1 {
            found_world = trimmed.starts_with("world");
        }
        row_index += 1;
    }

    assert!(found_hello);
    assert!(found_world);

    render_state.set_dirty(Dirty::Clean).unwrap();
    assert_eq!(render_state.dirty().unwrap(), Dirty::Clean);
}

#[test]
fn render_cells_preserve_issue_453_unicode_payload_exactly() {
    const PAYLOAD: &str = "README 👨‍👩‍👧‍👦 🧑‍💻 ✅ ⚡ 漢字 café é 🏳️‍🌈 🚀";
    let mut terminal = Terminal::new(80, 3, 100).unwrap();
    assert!(terminal.mode_get(MODE_GRAPHEME_CLUSTER).unwrap());
    terminal.write(format!("{PAYLOAD}\r\n").as_bytes());

    assert_eq!(first_rendered_row_text(&terminal), PAYLOAD);
}

#[test]
fn screen_text_rows_preserve_wrap_and_grapheme_cells() {
    let mut terminal = Terminal::new(5, 3, 100).unwrap();
    terminal.write("abcdef\r\n界e\u{301}".as_bytes());

    let rows = terminal.screen_text_rows().unwrap();

    assert_eq!(rows.len(), 3);
    assert!(rows[0].soft_wrapped);
    assert!(!rows[0].wrap_continuation);
    assert!(!rows[1].soft_wrapped);
    assert!(rows[1].wrap_continuation);
    assert!(!rows[2].wrap_continuation);
    assert_eq!(rows[2].cells[0].wide, CellWide::Wide);
    assert_eq!(rows[2].cells[0].graphemes, vec!['界' as u32]);
    assert_eq!(rows[2].cells[1].wide, CellWide::SpacerTail);
    assert_eq!(rows[2].cells[2].graphemes, vec!['e' as u32, 0x301]);
}

#[test]
fn render_state_row_dirty_can_be_cleared_independently() {
    let mut terminal = Terminal::new(8, 3, 100).unwrap();
    let mut render_state = RenderState::new().unwrap();

    render_state.update(&terminal).unwrap();
    {
        let mut row_iterator = RowIterator::new().unwrap();
        let mut rows = render_state
            .populate_row_iterator(&mut row_iterator)
            .unwrap();
        while rows.next() {
            rows.clear_dirty().unwrap();
            assert!(!rows.dirty().unwrap());
        }
    }
    render_state.set_dirty(Dirty::Clean).unwrap();
    assert_eq!(render_state.dirty().unwrap(), Dirty::Clean);

    terminal.write(b"A");
    render_state.update(&terminal).unwrap();
    assert_eq!(render_state.dirty().unwrap(), Dirty::Partial);

    let mut dirty_rows = 0usize;
    {
        let mut row_iterator = RowIterator::new().unwrap();
        let mut rows = render_state
            .populate_row_iterator(&mut row_iterator)
            .unwrap();
        while rows.next() {
            if rows.dirty().unwrap() {
                dirty_rows += 1;
                rows.clear_dirty().unwrap();
                assert!(!rows.dirty().unwrap());
            }
        }
    }
    assert_eq!(dirty_rows, 1);
    assert_eq!(render_state.dirty().unwrap(), Dirty::Partial);

    render_state.set_dirty(Dirty::Clean).unwrap();
    assert_eq!(render_state.dirty().unwrap(), Dirty::Clean);
}

#[test]
fn row_selection_returns_none_without_selection() {
    let terminal = Terminal::new(8, 3, 100).unwrap();
    let mut render_state = RenderState::new().unwrap();
    render_state.update(&terminal).unwrap();

    let mut row_iterator = RowIterator::new().unwrap();
    let mut rows = render_state
        .populate_row_iterator(&mut row_iterator)
        .unwrap();
    assert!(rows.next());
    assert_eq!(rows.selection().unwrap(), None);
}

#[test]
fn row_cell_basic_data_uses_batched_vendor_reads() {
    let mut terminal = Terminal::new(8, 3, 100).unwrap();
    terminal.write(b"\x1b[31mA\x1b[0m");

    let mut render_state = RenderState::new().unwrap();
    render_state.update(&terminal).unwrap();

    let mut row_iterator = RowIterator::new().unwrap();
    let mut rows = render_state
        .populate_row_iterator(&mut row_iterator)
        .unwrap();
    assert!(rows.next());

    let mut row_cells = RowCells::new().unwrap();
    let mut cells = rows.populate_cells(&mut row_cells).unwrap();
    assert!(cells.next());

    let basic = cells.basic_data().unwrap();
    assert_eq!(basic.wide, CellWide::Narrow);
    assert!(basic.has_styling);
    assert_eq!(basic.style.fg_color, Some(CellColor::Palette(1)));
    assert!(!basic.has_hyperlink);
}
