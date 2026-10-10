use super::diff::cell_width;
use super::REVERSED_MODIFIER;
use crate::protocol::wire::{CellData, FrameData};
use std::io::Write;

pub(crate) fn frame_with_drawn_cursor(mut frame: FrameData) -> FrameData {
    if let Some(cursor) = frame.cursor.as_ref().filter(|cursor| cursor.visible) {
        let (x, y) = clamp_cursor_position(&frame, cursor.x, cursor.y);
        let row_start = usize::from(y) * usize::from(frame.width);
        let x = drawn_cursor_column(x, |col| frame.cells.get(row_start + usize::from(col)));
        let idx = (y as usize)
            .saturating_mul(frame.width as usize)
            .saturating_add(x as usize);
        if let Some(cell) = frame.cells.get_mut(idx) {
            cell.modifier ^= REVERSED_MODIFIER;
        }
    }
    frame
}

/// Moves a cursor on a wide glyph's continuation cell back to the glyph.
/// Only the glyph's own cell is written to the host, so reversing the
/// continuation alone would leave the drawn cursor invisible.
pub(super) fn drawn_cursor_column<'a>(
    x: u16,
    cell_at: impl Fn(u16) -> Option<&'a CellData>,
) -> u16 {
    let mut col = 0u16;
    while col < x {
        let Some(cell) = cell_at(col) else {
            return x;
        };
        // Mirrors the blit walk: skipped cells are never written, so they
        // cover nothing beyond themselves.
        let width = if cell.skip {
            1
        } else {
            cell_width(cell).max(1)
        };
        if usize::from(x - col) < width {
            return col;
        }
        col = col.saturating_add(u16::try_from(width).unwrap_or(u16::MAX));
    }
    x
}

#[derive(Clone, Copy)]
pub(super) struct HostCursorState {
    pub(super) position: (u16, u16),
    pub(super) visible: bool,
    /// DECSCUSR parameter (0–6). 0 means terminal default.
    pub(super) shape: u8,
}

pub(super) fn resolve_host_cursor_state(
    frame: &FrameData,
    last_visible_cursor: &mut Option<(u16, u16)>,
) -> HostCursorState {
    if let Some(cursor) = &frame.cursor {
        if cursor.visible {
            let position = clamp_cursor_position(frame, cursor.x, cursor.y);
            *last_visible_cursor = Some(position);
            return HostCursorState {
                position,
                visible: true,
                shape: normalize_cursor_shape(cursor.shape),
            };
        }

        let position = clamp_cursor_position(frame, cursor.x, cursor.y);
        return HostCursorState {
            position,
            visible: false,
            shape: normalize_cursor_shape(cursor.shape),
        };
    }

    let position = (*last_visible_cursor)
        .map(|(x, y)| clamp_cursor_position(frame, x, y))
        .unwrap_or_else(|| default_hidden_cursor_position(frame));
    HostCursorState {
        position,
        visible: false,
        shape: 0,
    }
}

pub(super) fn normalize_cursor_shape(shape: u8) -> u8 {
    if shape <= 6 {
        shape
    } else {
        0
    }
}

pub(super) fn default_hidden_cursor_position(frame: &FrameData) -> (u16, u16) {
    (
        frame.width.saturating_sub(1),
        frame.height.saturating_sub(1),
    )
}

pub(super) fn clamp_cursor_position(frame: &FrameData, x: u16, y: u16) -> (u16, u16) {
    (
        x.min(frame.width.saturating_sub(1)),
        y.min(frame.height.saturating_sub(1)),
    )
}

pub(super) fn write_cursor_position(writer: &mut impl Write, (x, y): (u16, u16)) {
    // CUP: move cursor to (row+1, col+1) — 1-based.
    let _ = write!(writer, "\x1b[{};{}H", y + 1, x + 1);
}

pub(super) fn write_host_cursor_state(
    writer: &mut impl Write,
    cursor: HostCursorState,
    last_shape: &mut u8,
) {
    write_cursor_position(writer, cursor.position);
    if cursor.shape != *last_shape {
        let _ = write!(writer, "\x1b[{} q", cursor.shape);
        *last_shape = cursor.shape;
    }
    if cursor.visible {
        // Show cursor only after it is already at the final position.
        let _ = writer.write_all(b"\x1b[?25h");
    } else {
        let _ = writer.write_all(b"\x1b[?25l");
    }
}

pub(super) fn write_ime_anchor_cursor_state(writer: &mut impl Write, cursor: HostCursorState) {
    write_cursor_position(writer, cursor.position);
    if cursor.visible {
        let _ = writer.write_all(b"\x1b[?25h");
    } else {
        let _ = writer.write_all(b"\x1b[?25l");
    }
}
