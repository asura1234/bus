use super::*;

fn screen_with(bytes: &[u8]) -> Screen {
    let mut screen = Screen::new(40, 6).unwrap();
    screen.feed(bytes);
    screen
}

#[test]
fn rows_read_back_the_text_the_client_drew() {
    let screen = screen_with(b"\x1b[2;3Hhello\x1b[4;1Hworld  ");
    let grid = screen.grid();
    assert_eq!(grid.row_text(1), "  hello");
    assert_eq!(grid.row_text(3), "world");
    assert_eq!(grid.rows_text().len(), 6);
}

#[test]
fn wide_characters_take_two_cells_and_matches_report_cell_columns() {
    let screen = screen_with("界 ok".as_bytes());
    let grid = screen.grid();
    assert_eq!(grid.row_text(0), "界 ok");
    let matches = find(&grid, &Needle::Text("ok"), None);
    assert_eq!(
        matches,
        vec![Match {
            row: 0,
            col: 3,
            width: 2,
            text: "ok".into()
        }]
    );
}

#[test]
fn regex_and_row_ranges_narrow_matches() {
    let screen = screen_with(b"a1 a2\r\na3");
    let grid = screen.grid();
    let regex = Needle::Regex(regex::Regex::new(r"a\d").unwrap());
    assert_eq!(find(&grid, &regex, None).len(), 3);
    assert_eq!(find(&grid, &regex, Some((1, 1))).len(), 1);
    assert!(find(&grid, &Needle::Text(""), None).is_empty());
}

#[test]
fn styles_come_back_as_named_colours_and_attributes() {
    let screen = screen_with(b"\x1b[1;38;2;255;0;0mR\x1b[0m");
    let grid = screen.grid();
    let cell = &grid.cells[0][0];
    assert_eq!(cell.symbol, "R");
    assert_eq!(cell.fg, "#ff0000");
    assert!(cell.attrs.contains(&"bold"));
}

#[test]
fn frame_ends_are_counted_across_chunk_boundaries() {
    let mut tail = Vec::new();
    assert_eq!(count_frame_ends(&mut tail, b"x\x1b[?20"), 0);
    assert_eq!(count_frame_ends(&mut tail, b"26ly\x1b[?2026l"), 2);
    assert_eq!(count_frame_ends(&mut tail, b""), 0);
    let mut screen = Screen::new(10, 2).unwrap();
    assert_eq!(screen.feed(b"\x1b[?2026hA\x1b[?2026l").frames, 1);
}

#[test]
fn resize_changes_the_grid_and_queries_get_answers() {
    let mut screen = Screen::new(20, 4).unwrap();
    screen.resize(30, 5);
    let grid = screen.grid();
    assert_eq!((grid.cols, grid.rows), (30, 5));
    // A device status report must be answered back to the client.
    let feed = screen.feed(b"\x1b[6n");
    assert!(!feed.responses.is_empty());
}

#[test]
fn clipboard_writes_and_paste_mode_are_observed() {
    let mut screen = Screen::new(20, 4).unwrap();
    assert!(!screen.bracketed_paste());
    let feed = screen.feed(b"\x1b[?2004h\x1b]52;c;aGk=\x07");
    assert!(screen.bracketed_paste());
    assert_eq!(feed.clipboard_writes.len(), 1);
    let _ = screen.keyboard_protocol();
}

#[test]
fn blank_grids_and_text_join_rows() {
    let grid = Grid::blank(3, 2);
    assert_eq!(grid.text(), "\n");
    assert!(grid.cursor.is_none());
}
