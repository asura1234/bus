//! Markdown message preparation, style preservation and copyable row layout.
use super::super::render::{display, wrap_ranges};
use super::{History, Line, MarkdownSource, RowAnchor, Tone};
use markdown_ratatui::{DocumentRow, LayoutOptions, MarkdownView, Theme, ViewState};
use ratatui::buffer::{Buffer, CellWidth};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::StatefulWidget;
use std::sync::Arc;

pub(super) struct MarkdownBlock {
    source: Arc<str>,
    view: Option<MarkdownView>,
    width: Option<u16>,
    lines: Vec<Line>,
}

impl History {
    pub(super) fn markdown_block(&mut self, key: MarkdownSource, text: &str) -> &mut MarkdownBlock {
        let block = self
            .markdown
            .entry(key)
            .or_insert_with(|| MarkdownBlock::new(key, text));
        if block.source.as_ref() != text {
            *block = MarkdownBlock::new(key, text);
        }
        block
    }
}

impl MarkdownBlock {
    fn new(key: MarkdownSource, source: &str) -> Self {
        let raw: Arc<str> = Arc::from(source);
        // People type prompts like chat messages, so every newline they enter
        // is a line break. Agent replies keep standard Markdown paragraphs.
        let rendered = match key {
            MarkdownSource::Prompt(_) => std::borrow::Cow::Owned(source.replace('\n', "  \n")),
            MarkdownSource::Reply(_) => std::borrow::Cow::Borrowed(source),
        };
        let view = match MarkdownView::new(&rendered) {
            Ok(mut view) => {
                view.set_options(LayoutOptions {
                    theme: Theme {
                        text: Style::default(),
                        heading: Style::new().bold(),
                        link: Style::new().fg(Color::Cyan).underlined(),
                        code: Style::new().fg(Color::Cyan),
                        muted: Style::new().add_modifier(Modifier::DIM),
                        selected: Style::default(),
                    },
                    ..LayoutOptions::default()
                });
                Some(view)
            }
            Err(error) => {
                tracing::warn!(event = "bus.markdown.parse_failed", %error);
                None
            }
        };
        Self {
            source: raw,
            view,
            width: None,
            lines: Vec::new(),
        }
    }

    pub(super) fn lines(
        &mut self,
        width: u16,
        request: MarkdownSource,
        indent: &str,
        anchor: RowAnchor,
    ) -> &[Line] {
        if self.width == Some(width) {
            return &self.lines;
        }
        let content_width = width.saturating_sub(indent.len() as u16).max(1);
        self.lines = self
            .view
            .as_mut()
            .and_then(|view| {
                prepared_markdown_lines(view, content_width, request, &self.source, indent, anchor)
            })
            .unwrap_or_else(|| {
                literal_reply_lines(&self.source, content_width, request, indent, anchor)
            });
        self.width = Some(width);
        &self.lines
    }
}

fn prepared_markdown_lines(
    view: &mut MarkdownView,
    width: u16,
    request: MarkdownSource,
    source: &Arc<str>,
    indent: &str,
    anchor: RowAnchor,
) -> Option<Vec<Line>> {
    let layout = match view.prepare(width) {
        Ok(layout) => layout,
        Err(error) => {
            tracing::warn!(event = "bus.markdown.layout_failed", %error);
            return None;
        }
    };
    let line_count = layout.line_count();
    let mut lines = Vec::with_capacity(line_count);
    let mut first = 0;
    while first < line_count {
        // Bound the temporary cell buffer independently of reply length. The
        // prepared layout remains cached; chunks only adapt its styled runs to
        // the room history's existing row representation.
        const MAX_BUFFER_CELLS: usize = 256 * 1024;
        let rows_per_chunk = (MAX_BUFFER_CELLS / usize::from(width.max(1))).max(1);
        let height = (line_count - first)
            .min(rows_per_chunk)
            .min(usize::from(u16::MAX)) as u16;
        let area = Rect::new(0, 0, width, height);
        let mut buffer = Buffer::empty(area);
        let mut state = ViewState::default();
        state.scroll_to(DocumentRow::new(first));
        layout.widget().render(area, &mut buffer, &mut state);
        for offset in 0..height {
            let position = first + usize::from(offset);
            lines.push(line_from_buffer(
                &buffer,
                offset,
                width,
                request,
                source,
                indent,
                RowAnchor { position, ..anchor },
            ));
        }
        first += usize::from(height);
    }
    mark_soft_wraps(&mut lines, width, indent.len());
    Some(lines)
}

