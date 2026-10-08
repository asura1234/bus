//! Build view rows, editor windows and cell selections.
use super::super::editor::Editor;
use super::text::cells;
use super::{display, wrap, wrap_ranges, wrapped_position, Action, Hit, Row, View};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
};
use std::ops::Range;

impl View {
    pub(super) fn overlay_row(
        &mut self,
        rect: Rect,
        text: String,
        action: Option<Action>,
        selected: bool,
    ) {
        // Overlay labels must erase the draft/reply suffix beneath them too.
        let text = display(&text);
        let padding = usize::from(rect.width)
            .saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
        self.row(
            rect,
            format!("{text}{}", " ".repeat(padding)),
            action,
            selected,
            false,
        );
    }

    pub(super) fn row(
        &mut self,
        rect: Rect,
        text: impl Into<String>,
        action: Option<Action>,
        selected: bool,
        muted: bool,
    ) {
        if rect.width == 0 || rect.height == 0 {
            return;
        }
        self.rows.push(Row {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            text: text.into(),
            selected,
            muted,
            color: None,
            character_colors: None,
            style: None,
        });
        if let Some(action) = action {
            self.hits.push(Hit { rect, action });
        }
    }

    pub(super) fn color_last_row(&mut self, rect: Rect, color: Color) {
        if rect.width > 0 && rect.height > 0 {
            if let Some(row) = self.rows.last_mut() {
                row.color = Some(color);
            }
        }
    }

    pub(super) fn color_last_row_characters(&mut self, rect: Rect, colors: Vec<Color>) {
        if rect.width > 0 && rect.height > 0 {
            if let Some(row) = self.rows.last_mut() {
                row.character_colors = Some(colors);
            }
        }
    }

    pub(super) fn style_last_row(&mut self, rect: Rect, style: Style) {
        if rect.width > 0 && rect.height > 0 {
            if let Some(row) = self.rows.last_mut() {
                row.style = Some(style);
            }
        }
    }

    pub(super) fn lines(&mut self, rect: Rect, text: &str, action: Option<Action>, muted: bool) {
        for (index, line) in wrap(text, rect.width)
            .into_iter()
            .take(usize::from(rect.height))
            .enumerate()
        {
            self.row(
                Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
                line,
                None,
                false,
                muted,
            );
        }
        if let Some(action) = action {
            self.hits.push(Hit { rect, action });
        }
    }

    pub(super) fn editor(
        &mut self,
        rect: Rect,
        editor: &Editor,
        action: Option<Action>,
        focused: bool,
    ) {
        self.editor_scrolled(rect, editor, action, focused, None, 0);
    }

    /// Draws an editor's wrapped rows. `scroll` pins the viewport; otherwise it
    /// moves from the `previous` frame's offset only as far as the caret needs.
    pub(super) fn editor_scrolled(
        &mut self,
        rect: Rect,
        editor: &Editor,
        action: Option<Action>,
        focused: bool,
        scroll: Option<usize>,
        previous: usize,
    ) -> (usize, usize) {
        if rect.width == 0 || rect.height == 0 {
            return (0, 0);
        }
        let lines = wrap_ranges(&editor.text, rect.width);
        let (cursor_row, cursor_column) = wrapped_position(&editor.text, &lines, editor.cursor);
        let count = lines.len();
        let height = usize::from(rect.height);
        let skip = scroll
            .unwrap_or(if cursor_row < previous {
                cursor_row
            } else {
                previous.max((cursor_row + 1).saturating_sub(height))
            })
            .min(count.saturating_sub(height));
        let selection = editor.selection();
        for (index, line) in lines.iter().enumerate().skip(skip).take(height) {
            let row = Rect::new(rect.x, rect.y + (index - skip) as u16, rect.width, 1);
            let text = &editor.text[line.clone()];
            // Typed text sits on the box's own background; focus shows as the
            // cursor, and only a selection is highlighted.
            self.row(row, display(text), None, false, false);
            if let Some(selection) = &selection {
                let newline = editor.text[line.end..].starts_with('\n');
                self.select(row, text, line.start, selection, newline);
            }
        }
        if let Some(action) = action {
            self.hits.push(Hit { rect, action });
        }
        if focused && (skip..skip + height).contains(&cursor_row) {
            self.cursor = Some(crate::protocol::CursorState {
                x: rect.x + cursor_column.min(usize::from(rect.width - 1)) as u16,
                y: rect.y + (cursor_row - skip) as u16,
                visible: true,
                shape: 2,
            });
        }
        (skip, count)
    }

    /// Highlights the cells of one displayed row inside `selection`, a byte
    /// range in the source whose row text begins at byte `start`. A selection
    /// running through a hard line end also marks the cell after the text.
    pub(super) fn select(
        &mut self,
        row: Rect,
        text: &str,
        start: usize,
        selection: &Range<usize>,
        newline: bool,
    ) {
        let end = start + text.len();
        let through = newline && selection.start <= end && selection.end > end;
        if selection.start > end || selection.end < start {
            return;
        }
        let width = usize::from(row.width);
        // History offsets can predate a re-wrap; count whole characters only.
        let cells_before = |offset: usize| {
            (0..=offset - start)
                .rev()
                .find_map(|index| text.get(..index))
                .map_or(0, cells)
        };
        let left = cells_before(selection.start.max(start)).min(width);
        let right = (cells_before(selection.end.min(end)) + usize::from(through)).min(width);
        if right > left {
            self.selection.push(Rect::new(
                row.x + left as u16,
                row.y,
                (right - left) as u16,
                1,
            ));
        }
    }
}
