//! Frame blitting — renders FrameData to the terminal using diff-based updates.
//!
//! The blitting strategy:
//! 1. On the first frame, write the entire buffer (full redraw).
//! 2. On subsequent frames, diff against the last frame and only write
//!    the cells that changed.
//! 3. Wrap each frame in synchronized output so terminals that support it do
//!    not expose intermediate cursor positions while the frame is painted.
//! 4. Before writing any cells, hide the cursor to avoid stray cursor
//!    artifacts on terminals that render the hardware cursor at intermediate
//!    `CUP` positions during the frame stream.
//! 5. After writing all changed cells, restore the final cursor visibility
//!    and position from `frame.cursor`.
//! 6. On platforms that need it, repeat the final cursor anchor after ending
//!    synchronized output so external IMEs can place candidate windows at the
//!    real input position. Windows Terminal exposes that repeat as visible
//!    cursor movement during active TUI repaints, so Windows skips it.
//!
//! Escape sequences used:
//! - `CSI H` (CUP) — move cursor to (row, col)
//! - `CSI m` (SGR) — set graphic rendition (colors, bold, etc.)
//! - `CSI ? 2026 h/l` — begin/end synchronized output
//! - `CSI Ps SP q` — DECSCUSR cursor shape
//! - `ESC ] 52 ; c ; <base64> BEL` — OSC 52 clipboard write
//!
//! The goal is minimal output: skip unchanged cells, batch adjacent changes,
//! and minimize cursor movement.

mod cursor;
mod diff;
mod style;

use crate::protocol::wire::{CursorState, FrameData, PaneSurfacePatchRow};
use cursor::clamp_cursor_position;
pub(crate) use cursor::frame_with_drawn_cursor;
#[cfg(test)]
pub(crate) use diff::blit_frame_to;
#[cfg(test)]
use diff::{
    blit_frame_to_with_cursor_memory, blit_frame_to_with_cursor_memory_and_policy, cells_equal,
};
use diff::{
    blit_frame_to_with_cursor_memory_and_clear_policy, blit_patch_to, compute_prof_blit_stats,
    frame_cell_index, patch_cell_mut, patch_row_fits, patch_rows_overlap,
    repeat_ime_anchor_after_sync,
};
#[cfg(test)]
use style::{build_sgr, color_to_sgr_bg, color_to_sgr_fg, modifier_to_sgr_parts};

const REVERSED_MODIFIER: u16 = 1 << 6;
const SYNC_OUTPUT_END: &[u8] = b"\x1b[?2026l";

pub(crate) fn final_sync_output_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(SYNC_OUTPUT_END.len())
        .rposition(|window| window == SYNC_OUTPUT_END)
}

/// Bytes produced by a [`BlitEncoder`] for one terminal frame.
pub(crate) struct EncodedBlit {
    /// Terminal escape bytes ready to write to the host terminal.
    pub(crate) bytes: Vec<u8>,
    next_last_visible_cursor: Option<(u16, u16)>,
    next_last_cursor_shape: u8,
}

/// Stateful encoder that diffs semantic frames into terminal ANSI bytes.
#[derive(Default)]
pub(crate) struct BlitEncoder {
    last_frame: Option<FrameData>,
    last_visible_cursor: Option<(u16, u16)>,
    last_cursor_shape: u8,
}

