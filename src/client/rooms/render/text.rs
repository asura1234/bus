//! Display filtering and cell-aware wrapping/offsets.
use crate::messaging::model::{Provider, RuntimeStatus};
use ratatui::buffer::CellWidth;
use ratatui::style::Style;
use ratatui::text::Span;
use std::ops::Range;
use unicode_width::UnicodeWidthChar;

/// Display-only filtering: never place escape/control bytes in a terminal cell.
pub(in crate::client::rooms) fn display(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c == '\t' {
                ' '
            } else if c.is_control() {
                '�'
            } else {
                c
            }
        })
        .collect()
}
/// Terminal cells for one source character after `display` filtering.
pub(in crate::client::rooms) fn cell_width(c: char) -> usize {
    if c == '\t' || c.is_control() {
        1
    } else {
        c.width().unwrap_or(0)
    }
}
pub(super) fn cells(text: &str) -> usize {
    graphemes(text).map(|(_, width)| width).sum()
}
/// Source byte ranges and cell widths of the grapheme clusters the buffer
/// draws for `text`. `display` maps characters one to one, so walking its
/// clusters by character count recovers the source ranges.
fn graphemes(text: &str) -> impl Iterator<Item = (Range<usize>, usize)> + '_ {
    let shown = Span::raw(display(text));
    let clusters: Vec<_> = shown
        .styled_graphemes(Style::default())
        .map(|grapheme| {
            (
                grapheme.symbol.chars().count(),
                usize::from(grapheme.symbol.cell_width()),
            )
        })
        .collect();
    let mut chars = text.char_indices().map(|(index, _)| index).peekable();
    clusters.into_iter().map(move |(count, width)| {
        let start = chars.peek().copied().unwrap_or(text.len());
        let _ = chars.nth(count.saturating_sub(1));
        let end = chars.peek().copied().unwrap_or(text.len());
        (start..end, width)
    })
}
/// Word-wraps `text` into byte ranges, one per terminal row. Every byte except
/// the newline separators belongs to exactly one row, so cursors and
/// selections map back to the source. Spaces hang past the right edge instead
/// of starting a row, words longer than a row break at the edge, and wide
/// (CJK) characters may break on either side.
pub(in crate::client::rooms) fn wrap_ranges(text: &str, width: u16) -> Vec<Range<usize>> {
    if width == 0 {
        return Vec::new();
    }
    let width = usize::from(width);
    let mut lines = Vec::new();
    let mut base = 0;
    for source in text.split('\n') {
        let mut start = base;
        let mut used = 0;
        let mut breakpoint = None;
        for (offset, c) in source.char_indices() {
            let index = base + offset;
            let size = cell_width(c);
            if c == ' ' || c == '\t' {
                used += size;
                breakpoint = Some(index + c.len_utf8());
                continue;
            }
            if size > 1 {
                breakpoint = Some(index);
            }
            if used + size > width && index > start {
                match breakpoint.filter(|point| *point > start) {
                    Some(point) => {
                        lines.push(start..point);
                        used = cells(&text[point..index]);
                        start = point;
                    }
                    None => {
                        lines.push(start..index);
                        used = 0;
                        start = index;
                    }
                }
                breakpoint = None;
                if used + size > width && index > start {
                    lines.push(start..index);
                    used = 0;
                    start = index;
                }
            }
            used += size;
            if size > 1 {
                breakpoint = Some(index + c.len_utf8());
            }
        }
        base += source.len();
        lines.push(start..base);
        base += 1;
    }
    lines
}
pub(in crate::client::rooms) fn wrap(text: &str, width: u16) -> Vec<String> {
    wrap_ranges(text, width)
        .into_iter()
        .map(|range| display(&text[range]))
        .collect()
}
/// Row and cell column of a byte offset within wrapped rows. An offset on a
/// soft-wrap boundary belongs to the following row, where typing continues.
pub(in crate::client::rooms) fn wrapped_position(
    text: &str,
    lines: &[Range<usize>],
    offset: usize,
) -> (usize, usize) {
    let row = lines
        .iter()
        .rposition(|line| line.start <= offset)
        .unwrap_or(0);
    let column = lines.get(row).map_or(0, |line| {
        cells(&text[line.start..offset.clamp(line.start, line.end)])
    });
    (row, column)
}
/// Byte offset of the cell at `column` in `text`: before the grapheme there,
/// or after it when `inclusive` (a forward drag includes the cell under the
/// pointer). Columns past the end map to the end of the text.
pub(in crate::client::rooms) fn cell_offset(text: &str, column: usize, inclusive: bool) -> usize {
    let mut used = 0;
    for (range, width) in graphemes(text) {
        used += width;
        if used > column {
            return if inclusive { range.end } else { range.start };
        }
    }
    text.len()
}
pub(in crate::client::rooms) fn provider(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::ClaudeCode => "Claude Code",
        Provider::Cursor => "Cursor",
    }
}
pub(in crate::client::rooms) fn status(status: RuntimeStatus) -> &'static str {
    match status {
        RuntimeStatus::Idle => "Idle",
        RuntimeStatus::Working => "Working",
        RuntimeStatus::Blocked => "Blocked",
        RuntimeStatus::Launching => "Not ready",
        RuntimeStatus::Unavailable => "Unavailable",
    }
}
