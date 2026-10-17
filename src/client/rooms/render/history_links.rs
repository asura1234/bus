//! Painting and hit areas of the links in visible history rows.
use super::super::history::Line;
use super::super::links::{Candidate, Target, LINK};
use super::{Action, View};
use ratatui::layout::Rect;
use std::path::Path;
use unicode_width::UnicodeWidthStr;

impl View {
    /// Quote keeps its row's paint, but only visible label cells are a hit.
    pub(super) fn quote_link(&mut self, row: Rect, line: &Line) {
        let Some(action @ Action::Quote(_)) = &line.action else {
            return;
        };
        let label = line.text.trim_start();
        let indent = (line.text.width() - label.width()) as u16;
        let cells = (label.width() as u16).min(row.width.saturating_sub(indent));
        if cells > 0 {
            self.hit(Rect::new(row.x + indent, row.y, cells, 1), action.clone());
        }
    }

    /// An attachment is a link over its name or picture only, so a press
    /// beside it, or a drag across it, selects like any row.
    pub(super) fn attachment_link(&mut self, row: Rect, line: &Line, path: &Path) {
        let cells = line
            .thumbnail
            .as_ref()
            .map_or(line.text.width() as u16, |thumbnail| thumbnail.cols);
        let rect = Rect::new(row.x, row.y, cells.min(row.width), 1);
        if rect.width > 0 {
            self.hit(rect, Action::OpenFile(path.to_path_buf()));
            self.history_links
                .push((rect, Target::Path(path.to_path_buf())));
        }
    }

    /// Paints the URLs and paths of one row in the link style.
    pub(super) fn text_links(&mut self, row: Rect, line: &Line, links: &[Candidate]) {
        for link in links {
            let column = line.text[..link.range.start].width() as u16;
            let text = &line.text[link.range.clone()];
            let rect = Rect::new(
                row.x + column,
                row.y,
                (text.width() as u16).min(row.width.saturating_sub(column)),
                1,
            );
            self.row(rect, text, None, false, false);
            self.style_last_row(rect, LINK);
            if rect.width > 0 {
                self.history_links.push((rect, link.target.clone()));
            }
        }
    }
}
