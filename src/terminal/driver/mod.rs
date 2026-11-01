//! Provider-independent terminal observation and input. Neither caller needs
//! a dialog parser or an agent state to read and answer a terminal prompt.
mod grid;
pub(crate) mod input;
mod selection;

pub(crate) use grid::{Cell, Cursor, Grid};
pub(crate) use selection::{selection_focus, selection_step, SelectionStep};

use crate::terminal::TerminalRuntime;
use ratatui::{backend::TestBackend, layout::Rect};

/// Read a complete styled viewport, including shell prompts and pagers.
pub(crate) fn observe(runtime: &TerminalRuntime) -> Result<(Grid, u64), String> {
    for _ in 0..3 {
        let before = runtime.content_seq();
        if !before.is_multiple_of(2) || runtime.synchronized_output_active() {
            continue;
        }
        let (rows, cols) = runtime.current_size();
        let area = Rect::new(0, 0, cols, rows);
        let mut terminal = ratatui::Terminal::new(TestBackend::new(cols, rows))
            .map_err(|error| error.to_string())?;
        terminal
            .draw(|frame| runtime.render(frame, area, false))
            .map_err(|error| error.to_string())?;
        let mut grid = Grid::from_buffer(terminal.backend().buffer());
        for ((x, y), _, uri) in runtime.visible_hyperlinks(area) {
            if let Some(cell) = grid
                .cells
                .get_mut(y as usize)
                .and_then(|row| row.get_mut(x as usize))
            {
                cell.link = Some(uri);
            }
        }
        grid.cursor = runtime.cursor_state(area, true).map(|cursor| Cursor {
            row: cursor.y,
            col: cursor.x,
            visible: cursor.visible,
        });
        let after = runtime.content_seq();
        if before == after && !runtime.synchronized_output_active() {
            return Ok((grid, after));
        }
    }
    Err("Terminal is redrawing; observe it again".into())
}

#[cfg(test)]
#[path = "tests/driver_test.rs"]
mod tests;
