//! Paint the prepared view and acknowledge emitted thumbnail graphics.
use super::super::BusUi;
use super::{cell_width, display, ACCENT, SELECTION};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
};

impl BusUi {
    /// Graphics commands updating the history thumbnails for the latest view.
    /// A full repaint of this frame erases iTerm2 images, so they are drawn
    /// again after it; moving or removing them asks for that repaint.
    pub fn thumbnail_graphics(&mut self) -> Vec<u8> {
        // The last thumbnail bytes never reached the terminal (the frame was
        // dropped), so it may lack images Bus believes it sent.
        if std::mem::take(&mut self.graphics_unconfirmed) {
            self.thumbnails.forget_terminal();
        }
        if self.full_repaint {
            self.thumbnails.invalidate();
        }
        let graphics = self.thumbnails.encode(&self.view.thumbnails);
        if self.thumbnails.take_repaint() {
            self.full_repaint = true;
        }
        self.graphics_unconfirmed = !graphics.is_empty();
        graphics
    }

    /// The client wrote the latest frame. `cleared` when it cleared the screen
    /// first, which removes images placed by earlier frames.
    pub fn graphics_presented(&mut self, cleared: bool) {
        self.graphics_unconfirmed = false;
        if cleared {
            self.thumbnails.forget_terminal();
        }
    }

    pub fn render(&self, buffer: &mut Buffer) {
        buffer.set_style(
            buffer.area,
            Style::default()
                .fg(Color::Rgb(222, 222, 226))
                .bg(Color::Rgb(24, 24, 28)),
        );
        // Fixed-count borders/dividers per client frame, never per agent/pane.
        for rect in [
            self.view.composer_box,
            self.view.notes_box,
            self.view.search_box,
            self.view.search_field,
        ] {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT))
                .render(rect.intersection(buffer.area), buffer);
        }
        for (rect, color) in &self.view.recipient_chips {
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(*color))
                .render(rect.intersection(buffer.area), buffer);
        }
        for rect in [
            self.view.composer_divider,
            self.view.history_divider,
            self.view.sidebar_divider,
            self.view.settings_divider,
        ] {
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(ACCENT))
                .render(rect.intersection(buffer.area), buffer);
        }
        let divider = self.view.composer_divider;
        if divider.width >= 2 && divider.height > 0 {
            for (x, symbol) in [(divider.x, "├"), (divider.right() - 1, "┤")] {
                if buffer.area.contains((x, divider.y).into()) {
                    buffer.set_stringn(x, divider.y, symbol, 1, Style::default().fg(ACCENT));
                }
            }
        }
        // Selection tints the rows it covers but stays beneath a dialog.
        fn paint_selection(selection: &[Rect], buffer: &mut Buffer) {
            for rect in selection {
                let rect = rect.intersection(buffer.area);
                for y in rect.top()..rect.bottom() {
                    for x in rect.left()..rect.right() {
                        buffer[(x, y)].set_bg(SELECTION);
                    }
                }
            }
        }
        let mut selection_painted = false;
        for (index, row) in self.view.rows.iter().enumerate() {
            if index == self.view.dialog_rows_start && self.view.dialog.width > 0 {
                paint_selection(&self.view.selection, buffer);
                selection_painted = true;
                let rect = self.view.dialog.intersection(buffer.area);
                Clear.render(rect, buffer);
                Block::default()
                    .borders(Borders::ALL)
                    .style(Style::default().bg(Color::Rgb(24, 24, 28)))
                    .border_style(Style::default().fg(ACCENT))
                    .render(rect, buffer);
            }
            if row.y >= buffer.area.bottom() || row.x >= buffer.area.right() {
                continue;
            }
            let style = Style::default()
                .fg(row.color.unwrap_or(if row.muted {
                    Color::Rgb(145, 148, 159)
                } else {
                    Color::Rgb(222, 222, 226)
                }))
                .bg(if row.selected {
                    Color::Rgb(46, 48, 58)
                } else {
                    Color::Rgb(24, 24, 28)
                })
                .patch(row.style.unwrap_or_default());
            buffer.set_stringn(
                row.x,
                row.y,
                display(&row.text),
                row.width.min(buffer.area.right() - row.x) as usize,
                style,
            );
            if let Some(colors) = &row.character_colors {
                let mut x = row.x;
                let right = row.x.saturating_add(row.width).min(buffer.area.right());
                for (index, character) in display(&row.text).chars().enumerate() {
                    let width = cell_width(character) as u16;
                    if width == 0 {
                        continue;
                    }
                    if x.saturating_add(width) > right {
                        break;
                    }
                    let color = colors.get(index).copied().unwrap_or(style.fg.unwrap());
                    buffer.set_stringn(
                        x,
                        row.y,
                        character.to_string(),
                        usize::from(width),
                        style.fg(color),
                    );
                    x = x.saturating_add(width);
                }
            }
            if row.color.is_none() && row.text.starts_with('#') {
                buffer.set_stringn(row.x, row.y, "#", 1, Style::default().fg(ACCENT));
            }
        }
        if !selection_painted {
            paint_selection(&self.view.selection, buffer);
        }
        let sidebar = self.view.sidebar.intersection(buffer.area);
        if sidebar.width > 0 {
            for y in sidebar.y..sidebar.bottom() {
                if !self.view.dialog.contains((sidebar.right() - 1, y).into()) {
                    buffer.set_stringn(sidebar.right() - 1, y, "│", 1, Style::default().fg(ACCENT));
                }
            }
        }
    }
}