impl BlitEncoder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn encode(&self, frame: &FrameData, repaint: bool) -> EncodedBlit {
        self.encode_inner(frame, repaint, false)
    }

    pub(crate) fn encode_with_suppressed_visible_cursor(
        &self,
        frame: &FrameData,
        repaint: bool,
    ) -> EncodedBlit {
        self.encode_inner(frame, repaint, true)
    }

    fn encode_inner(
        &self,
        frame: &FrameData,
        repaint: bool,
        suppress_visible_cursor: bool,
    ) -> EncodedBlit {
        let previous_frame = self.last_frame.as_ref();
        let prev = if repaint { None } else { previous_frame };
        let full = repaint
            || prev.is_none()
            || prev.is_some_and(|p| p.width != frame.width || p.height != frame.height);
        // Clear before the first frame and after a size change: a full redraw
        // only writes the new frame's cells, and some terminals (Terminal.app)
        // keep text pushed past the right edge by a narrower window and show it
        // again later. A same-size forced repaint overwrites every visible cell
        // without the flash of a clear.
        let clear_before_full_redraw =
            previous_frame.is_none_or(|p| p.width != frame.width || p.height != frame.height);
        let prof_stats = crate::utils::render::prof::enabled()
            .then(|| compute_prof_blit_stats(frame, prev, full));
        let prof_started = crate::utils::render::prof::timer();
        let mut bytes = Vec::new();
        let mut next_last_visible_cursor = self.last_visible_cursor;
        let mut next_last_cursor_shape = self.last_cursor_shape;
        blit_frame_to_with_cursor_memory_and_clear_policy(
            &mut bytes,
            frame,
            prev,
            &mut next_last_visible_cursor,
            &mut next_last_cursor_shape,
            repeat_ime_anchor_after_sync(),
            clear_before_full_redraw,
            suppress_visible_cursor,
        );
        if let Some(stats) = prof_stats {
            crate::utils::render::prof::duration_since("ansi_encode.total", prof_started);
            crate::utils::render::prof::counter("ansi_encode.bytes", bytes.len() as u64);
            crate::utils::render::prof::counter("ansi_encode.scanned_cells", stats.scanned_cells);
            crate::utils::render::prof::counter("ansi_encode.changed_cells", stats.changed_cells);
            crate::utils::render::prof::counter("ansi_encode.changed_runs", stats.changed_runs);
            if full {
                crate::utils::render::prof::event("ansi_encode.full");
            } else {
                crate::utils::render::prof::event("ansi_encode.partial");
            }
        }
        EncodedBlit {
            bytes,
            next_last_visible_cursor,
            next_last_cursor_shape,
        }
    }

    /// Whether encoding `frame` next clears the screen first: the first frame
    /// and any size change do.
    pub(crate) fn clears_before(&self, frame: &FrameData) -> bool {
        self.last_frame
            .as_ref()
            .is_none_or(|last| last.width != frame.width || last.height != frame.height)
    }

    /// Forgets the presented frame, so the next one clears the screen and
    /// redraws every cell. A host resize can leave cells the encoder never
    /// drew (Terminal.app keeps text beyond a narrowed edge), even when the
    /// window ends up the size of the last frame.
    pub(crate) fn invalidate(&mut self) {
        self.last_frame = None;
    }

    pub(crate) fn commit(&mut self, frame: FrameData, encoded: EncodedBlit) {
        self.last_visible_cursor = encoded.next_last_visible_cursor;
        self.last_cursor_shape = encoded.next_last_cursor_shape;
        self.last_frame = Some(frame);
    }

    pub(crate) fn encode_patch(
        &self,
        rows: &[PaneSurfacePatchRow],
        cursor: Option<CursorState>,
        suppress_visible_cursor: bool,
    ) -> Option<EncodedBlit> {
        let frame = self.last_frame.as_ref()?;
        if rows.iter().any(|row| !patch_row_fits(frame, row)) || patch_rows_overlap(rows) {
            return None;
        }
        let mut bytes = Vec::new();
        let mut next_last_visible_cursor = self.last_visible_cursor;
        let mut next_last_cursor_shape = self.last_cursor_shape;
        blit_patch_to(
            &mut bytes,
            frame,
            rows,
            cursor,
            &mut next_last_visible_cursor,
            &mut next_last_cursor_shape,
            repeat_ime_anchor_after_sync(),
            suppress_visible_cursor,
        );
        Some(EncodedBlit {
            bytes,
            next_last_visible_cursor,
            next_last_cursor_shape,
        })
    }

    pub(crate) fn patch_rows_with_drawn_cursor(
        &self,
        rows: &[PaneSurfacePatchRow],
        cursor: Option<&CursorState>,
    ) -> Option<Vec<PaneSurfacePatchRow>> {
        let frame = self.last_frame.as_ref()?;
        let mut rows = rows.to_vec();
        let previous = frame
            .cursor
            .as_ref()
            .filter(|cursor| cursor.visible)
            .map(|cursor| clamp_cursor_position(frame, cursor.x, cursor.y));
        let next = cursor
            .filter(|cursor| cursor.visible)
            .map(|cursor| clamp_cursor_position(frame, cursor.x, cursor.y));

        if let Some((x, y)) = previous.filter(|position| Some(*position) != next) {
            if patch_cell_mut(&mut rows, x, y).is_none() {
                let mut cell = frame.cells.get(frame_cell_index(frame, x, y)?)?.clone();
                cell.modifier ^= REVERSED_MODIFIER;
                rows.push(PaneSurfacePatchRow {
                    x,
                    y,
                    cells: vec![cell],
                });
            }
        }
        if let Some((x, y)) = next {
            if let Some(cell) = patch_cell_mut(&mut rows, x, y) {
                cell.modifier ^= REVERSED_MODIFIER;
            } else if previous != next {
                let mut cell = frame.cells.get(frame_cell_index(frame, x, y)?)?.clone();
                cell.modifier ^= REVERSED_MODIFIER;
                rows.push(PaneSurfacePatchRow {
                    x,
                    y,
                    cells: vec![cell],
                });
            }
        }
        Some(rows)
    }

    pub(crate) fn commit_patch(
        &mut self,
        rows: &[PaneSurfacePatchRow],
        cursor: Option<CursorState>,
        encoded: EncodedBlit,
    ) -> bool {
        let Some(frame) = self.last_frame.as_mut() else {
            return false;
        };
        for row in rows {
            let start = usize::from(row.y) * usize::from(frame.width) + usize::from(row.x);
            let end = start + row.cells.len();
            let Some(target) = frame.cells.get_mut(start..end) else {
                return false;
            };
            target.clone_from_slice(&row.cells);
        }
        frame.cursor = cursor;
        self.last_visible_cursor = encoded.next_last_visible_cursor;
        self.last_cursor_shape = encoded.next_last_cursor_shape;
        true
    }
}

#[cfg(test)]
#[path = "tests/diff_test.rs"]
mod tests;
