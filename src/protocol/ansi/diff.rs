use super::cursor::{
    resolve_host_cursor_state, write_host_cursor_state, write_ime_anchor_cursor_state,
};
use super::style::{cells_visually_equal, close_hyperlink, sanitized_frame_hyperlinks, write_cell};
use crate::protocol::{CellData, CursorState, FrameData, PaneSurfacePatchRow};
use std::cmp;
use std::io::Write;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Default)]
pub(super) struct ProfBlitStats {
    pub(super) scanned_cells: u64,
    pub(super) changed_cells: u64,
    pub(super) changed_runs: u64,
}

pub(super) fn compute_prof_blit_stats(
    frame: &FrameData,
    prev: Option<&FrameData>,
    full: bool,
) -> ProfBlitStats {
    let Some(prev) = prev.filter(|_| !full) else {
        let changed_cells = frame.cells.iter().filter(|cell| !cell.skip).count() as u64;
        return ProfBlitStats {
            scanned_cells: frame.cells.len() as u64,
            changed_cells,
            changed_runs: changed_cells,
        };
    };

    let sanitized_hyperlinks = sanitized_frame_hyperlinks(frame);
    let prev_sanitized_hyperlinks = sanitized_frame_hyperlinks(prev);
    let mut stats = ProfBlitStats {
        scanned_cells: frame.cells.len() as u64,
        changed_cells: 0,
        changed_runs: 0,
    };
    for row in 0..frame.height {
        let mut in_run = false;
        let mut invalidated = 0usize;
        let mut to_skip = 0usize;
        for col in 0..frame.width {
            let idx = (row as usize) * (frame.width as usize) + (col as usize);
            let cell = &frame.cells[idx];
            let prev_cell = &prev.cells[idx];
            let changed = !cell.skip
                && (!cells_visually_equal(
                    &sanitized_hyperlinks,
                    cell,
                    &prev_sanitized_hyperlinks,
                    prev_cell,
                ) || invalidated > 0)
                && to_skip == 0;
            if changed {
                stats.changed_cells += 1;
                if !in_run {
                    stats.changed_runs += 1;
                    in_run = true;
                }
            } else {
                in_run = false;
            }
            to_skip = cell_width(cell).saturating_sub(1);
            let affected_width = cmp::max(cell_width(cell), cell_width(prev_cell));
            invalidated = cmp::max(affected_width, invalidated).saturating_sub(1);
        }
    }
    stats
}

// ---------------------------------------------------------------------------
// Color → escape sequence
// ---------------------------------------------------------------------------

/// Converts a packed u32 color to an SGR escape sequence fragment.
///
/// Returns a string like `38;5;123` (indexed) or `38;2;255;128;64` (RGB)
/// or `39` (reset), without the leading `\x1b[` or trailing `m`.
pub(super) fn cells_equal(a: &CellData, b: &CellData) -> bool {
    a.symbol == b.symbol
        && a.fg == b.fg
        && a.bg == b.bg
        && a.modifier == b.modifier
        && a.hyperlink == b.hyperlink
    // Skip flag is only for ratatui internal use, not visual.
}

// ---------------------------------------------------------------------------
// Blitting
// ---------------------------------------------------------------------------

/// Blits a frame to a writer, diffing against the previous frame.
#[cfg(test)]
pub(crate) fn blit_frame_to(writer: impl Write, frame: &FrameData, prev: Option<&FrameData>) {
    let mut last_visible_cursor = None;
    let mut last_cursor_shape = 0;
    blit_frame_to_with_cursor_memory(
        writer,
        frame,
        prev,
        &mut last_visible_cursor,
        &mut last_cursor_shape,
        false,
    );
}

#[cfg(test)]
pub(super) fn blit_frame_to_with_cursor_memory(
    writer: impl Write,
    frame: &FrameData,
    prev: Option<&FrameData>,
    last_visible_cursor: &mut Option<(u16, u16)>,
    last_cursor_shape: &mut u8,
    suppress_visible_cursor: bool,
) {
    blit_frame_to_with_cursor_memory_and_policy(
        writer,
        frame,
        prev,
        last_visible_cursor,
        last_cursor_shape,
        repeat_ime_anchor_after_sync(),
        suppress_visible_cursor,
    );
}

