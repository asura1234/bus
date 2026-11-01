//! Provider-independent terminal observation and input. Neither caller needs
//! a dialog parser or an agent state to read and answer a terminal prompt.
mod grid;
pub(crate) mod input;
mod press;
mod selection;

pub(crate) use grid::Grid;
pub(super) use press::DriverState;
pub(crate) use press::{press, validate_press};
pub(crate) use selection::{
    selection_focus, selection_identity, selection_step, SelectionIdentity, SelectionStep,
};

use crate::terminal::TerminalRuntime;

/// Read a styled viewport without acknowledging presentation dirty state.
pub(crate) fn observe(runtime: &TerminalRuntime) -> Result<(Grid, u64), String> {
    for _ in 0..3 {
        let before = runtime.content_seq();
        if !before.is_multiple_of(2) || runtime.synchronized_output_active() {
            continue;
        }
        let grid = runtime.with_terminal(Grid::from_terminal)?;
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
