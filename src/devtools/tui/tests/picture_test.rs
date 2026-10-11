use super::*;
use crate::devtools::tui::screen::Grid;

fn pixel(path: &Path, x: usize, y: usize) -> [u8; 3] {
    let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut data).unwrap();
    let at = (y * info.width as usize + x) * 3;
    [data[at], data[at + 1], data[at + 2]]
}

#[test]
fn cells_draw_their_background_glyphs_and_box_lines() {
    let mut grid = Grid::blank(4, 2);
    grid.cells[0][0].bg = "#ff0000".into();
    grid.cells[0][1].symbol = "─".into();
    grid.cells[0][1].fg = "idx:15".into();
    grid.cells[1][2].symbol = "W".into();
    grid.cells[1][3].symbol = "界".into();
    let path = crate::utils::test_temp::unique_temp_path("picture").with_extension("png");
    let size = write_png(&grid, &path).unwrap();
    assert_eq!(size, (4 * CELL_W, 2 * CELL_H));
    assert_eq!(pixel(&path, 2, 2), [255, 0, 0], "background fills the cell");
    assert_eq!(
        pixel(&path, CELL_W + 1, CELL_H / 2),
        [255, 255, 255],
        "box line spans the cell"
    );
    let glyph_cell = (0..CELL_W)
        .flat_map(|x| (0..CELL_H).map(move |y| (2 * CELL_W + x, CELL_H + y)))
        .filter(|(x, y)| pixel(&path, *x, *y) != DEFAULT_BG)
        .count();
    assert!(glyph_cell > 10, "the glyph is drawn");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn colors_parse_the_names_the_screen_reports() {
    assert_eq!(parse_color("#5a96ff"), Some([0x5a, 0x96, 0xff]));
    assert_eq!(parse_color("idx:1"), Some([205, 49, 49]));
    assert_eq!(parse_color("idx:16"), Some([0, 0, 0]));
    assert_eq!(parse_color("idx:255"), Some([238, 238, 238]));
    assert_eq!(parse_color("lightblue"), Some([59, 142, 234]));
    assert_eq!(parse_color("default"), None);
}

#[test]
fn italic_cells_look_different_from_plain_ones() {
    use crate::devtools::tui::screen::Screen;

    let mut screen = Screen::new(5, 1).unwrap();
    screen.feed(b"plain");
    let normal = screen.grid();
    screen.feed(b"\x1b[H\x1b[3mplain\x1b[0m");
    let italic = screen.grid();
    assert!(italic.cells[0][0].attrs.contains(&"italic"));
    assert!(!normal.cells[0][0].attrs.contains(&"italic"));

    let normal_path =
        crate::utils::test_temp::unique_temp_path("normal-cell-style").with_extension("png");
    let italic_path =
        crate::utils::test_temp::unique_temp_path("italic-cell-style").with_extension("png");
    let normal_size = write_png(&normal, &normal_path).unwrap();
    let italic_size = write_png(&italic, &italic_path).unwrap();
    let decode = |path: &Path| {
        let decoder = png::Decoder::new(std::fs::File::open(path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut pixels).unwrap();
        pixels
    };
    let normal_pixels = decode(&normal_path);
    let italic_pixels = decode(&italic_path);
    std::fs::remove_file(normal_path).unwrap();
    std::fs::remove_file(italic_path).unwrap();
    assert_eq!(normal_size, italic_size);
    assert!(
        normal_pixels != italic_pixels,
        "italic terminal cells must remain visibly distinct in a cell screenshot"
    );
}

#[test]
fn a_descender_survives_the_next_rows_background() {
    let mut grid = Grid::blank(1, 2);
    grid.cells[0][0].symbol = "_".into();
    grid.cells[1][0].bg = "#000000".into();
    let path = crate::utils::test_temp::unique_temp_path("descender").with_extension("png");
    write_png(&grid, &path).unwrap();
    let lit = (0..CELL_W)
        .flat_map(|x| (0..CELL_H * 2).map(move |y| (x, y)))
        .filter(|(x, y)| pixel(&path, *x, *y)[0] > 100)
        .count();
    assert!(lit > 0, "the underscore is drawn");
    std::fs::remove_file(path).unwrap();
}