#[cfg(test)]
pub(super) fn blit_frame_to_with_cursor_memory_and_policy(
    writer: impl Write,
    frame: &FrameData,
    prev: Option<&FrameData>,
    last_visible_cursor: &mut Option<(u16, u16)>,
    last_cursor_shape: &mut u8,
    repeat_ime_anchor: bool,
    suppress_visible_cursor: bool,
) {
    blit_frame_to_with_cursor_memory_and_clear_policy(
        writer,
        frame,
        prev,
        last_visible_cursor,
        last_cursor_shape,
        repeat_ime_anchor,
        true,
        suppress_visible_cursor,
    );
}

pub(super) fn frame_cell_index(frame: &FrameData, x: u16, y: u16) -> Option<usize> {
    (x < frame.width && y < frame.height)
        .then(|| usize::from(y) * usize::from(frame.width) + usize::from(x))
}

pub(super) fn patch_cell_mut(
    rows: &mut [PaneSurfacePatchRow],
    x: u16,
    y: u16,
) -> Option<&mut CellData> {
    rows.iter_mut().rev().find_map(|row| {
        if row.y != y || x < row.x {
            return None;
        }
        row.cells.get_mut(usize::from(x - row.x))
    })
}

pub(super) fn patch_rows_overlap(rows: &[PaneSurfacePatchRow]) -> bool {
    rows.iter().enumerate().any(|(index, left)| {
        let left_end = left.x.saturating_add(left.cells.len() as u16);
        rows[index + 1..].iter().any(|right| {
            if left.y != right.y {
                return false;
            }
            let right_end = right.x.saturating_add(right.cells.len() as u16);
            left.x < right_end && right.x < left_end
        })
    })
}

pub(super) fn patch_row_fits(frame: &FrameData, row: &PaneSurfacePatchRow) -> bool {
    let Ok(len) = u16::try_from(row.cells.len()) else {
        return false;
    };
    if row.y >= frame.height
        || row.x.saturating_add(len) > frame.width
        || row.cells.iter().any(|cell| cell.hyperlink.is_some())
    {
        return false;
    }
    let start = usize::from(row.y) * usize::from(frame.width) + usize::from(row.x);
    let end = start + row.cells.len();
    frame
        .cells
        .get(start..end)
        .is_some_and(|cells| cells.iter().all(|cell| cell.hyperlink.is_none()))
}

pub(super) fn blit_patch_to(
    mut writer: impl Write,
    frame: &FrameData,
    rows: &[PaneSurfacePatchRow],
    cursor: Option<CursorState>,
    last_visible_cursor: &mut Option<(u16, u16)>,
    last_cursor_shape: &mut u8,
    repeat_ime_anchor: bool,
    suppress_visible_cursor: bool,
) {
    let _ = writer.write_all(b"\x1b[?2026h\x1b[?25l\x1b]8;;\x1b\\");
    let mut last_sgr = String::new();
    let mut active_hyperlink = None;
    for row in rows {
        let mut invalidated = 0usize;
        let mut to_skip = 0usize;
        let mut next_inline_col = None;
        for (offset, cell) in row.cells.iter().enumerate() {
            let col = row.x + offset as u16;
            let idx = usize::from(row.y) * usize::from(frame.width) + usize::from(col);
            let prev_cell = &frame.cells[idx];
            if !cell.skip && (!cells_equal(cell, prev_cell) || invalidated > 0) && to_skip == 0 {
                let cursor_position =
                    (next_inline_col != Some(col) || invalidated > 0).then_some((col, row.y));
                write_cell(
                    &mut writer,
                    cursor_position,
                    cell,
                    &mut last_sgr,
                    &mut active_hyperlink,
                    frame,
                );
                next_inline_col = (cell.symbol.is_ascii() && cell_width(cell) == 1)
                    .then_some(col.saturating_add(1));
            }
            to_skip = cell_width(cell).saturating_sub(1);
            let affected_width = cmp::max(cell_width(cell), cell_width(prev_cell));
            invalidated = cmp::max(affected_width, invalidated).saturating_sub(1);
        }
    }
    close_hyperlink(&mut writer, &mut active_hyperlink);
    if !last_sgr.is_empty() {
        let _ = writer.write_all(b"\x1b[0m");
    }

    let cursor_frame = FrameData {
        cells: Vec::new(),
        width: frame.width,
        height: frame.height,
        cursor,
        hyperlinks: Vec::new(),
        graphics: Vec::new(),
    };
    let mut host_cursor = resolve_host_cursor_state(&cursor_frame, last_visible_cursor);
    if suppress_visible_cursor && host_cursor.visible {
        host_cursor.visible = false;
    }
    write_host_cursor_state(&mut writer, host_cursor, last_cursor_shape);
    let _ = writer.write_all(b"\x1b[?2026l");
    if repeat_ime_anchor {
        write_ime_anchor_cursor_state(&mut writer, host_cursor);
    }
    let _ = writer.flush();
}

