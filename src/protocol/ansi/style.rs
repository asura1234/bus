use super::cursor::write_cursor_position;
use super::REVERSED_MODIFIER;
use crate::protocol::{underline_style_from_modifier, CellData, FrameData};
use std::io::Write;

pub(super) fn color_to_sgr_fg(val: u32) -> String {
    match val >> 24 {
        0x00 => match val & 0xFF {
            0x00 => "39".to_owned(), // Reset
            0x01 => "30".to_owned(), // Black
            0x02 => "31".to_owned(), // Red
            0x03 => "32".to_owned(), // Green
            0x04 => "33".to_owned(), // Yellow
            0x05 => "34".to_owned(), // Blue
            0x06 => "35".to_owned(), // Magenta
            0x07 => "36".to_owned(), // Cyan
            0x08 => "37".to_owned(), // Gray (light gray)
            0x09 => "90".to_owned(), // DarkGray
            0x0A => "91".to_owned(), // LightRed
            0x0B => "92".to_owned(), // LightGreen
            0x0C => "93".to_owned(), // LightYellow
            0x0D => "94".to_owned(), // LightBlue
            0x0E => "95".to_owned(), // LightMagenta
            0x0F => "96".to_owned(), // LightCyan
            0x10 => "97".to_owned(), // White
            _ => "39".to_owned(),    // Unknown → Reset
        },
        0x01 => format!("38;5;{}", val & 0xFF), // Indexed
        0x02 => {
            // RGB
            let r = (val >> 16) & 0xFF;
            let g = (val >> 8) & 0xFF;
            let b = val & 0xFF;
            format!("38;2;{r};{g};{b}")
        }
        _ => "39".to_owned(), // Unknown → Reset
    }
}

/// Converts a packed u32 color to a background SGR fragment.
pub(super) fn color_to_sgr_bg(val: u32) -> String {
    match val >> 24 {
        0x00 => match val & 0xFF {
            0x00 => "49".to_owned(),  // Reset
            0x01 => "40".to_owned(),  // Black
            0x02 => "41".to_owned(),  // Red
            0x03 => "42".to_owned(),  // Green
            0x04 => "43".to_owned(),  // Yellow
            0x05 => "44".to_owned(),  // Blue
            0x06 => "45".to_owned(),  // Magenta
            0x07 => "46".to_owned(),  // Cyan
            0x08 => "47".to_owned(),  // Gray (light gray)
            0x09 => "100".to_owned(), // DarkGray
            0x0A => "101".to_owned(), // LightRed
            0x0B => "102".to_owned(), // LightGreen
            0x0C => "103".to_owned(), // LightYellow
            0x0D => "104".to_owned(), // LightBlue
            0x0E => "105".to_owned(), // LightMagenta
            0x0F => "106".to_owned(), // LightCyan
            0x10 => "107".to_owned(), // White
            _ => "49".to_owned(),     // Unknown → Reset
        },
        0x01 => format!("48;5;{}", val & 0xFF), // Indexed
        0x02 => {
            let r = (val >> 16) & 0xFF;
            let g = (val >> 8) & 0xFF;
            let b = val & 0xFF;
            format!("48;2;{r};{g};{b}")
        }
        _ => "49".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Modifier → SGR
// ---------------------------------------------------------------------------

/// Converts a u16 modifier bitmask to SGR escape sequence fragments.
///
/// Returns a Vec of SGR parameter strings (e.g., "1" for bold, "3" for italic).
pub(super) fn modifier_to_sgr_parts(val: u16) -> Vec<&'static str> {
    let mut parts = Vec::new();

    // ratatui::Modifier bits (from bitflags)
    const BOLD: u16 = 1 << 0; // 0x01
    const DIM: u16 = 1 << 1; // 0x02
    const ITALIC: u16 = 1 << 2; // 0x04
    const UNDERLINED: u16 = 1 << 3; // 0x08
    const SLOW_BLINK: u16 = 1 << 4; // 0x10
    const RAPID_BLINK: u16 = 1 << 5; // 0x20
    const HIDDEN: u16 = 1 << 7; // 0x80
    const CROSSED_OUT: u16 = 1 << 8; // 0x100

    if val & BOLD != 0 {
        parts.push("1");
    }
    if val & DIM != 0 {
        parts.push("2");
    }
    if val & ITALIC != 0 {
        parts.push("3");
    }
    if val & UNDERLINED != 0 {
        parts.push(match underline_style_from_modifier(val) {
            2 => "4:2",
            3 => "4:3",
            4 => "4:4",
            5 => "4:5",
            _ => "4",
        });
    }
    if val & SLOW_BLINK != 0 {
        parts.push("5");
    }
    if val & RAPID_BLINK != 0 {
        parts.push("6");
    }
    if val & REVERSED_MODIFIER != 0 {
        parts.push("7");
    }
    if val & HIDDEN != 0 {
        parts.push("8");
    }
    if val & CROSSED_OUT != 0 {
        parts.push("9");
    }

    parts
}

/// Builds a complete SGR escape sequence for a cell's style.
pub(super) fn build_sgr(fg: u32, bg: u32, modifier: u16) -> String {
    let mut parts = vec!["0".to_owned()];
    parts.extend(
        modifier_to_sgr_parts(modifier)
            .into_iter()
            .map(str::to_owned),
    );
    parts.push(color_to_sgr_fg(fg));
    parts.push(color_to_sgr_bg(bg));
    format!("\x1b[{}m", parts.join(";"))
}

// ---------------------------------------------------------------------------
// Cell comparison
// ---------------------------------------------------------------------------

/// Checks if two cells are visually identical.
pub(super) fn cell_hyperlink_uri<'a>(frame: &'a FrameData, cell: &CellData) -> Option<&'a str> {
    let index = cell.hyperlink? as usize;
    frame.hyperlinks.get(index).map(String::as_str)
}

