//! Shared room-shell and agent-form geometry.
use super::super::forms::Form;
use ratatui::layout::Rect;

/// `sidebar_width` is the saved setting; a narrow terminal still keeps the
/// sidebar to a third so the chat and agent panes stay usable.
pub(in crate::client) fn layout(
    cols: u16,
    rows: u16,
    sidebar_width: u16,
) -> crate::client::compositor::ClientShellLayout {
    let width = sidebar_width.min(cols / 3).max(1).min(cols);
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