pub(super) fn blit_frame_to_with_cursor_memory_and_clear_policy(
    mut writer: impl Write,
    frame: &FrameData,
    prev: Option<&FrameData>,
    last_visible_cursor: &mut Option<(u16, u16)>,
    last_cursor_shape: &mut u8,
    repeat_ime_anchor: bool,
    clear_before_full_redraw: bool,
    suppress_visible_cursor: bool,
) {
    // Ask terminals that support synchronized output to apply the whole frame
    // atomically. This keeps IMEs and cursor trackers from observing the
    // intermediate CUP positions used while painting changed cells.
    let _ = writer.write_all(b"\x1b[?2026h");

    // Hide cursor before any cell writes to avoid stray cursor artifacts
    // on terminals that render the hardware cursor at intermediate CUP positions.
    let _ = writer.write_all(b"\x1b[?25l");

    // Start each frame from a known OSC 8 state. If a previous write was
    // interrupted or the outer terminal had an active hyperlink, unlinked cells
    // must not inherit it.
    let _ = writer.write_all(b"\x1b]8;;\x1b\\");

    // Diffing requires a previous frame of the same size. Binding it here
    // expresses that invariant without unwrapping an independently tested Option.
    if let Some(prev) = prev.filter(|p| p.width == frame.width && p.height == frame.height) {
        write_changed_cells(&mut writer, frame, prev);
    } else {
        // On first frame or size change, do a full redraw.
        if clear_before_full_redraw {
            let _ = writer.write_all(b"\x1b[2J");
        }
        write_all_cells(&mut writer, frame);
    }

    // Position the cursor while it is still hidden, then restore visibility.
    // Showing before moving makes slow terminals and IMEs briefly observe the
    // cursor at the last painted cell, which can be an animated sidebar/status
    // cell rather than the focused pane's input position. When the focused pane
    // hides its cursor, still park the host cursor intentionally so IMEs do not
    // anchor to whichever cell happened to be painted last.
    let mut host_cursor = resolve_host_cursor_state(frame, last_visible_cursor);
    if suppress_visible_cursor && host_cursor.visible {
        host_cursor.visible = false;
    }
    write_host_cursor_state(&mut writer, host_cursor, last_cursor_shape);

    // End the synchronized output block immediately after the final cursor
    // state is emitted so supporting terminals can present the frame atomically.
    let _ = writer.write_all(b"\x1b[?2026l");

    // Some native IMEs track candidate-window placement from normal terminal
    // cursor updates and may not observe cursor moves emitted inside synchronized
    // output. Re-emit only the resolved final cursor anchor after the sync block
    // on targets that need it; Windows Terminal exposes that repeat as cursor
    // movement during active TUI repaints.
    if repeat_ime_anchor {
        write_ime_anchor_cursor_state(&mut writer, host_cursor);
    }
    let _ = writer.flush();
}

