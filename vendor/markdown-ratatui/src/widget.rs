use crate::{DocumentRow, Layout};
use markdown_model::LinkId;
use ratatui_core::{buffer::Buffer, layout::Rect, text::Span, widgets::StatefulWidget};
use unicode_width::UnicodeWidthStr;

/// Caller-owned scrolling and link-selection state.
///
/// The widget only clamps its scroll position during rendering. Key bindings,
/// selection movement, and link activation belong to the host.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewState {
    scroll: usize,
    selected_link: Option<LinkId>,
}
impl ViewState {
    /// Returns the zero-based row currently shown at the top of the viewport.
    pub const fn scroll(&self) -> DocumentRow {
        DocumentRow::new(self.scroll)
    }
    /// Moves the viewport to a document-relative row.
    pub fn scroll_to(&mut self, row: DocumentRow) {
        self.scroll = row.index();
    }
    /// Requests the final possible viewport position.
    ///
    /// [`ViewState::clamp`] or widget rendering resolves the request against a
    /// concrete layout and viewport height.
    pub fn scroll_to_end(&mut self) {
        self.scroll = usize::MAX;
    }
    /// Moves the viewport forward by `rows`, saturating at the integer limit.
    pub fn scroll_forward(&mut self, rows: usize) {
        self.scroll = self.scroll.saturating_add(rows);
    }
    /// Moves the viewport backward by `rows`, saturating at the first row.
    pub fn scroll_backward(&mut self, rows: usize) {
        self.scroll = self.scroll.saturating_sub(rows);
    }
    /// Returns the selected document-local link identity.
    pub const fn selected_link(&self) -> Option<LinkId> {
        self.selected_link
    }
    /// Selects a link for styling, or clears selection with `None`.
    ///
    /// Opening a destination remains the caller's explicit action. An ID
    /// absent from the prepared document selects nothing.
    pub fn select_link(&mut self, link: Option<LinkId>) {
        self.selected_link = link;
    }
    /// Clamps a retained scroll offset to the last full viewport.
    ///
    /// Call this after changing documents or content-area height. If the
    /// document is shorter than `height`, `scroll` becomes zero. A zero height
    /// permits the offset to reach [`Layout::line_count`]. Rendering calls this
    /// method automatically when the buffer intersection is non-empty.
    pub fn clamp(&mut self, layout: &Layout, height: u16) {
        self.scroll = self
            .scroll
            .min(layout.line_count().saturating_sub(usize::from(height)));
    }
    /// Moves the viewport just enough to reveal a document-relative row.
    ///
    /// Rows above the viewport become its first row. Rows below it become its
    /// last row. With zero height, `scroll` becomes `row`. Call [`ViewState::clamp`]
    /// afterward if `row` might exceed the current layout.
    pub fn reveal(&mut self, row: DocumentRow, height: u16) {
        let row = row.index();
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll.saturating_add(usize::from(height)) {
            self.scroll = row.saturating_sub(usize::from(height).saturating_sub(1));
        }
    }
}
/// A borrowed, already-wrapped Markdown widget.
///
/// Create one with [`Layout::widget`]. Rendering intersects the supplied area
/// with the buffer, clamps scroll state, and clips rows and columns to that
/// intersection. A smaller area does not rewrap text; prepare a layout for the
/// new content width after a resize.
#[derive(Clone, Copy, Debug)]
pub struct MarkdownWidget<'a> {
    pub(crate) layout: &'a Layout,
}
impl StatefulWidget for MarkdownWidget<'_> {
    type State = ViewState;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }
        state.clamp(self.layout, area.height);
        for (offset, line) in self
            .layout
            .lines
            .iter()
            .skip(state.scroll)
            .take(usize::from(area.height))
            .enumerate()
        {
            let mut x = area.x;
            for run in line {
                if x >= area.right() {
                    break;
                }
                let style = if run.link.is_some() && run.link == state.selected_link {
                    run.style.patch(self.layout.selected)
                } else {
                    run.style
                };
                buf.set_span(
                    x,
                    area.y + offset as u16,
                    &Span::styled(&run.text, style),
                    area.right() - x,
                );
                x = x.saturating_add(run.text.width().min(usize::from(u16::MAX)) as u16);
            }
        }
    }
}
