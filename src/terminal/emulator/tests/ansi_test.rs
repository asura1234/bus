use crate::protocol::ansi::{blit_frame_to, BlitEncoder};
use crate::protocol::wire::{CellData, FrameData};

fn make_cell(symbol: &str, fg: u32, bg: u32, modifier: u16) -> CellData {
    CellData {
        symbol: symbol.to_owned(),
        fg,
        bg,
        modifier,
        skip: false,
        hyperlink: None,
    }
}

fn make_frame(width: u16, height: u16, cells: Vec<CellData>) -> FrameData {
    FrameData {
        cells,
        width,
        height,
        cursor: None,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    }
}

#[test]
fn batched_ascii_diff_replays_to_current_frame() {
    let prev = make_frame(4, 3, vec![make_cell("A", 0, 0, 0); 12]);
    let curr = make_frame(4, 3, vec![make_cell("B", 0, 0, 0); 12]);
    let mut terminal = crate::terminal::vt::Terminal::new(4, 3, 0).unwrap();

    let mut initial = Vec::new();
    blit_frame_to(&mut initial, &prev, None);
    terminal.write(&initial);

    let mut diff = Vec::new();
    blit_frame_to(&mut diff, &curr, Some(&prev));
    terminal.write(&diff);

    for row in 0..3 {
        for col in 0..4 {
            let (_, graphemes) = terminal.screen_cell(col, row).unwrap();
            assert_eq!(graphemes, vec![u32::from('B')]);
        }
    }
}

#[test]
fn encoder_size_change_clears_cells_outside_the_new_frame() {
    let prev = make_frame(3, 2, vec![make_cell("A", 0, 0, 0); 6]);
    let curr = make_frame(2, 2, vec![make_cell("B", 0, 0, 0); 4]);
    let mut encoder = BlitEncoder::new();
    let mut terminal = crate::terminal::vt::Terminal::new(3, 2, 0).unwrap();
    let initial = encoder.encode(&prev, false);
    terminal.write(&initial.bytes);
    encoder.commit(prev, initial);

    // The host grid is still three columns wide; the frame is two.
    let encoded = encoder.encode(&curr, false);
    terminal.write(&encoded.bytes);
    let output = String::from_utf8(encoded.bytes).unwrap();
    assert!(output.contains("\x1b[2J"));
    assert!(output.bytes().filter(|byte| *byte == b'B').count() >= 4);
    for row in 0..2 {
        let (_, graphemes) = terminal.screen_cell(2, row).unwrap();
        assert!(
            graphemes.iter().all(|code| *code == u32::from(' ')),
            "row {row} keeps a cell from the wider frame: {graphemes:?}"
        );
    }
}

/// Replays a host terminal that, like Terminal.app, keeps cells beyond a
/// narrowed window: the emulator grid stays at the widest size, and text
/// the host revealed is written into it directly.
fn host_text(terminal: &crate::terminal::vt::Terminal, width: u16, rows: u32) -> Vec<String> {
    (0..rows)
        .map(|row| {
            (0..width)
                .map(|col| {
                    let (_, graphemes) = terminal.screen_cell(col, row).unwrap();
                    graphemes
                        .first()
                        .and_then(|code| char::from_u32(*code))
                        .unwrap_or(' ')
                })
                .collect()
        })
        .collect()
}

fn present_resized(
    encoder: &mut BlitEncoder,
    terminal: &mut crate::terminal::vt::Terminal,
    frame: FrameData,
) {
    // What the client does for every resize event.
    encoder.invalidate();
    let encoded = encoder.encode(&frame, true);
    assert!(String::from_utf8_lossy(&encoded.bytes).contains("\x1b[2J"));
    terminal.write(&encoded.bytes);
    encoder.commit(frame, encoded);
}

#[test]
fn every_resize_clears_cells_the_host_kept_beyond_the_frame() {
    let mut encoder = BlitEncoder::new();
    let mut terminal = crate::terminal::vt::Terminal::new(6, 2, 0).unwrap();
    let wide = make_frame(6, 2, vec![make_cell("W", 0, 0, 0); 12]);
    let initial = encoder.encode(&wide, false);
    terminal.write(&initial.bytes);
    encoder.commit(wide, initial);

    // Wider, then narrower: the wide frame's right cells must go.
    present_resized(
        &mut encoder,
        &mut terminal,
        make_frame(4, 2, vec![make_cell("n", 0, 0, 0); 8]),
    );
    assert_eq!(host_text(&terminal, 6, 2), ["nnnn  ", "nnnn  "]);

    // Narrower, then wider: the host reveals old text it kept at the
    // right edge; the wider frame covers it.
    terminal.write(b"\x1b[1;5Hru\x1b[2;5Hfi");
    present_resized(
        &mut encoder,
        &mut terminal,
        make_frame(6, 2, vec![make_cell(" ", 0, 0, 0); 12]),
    );
    assert_eq!(host_text(&terminal, 6, 2), ["      ", "      "]);

    // A drag that ends at the frame's own size still clears.
    terminal.write(b"\x1b[1;5Hll\x1b[2;5Hom");
    present_resized(
        &mut encoder,
        &mut terminal,
        make_frame(6, 2, vec![make_cell(" ", 0, 0, 0); 12]),
    );
    assert_eq!(host_text(&terminal, 6, 2), ["      ", "      "]);
}

#[test]
fn drawn_cursor_remains_visible_on_a_wide_character_continuation() {
    let mut source = crate::terminal::vt::Terminal::new(4, 2, 0).unwrap();
    source.write("界\x1b[1;2H".as_bytes());
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let pane = super::GhosttyPaneTerminal::new(source, tx).unwrap();
    let cursor = pane.cursor_state().expect("visible pane cursor");
    assert!(cursor.visible);

    let mut rendered = ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 2)).unwrap();
    rendered
        .draw(|frame| pane.render(frame, ratatui::layout::Rect::new(0, 0, 4, 2), false))
        .unwrap();
    let frame = FrameData::from_ratatui_buffer(
        rendered.backend().buffer(),
        Some(crate::protocol::wire::CursorState {
            x: cursor.x,
            y: cursor.y,
            visible: cursor.visible,
            shape: cursor.shape,
        }),
    );
    let drawn = crate::protocol::ansi::frame_with_drawn_cursor(frame);
    let encoded = BlitEncoder::new().encode_with_suppressed_visible_cursor(&drawn, false);
    let mut host = crate::terminal::vt::Terminal::new(4, 2, 0).unwrap();
    host.write(&encoded.bytes);

    let mut state = crate::terminal::vt::RenderState::new().unwrap();
    state.update(&host).unwrap();
    let mut iterator = crate::terminal::vt::RowIterator::new().unwrap();
    let mut rows = state.populate_row_iterator(&mut iterator).unwrap();
    assert!(rows.next());
    let mut row_cells = crate::terminal::vt::RowCells::new().unwrap();
    let mut cells = rows.populate_cells(&mut row_cells).unwrap();
    assert!(cells.next());
    assert_eq!(cells.grapheme_text().unwrap(), "界");
    assert!(
        cells.basic_data().unwrap().style.inverse,
        "drawn cursor at ({}, {}) must remain visible on the wide glyph",
        cursor.x,
        cursor.y
    );
}