/// Marks which rendered Markdown rows continue a soft-wrapped line. The
/// renderer keeps no such flag and trims the space at each wrap, so a row
/// counts as a wrap of the one above when its first word could not have fit
/// on that row. Copying then rejoins it with one space instead of a line end.
fn mark_soft_wraps(lines: &mut [Line], width: u16, indent: usize) {
    use unicode_width::UnicodeWidthStr;
    for index in 1..lines.len() {
        let previous = lines[index - 1].text.get(indent..).unwrap_or_default();
        let current = lines[index]
            .text
            .get(indent..)
            .unwrap_or_default()
            .trim_start();
        let first_word = current.split(' ').next().unwrap_or_default();
        let wrapped = !previous.trim().is_empty()
            && !first_word.is_empty()
            && previous.width() + 1 + first_word.width() > usize::from(width);
        lines[index].continued = wrapped;
        lines[index].rejoin_space = wrapped;
    }
}

#[allow(clippy::too_many_arguments)]
fn line_from_buffer(
    buffer: &Buffer,
    row: u16,
    width: u16,
    request: MarkdownSource,
    source: &Arc<str>,
    indent: &str,
    anchor: RowAnchor,
) -> Line {
    let mut text = indent.to_owned();
    let mut styles: Vec<(String, Style)> = if indent.is_empty() {
        Vec::new()
    } else {
        vec![(indent.to_owned(), Style::default())]
    };
    let mut column = 0u16;
    while column < width {
        let cell = &buffer[(column, row)];
        let symbol = cell.symbol();
        let mut style = cell.style();
        // A scratch buffer represents unspecified colors as Reset. Treat those
        // as transparent so Markdown modifiers patch the Bus room palette
        // instead of replacing it with the user's terminal defaults.
        if style.fg == Some(Color::Reset) {
            style.fg = None;
        }
        if style.bg == Some(Color::Reset) {
            style.bg = None;
        }
        if style.underline_color == Some(Color::Reset) {
            style.underline_color = None;
        }
        if !symbol.is_empty() {
            text.push_str(symbol);
            if let Some((run, _)) = styles.last_mut().filter(|(_, current)| *current == style) {
                run.push_str(symbol);
            } else {
                styles.push((symbol.to_owned(), style));
            }
        }
        column = column.saturating_add(cell.cell_width().max(1));
    }
    while text.len() > indent.len() && text.ends_with(' ') {
        text.pop();
        if let Some((run, _)) = styles.last_mut() {
            debug_assert!(run.ends_with(' '));
            run.pop();
            if run.is_empty() {
                styles.pop();
            }
        }
    }
    Line {
        text,
        action: None,
        tone: Tone::Text,
        spans: Vec::new(),
        styles,
        thumbnail: None,
        raw_markdown: Some((request, Arc::clone(source))),
        // Set by `mark_soft_wraps` once every row of the message exists.
        continued: false,
        rejoin_space: false,
        copy_from: indent.len(),
        anchor,
    }
}

fn literal_reply_lines(
    source: &Arc<str>,
    width: u16,
    request: MarkdownSource,
    indent: &str,
    anchor: RowAnchor,
) -> Vec<Line> {
    let rows = wrap_ranges(source, width);
    rows.iter()
        .enumerate()
        .map(|(index, row)| Line {
            text: format!("{indent}{}", display(&source[row.clone()])),
            action: None,
            tone: Tone::Text,
            spans: Vec::new(),
            styles: Vec::new(),
            thumbnail: None,
            raw_markdown: Some((request, Arc::clone(source))),
            continued: index > 0 && rows[index - 1].end == row.start,
            rejoin_space: false,
            copy_from: indent.len(),
            anchor: RowAnchor {
                position: row.start,
                ..anchor
            },
        })
        .collect()
}

// Keep styling attached to header fields, not matches against arbitrary message
// text. Wrapping preserves every byte of the sanitized source, including UTF-8.
