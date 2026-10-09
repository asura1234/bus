//! Shared room-shell and agent-form geometry.
use super::super::forms::Form;
use ratatui::layout::Rect;

pub(in crate::client) fn layout(
    cols: u16,
    rows: u16,
) -> crate::client::compositor::ClientShellLayout {
    let width = 28.min(cols / 3).max(1).min(cols);
    crate::client::compositor::ClientShellLayout {
        sidebar: Rect::new(0, 0, width, rows),
        pane_surface: Rect::new(width, 0, cols - width, rows),
    }
}

/// Rows from one agent-form field to the next: the MASTER form, which adds a
/// room and a prompt, drops the blank line in short terminals so its buttons
/// and validation error stay on screen.
pub(super) fn agent_form_gap(main: Rect, form: &Form) -> u16 {
    match form {
        Form::Agent {
            prompt: Some(_), ..
        } if main.height < 34 => 1,
        _ => 2,
    }
}