pub(super) fn sanitized_hyperlink_uri(uri: &str) -> Option<String> {
    let sanitized: String = uri
        .chars()
        .filter(|ch| *ch != '\x1b' && *ch != '\x07' && !ch.is_control())
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

pub(super) fn sanitized_frame_hyperlinks(frame: &FrameData) -> Vec<Option<String>> {
    frame
        .hyperlinks
        .iter()
        .map(|uri| sanitized_hyperlink_uri(uri))
        .collect()
}

pub(super) fn sanitized_cell_hyperlink_uri<'a>(
    sanitized_hyperlinks: &'a [Option<String>],
    cell: &CellData,
) -> Option<&'a str> {
    let index = cell.hyperlink? as usize;
    sanitized_hyperlinks.get(index)?.as_deref()
}

pub(super) fn write_hyperlink_if_changed(
    writer: &mut impl Write,
    active: &mut Option<String>,
    requested: Option<&str>,
) {
    let requested = requested.and_then(sanitized_hyperlink_uri);
    if active.as_deref() == requested.as_deref() {
        return;
    }

    if active.is_some() {
        let _ = writer.write_all(b"\x1b]8;;\x1b\\");
    }
    *active = requested;
    if let Some(uri) = active.as_deref() {
        let _ = write!(writer, "\x1b]8;;{uri}\x1b\\");
    }
}

pub(super) fn close_hyperlink(writer: &mut impl Write, active: &mut Option<String>) {
    if active.take().is_some() {
        let _ = writer.write_all(b"\x1b]8;;\x1b\\");
    }
}

pub(super) fn write_cell(
    writer: &mut impl Write,
    cursor_position: Option<(u16, u16)>,
    cell: &CellData,
    last_sgr: &mut String,
    active_hyperlink: &mut Option<String>,
    frame: &FrameData,
) {
    if cell.skip {
        return;
    }

    if let Some(position) = cursor_position {
        write_cursor_position(writer, position);
    }

    let sgr = build_sgr(cell.fg, cell.bg, cell.modifier);
    if sgr != *last_sgr {
        let _ = writer.write_all(sgr.as_bytes());
        *last_sgr = sgr;
    }

    write_hyperlink_if_changed(writer, active_hyperlink, cell_hyperlink_uri(frame, cell));
    let _ = writer.write_all(cell.symbol.as_bytes());
}

/// Writes only the cells that changed between the previous and current frame.
pub(super) fn cells_visually_equal(
    sanitized_hyperlinks: &[Option<String>],
    cell: &CellData,
    prev_sanitized_hyperlinks: &[Option<String>],
    prev_cell: &CellData,
) -> bool {
    cell.symbol == prev_cell.symbol
        && cell.fg == prev_cell.fg
        && cell.bg == prev_cell.bg
        && cell.modifier == prev_cell.modifier
        && sanitized_cell_hyperlink_uri(sanitized_hyperlinks, cell)
            == sanitized_cell_hyperlink_uri(prev_sanitized_hyperlinks, prev_cell)
    // Skip flag is only for ratatui internal use, not visual.
}
