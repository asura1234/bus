use ratatui::style::Color;

use super::render::{
    blank_cell_data, cell_data_from_style, ghostty_buffer_symbol_into, ghostty_cell_style,
    ghostty_color, ghostty_default_bg, ghostty_default_fg, PaletteOverrides,
};
use super::{
    ghostty_blank_symbol_for_width, GhosttyPaneCore, TerminalDirtyPatch, TerminalDirtyPatchOutcome,
};
use crate::protocol::CellData;

type PatchRows = Vec<(u16, Vec<CellData>)>;
type PatchCollection<T> = Result<T, &'static str>;

struct PatchCellAppearance {
    default_fg: Option<Color>,
    default_bg: Option<Color>,
    resolved_fg: Option<Color>,
    resolved_bg: Option<Color>,
    palette_overrides: Option<PaletteOverrides>,
    hide_kitty_placeholders: bool,
}

pub(super) fn ghostty_collect_dirty_patch(
    core: &mut GhosttyPaneCore,
    area_width: u16,
    area_height: u16,
) -> TerminalDirtyPatchOutcome {
    let prof_started = crate::render_prof::timer();
    let outcome = match collect_dirty_patch(core, area_width, area_height) {
        Ok(outcome) => outcome,
        Err(reason) => {
            crate::render_prof::event(reason);
            TerminalDirtyPatchOutcome::Fallback
        }
    };
    finish_dirty_collection(prof_started, outcome)
}

fn finish_dirty_collection(
    prof_started: Option<std::time::Instant>,
    outcome: TerminalDirtyPatchOutcome,
) -> TerminalDirtyPatchOutcome {
    if let Some(started) = prof_started {
        crate::render_prof::duration("dirty_collect.total", started.elapsed());
        match &outcome {
            TerminalDirtyPatchOutcome::Clean => {
                crate::render_prof::event("dirty_collect.clean");
            }
            TerminalDirtyPatchOutcome::Fallback => {
                crate::render_prof::event("dirty_collect.fallback");
            }
            TerminalDirtyPatchOutcome::Patch(patch) => {
                crate::render_prof::event("dirty_collect.patch");
                crate::render_prof::counter("dirty_collect.rows", patch.rows.len() as u64);
                let cells = patch.rows.iter().map(|(_, cells)| cells.len() as u64).sum();
                crate::render_prof::counter("dirty_collect.cells", cells);
            }
        }
    }
    outcome
}

fn collect_dirty_patch(
    core: &mut GhosttyPaneCore,
    area_width: u16,
    area_height: u16,
) -> PatchCollection<TerminalDirtyPatchOutcome> {
    let host_theme = core.host_terminal_theme;
    let initial_default_foreground = core.initial_default_foreground;
    let initial_default_background = core.initial_default_background;
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = core;
    if render_state.update(terminal).is_err() {
        return Err("dirty_fallback.render_state_update_error");
    }
    let collect_all_rows = match render_state.dirty() {
        Ok(crate::ghostty::Dirty::Clean) => return Ok(TerminalDirtyPatchOutcome::Clean),
        Ok(crate::ghostty::Dirty::Partial) => false,
        // A full dirty state means that every visible row may have changed. It
        // is still safe to send this as a bounded patch: the client replaces
        // only this pane's viewport, rather than falling back to the whole
        // shell surface.
        Ok(crate::ghostty::Dirty::Full) => true,
        Err(_) => return Err("dirty_fallback.dirty_read_error"),
    };

    let colors = render_state.colors().ok();
    let default_bg = colors
        .and_then(|c| ghostty_default_bg(c.background, host_theme, initial_default_background));
    let default_fg = colors
        .and_then(|c| ghostty_default_fg(c.foreground, host_theme, initial_default_foreground));
    let resolved_fg = colors.map(|c| ghostty_color(c.foreground));
    let resolved_bg = colors.map(|c| ghostty_color(c.background));
    let palette_overrides = colors
        .zip(terminal.default_palette().ok())
        .and_then(|(colors, default)| PaletteOverrides::new(&colors.palette, &default));
    let hide_kitty_placeholders = crate::kitty_graphics::is_enabled();

    let appearance = PatchCellAppearance {
        default_fg,
        default_bg,
        resolved_fg,
        resolved_bg,
        palette_overrides,
        hide_kitty_placeholders,
    };
    let Ok(mut row_iterator) = crate::ghostty::RowIterator::new() else {
        return Err("dirty_fallback.row_iterator_new_error");
    };
    let Ok(mut row_cells) = crate::ghostty::RowCells::new() else {
        return Err("dirty_fallback.row_cells_new_error");
    };
    let patch_rows = collect_patch_rows(
        render_state,
        &mut row_iterator,
        &mut row_cells,
        area_width,
        area_height,
        collect_all_rows,
        &appearance,
    )?;
    clear_collected_dirty(render_state, area_height, &patch_rows)?;
    Ok(TerminalDirtyPatchOutcome::Patch(TerminalDirtyPatch {
        rows: patch_rows,
    }))
}