#[cfg(windows)]
pub(super) fn repeat_ime_anchor_after_sync() -> bool {
    false
}

#[cfg(not(windows))]
pub(super) fn repeat_ime_anchor_after_sync() -> bool {
    true
}

/// Writes all cells in the frame (full redraw).
pub(super) fn cell_width(cell: &CellData) -> usize {
    if is_halfwidth_katakana_voiced_grapheme(&cell.symbol) {
        return 2;
    }
    cell.symbol.width()
}

pub(super) fn is_halfwidth_katakana_voiced_grapheme(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(base) = chars.next() else {
        return false;
    };
    let Some(mark) = chars.next() else {
        return false;
    };
    chars.next().is_none()
        && ('\u{ff66}'..='\u{ff9d}').contains(&base)
        && matches!(mark, '\u{ff9e}' | '\u{ff9f}')
}

pub(super) fn write_all_cells(writer: &mut impl Write, frame: &FrameData) {
    let mut last_sgr = String::new();
    let mut active_hyperlink = None;
    for row in 0..frame.height {
        let mut to_skip = 0usize;
        let mut next_inline_col = None;
        for col in 0..frame.width {
            if to_skip > 0 {
                to_skip -= 1;
                continue;
            }

            let idx = (row as usize) * (frame.width as usize) + (col as usize);
            let cell = &frame.cells[idx];

            if cell.skip {
                next_inline_col = None;
                continue;
            }

            let cursor_position = (next_inline_col != Some(col)).then_some((col, row));
            write_cell(
                writer,
                cursor_position,
                cell,
                &mut last_sgr,
                &mut active_hyperlink,
                frame,
            );
            let width = cell_width(cell);
            next_inline_col =
                (cell.symbol.is_ascii() && width == 1).then_some(col.saturating_add(1));
            to_skip = width.saturating_sub(1);
        }
    }

    close_hyperlink(writer, &mut active_hyperlink);

    // Reset style at the end.
    let _ = writer.write_all(b"\x1b[0m");
}

pub(super) fn write_changed_cells(writer: &mut impl Write, frame: &FrameData, prev: &FrameData) {
    let mut last_sgr = String::new(); // Track last SGR to avoid redundant style changes.
    let mut active_hyperlink = None;
    let sanitized_hyperlinks = sanitized_frame_hyperlinks(frame);
    let prev_sanitized_hyperlinks = sanitized_frame_hyperlinks(prev);

    for row in 0..frame.height {
        let mut invalidated = 0usize;
        let mut to_skip = 0usize;
        // Herdr clients disable host autowrap, so safe cells can advance inline
        // without spilling into adjacent rows during a resize race.
        let mut next_inline_col = None;

        for col in 0..frame.width {
            let idx = (row as usize) * (frame.width as usize) + (col as usize);
            let cell = &frame.cells[idx];
            let prev_cell = &prev.cells[idx];

            if !cell.skip
                && (!cells_visually_equal(
                    &sanitized_hyperlinks,
                    cell,
                    &prev_sanitized_hyperlinks,
                    prev_cell,
                ) || invalidated > 0)
                && to_skip == 0
            {
                let cursor_position =
                    (next_inline_col != Some(col) || invalidated > 0).then_some((col, row));
                write_cell(
                    writer,
                    cursor_position,
                    cell,
                    &mut last_sgr,
                    &mut active_hyperlink,
                    frame,
                );
                next_inline_col = (cell.symbol.is_ascii() && cell_width(cell) == 1)
                    .then_some(col.saturating_add(1));
            }

            to_skip = cell_width(cell).saturating_sub(1);
            let affected_width = cmp::max(cell_width(cell), cell_width(prev_cell));
            invalidated = cmp::max(affected_width, invalidated).saturating_sub(1);
        }
    }

    close_hyperlink(writer, &mut active_hyperlink);

    // Reset style if we wrote anything.
    if !last_sgr.is_empty() {
        let _ = writer.write_all(b"\x1b[0m");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