fn collect_patch_rows(
    render_state: &mut crate::ghostty::RenderState,
    row_iterator: &mut crate::ghostty::RowIterator,
    row_cells: &mut crate::ghostty::RowCells,
    area_width: u16,
    area_height: u16,
    collect_all_rows: bool,
    appearance: &PatchCellAppearance,
) -> PatchCollection<PatchRows> {
    let Ok(mut rows) = render_state.populate_row_iterator(row_iterator) else {
        return Err("dirty_fallback.populate_rows_error");
    };
    let mut grapheme_bytes = Vec::new();
    let mut symbol_scratch = String::new();
    let mut patch_rows = Vec::new();
    let mut y = 0u16;
    while y < area_height && rows.next() {
        let Ok(dirty) = rows.dirty() else {
            return Err("dirty_fallback.row_dirty_read_error");
        };
        if collect_all_rows || dirty {
            match rows.selection() {
                Ok(None) => {}
                Ok(Some(_)) => return Err("dirty_fallback.row_selection_present"),
                Err(_) => return Err("dirty_fallback.row_selection_error"),
            }
            let Ok(mut cells) = rows.populate_cells(row_cells) else {
                return Err("dirty_fallback.populate_cells_error");
            };
            let patch_cells = collect_patch_row(
                &mut cells,
                area_width,
                appearance,
                &mut grapheme_bytes,
                &mut symbol_scratch,
            )?;
            patch_rows.push((y, patch_cells));
        }
        y += 1;
    }

    Ok(patch_rows)
}

fn collect_patch_row(
    cells: &mut crate::ghostty::RowCellIter<'_>,
    area_width: u16,
    appearance: &PatchCellAppearance,
    grapheme_bytes: &mut Vec<u8>,
    symbol_scratch: &mut String,
) -> PatchCollection<Vec<CellData>> {
    let mut patch_cells = Vec::with_capacity(usize::from(area_width));
    let mut x = 0u16;
    while x < area_width && cells.next() {
        let Ok(basic) = cells.basic_data() else {
            return Err("dirty_fallback.basic_data_error");
        };
        if basic.has_hyperlink {
            return Err("dirty_fallback.hyperlink_present");
        }
        let style = ghostty_cell_style(
            cells,
            &basic,
            appearance.default_fg,
            appearance.default_bg,
            appearance.resolved_fg,
            appearance.resolved_bg,
            appearance.palette_overrides.as_ref(),
        );
        let symbol = match ghostty_buffer_symbol_into(
            cells,
            basic.wide,
            appearance.hide_kitty_placeholders,
            grapheme_bytes,
            symbol_scratch,
        ) {
            Ok(symbol) => symbol.to_owned(),
            Err(_) => ghostty_blank_symbol_for_width(basic.wide).to_owned(),
        };
        patch_cells.push(cell_data_from_style(symbol, style));
        x += 1;
    }
    while x < area_width {
        patch_cells.push(blank_cell_data(
            appearance.default_fg,
            appearance.default_bg,
        ));
        x += 1;
    }
    Ok(patch_cells)
}

fn clear_collected_dirty(
    render_state: &mut crate::ghostty::RenderState,
    area_height: u16,
    patch_rows: &PatchRows,
) -> PatchCollection<()> {
    // Nothing above mutates dirty state. Only clear it after every row has
    // been collected successfully, so a safety fallback leaves the next
    // collection with the same information.
    let dirty_ys: std::collections::HashSet<u16> = patch_rows.iter().map(|(row, _)| *row).collect();
    if !dirty_ys.is_empty() {
        let Ok(mut clear_row_iterator) = crate::ghostty::RowIterator::new() else {
            return Err("dirty_fallback.clear_row_iterator_new_error");
        };
        let Ok(mut clear_rows) = render_state.populate_row_iterator(&mut clear_row_iterator) else {
            return Err("dirty_fallback.clear_populate_rows_error");
        };
        let mut clear_y = 0u16;
        while clear_y < area_height && clear_rows.next() {
            if dirty_ys.contains(&clear_y) && clear_rows.clear_dirty().is_err() {
                return Err("dirty_fallback.clear_dirty_error");
            }
            clear_y += 1;
        }
    }
    if render_state
        .set_dirty(crate::ghostty::Dirty::Clean)
        .is_err()
    {
        return Err("dirty_fallback.set_clean_error");
    }

    Ok(())
}
