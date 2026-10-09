#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextCell {
    ch: char,
    start_col: u16,
    end_col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CellSpan {
    start: usize,
    end: usize,
}

impl CellSpan {
    fn contains(self, idx: usize) -> bool {
        idx >= self.start && idx <= self.end
    }

    fn columns(self, cells: &[TextCell]) -> (u16, u16) {
        (cells[self.start].start_col, cells[self.end].end_col)
    }
}

/// Finds the terminal display-column bounds for the token under a double-click.
///
/// The algorithm first maps text to terminal cells so wide characters and
/// zero-width marks use display columns, then prefers structured spans that
/// users expect to copy whole (URLs and quoted paths), and finally falls back
/// to a separator-delimited token.
pub(crate) fn word_bounds_at_column(
    row: &str,
    col: u16,
    width: impl Fn(char) -> u16,
) -> Option<(u16, u16)> {
    // Map the row into display cells before doing any word-boundary work.
    let cells = text_cells(row, width);
    let clicked_idx = cell_index_at_column(&cells, col)?;

    // Prefer spans that can legally include punctuation or spaces.
    let span = url_span_at_column(&cells, clicked_idx)
        .or_else(|| quoted_path_span_at_column(&cells, clicked_idx))
        .or_else(|| token_span_at_column(&cells, clicked_idx))?;

    // Convert the internal cell span back to inclusive terminal columns.
    Some(span.columns(&cells))
}

pub(crate) fn url_at_column(row: &str, col: u16, width: impl Fn(char) -> u16) -> Option<&str> {
    let cells = text_cells(row, width);
    let clicked_idx = cell_index_at_column(&cells, col)?;
    let span = url_spans(&cells)
        .into_iter()
        .find(|span| span.contains(clicked_idx))?;
    let start_byte = byte_index_for_cell(row, span.start);
    let end_byte = byte_index_after_cell(row, span.end);
    crate::utils::url::safe_web_url(row.get(start_byte..end_byte)?)
}

fn url_spans(cells: &[TextCell]) -> Vec<CellSpan> {
    let mut spans = Vec::new();
    let mut start = 0;
    while start < cells.len() {
        if starts_with_chars(&cells[start..], "http://")
            || starts_with_chars(&cells[start..], "https://")
        {
            let mut end = start;
            while end + 1 < cells.len() && !cells[end + 1].ch.is_whitespace() {
                end += 1;
            }
            if let Some(span) = trim_url_edges(cells, CellSpan { start, end }) {
                spans.push(span);
            }
            start = end + 1;
        } else {
            start += 1;
        }
    }
    spans
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VisibleTextCell {
    pub(crate) byte_index: usize,
    pub(crate) ch: char,
    pub(crate) logical_col: u16,
    pub(crate) screen_row: u16,
    pub(crate) screen_col: u16,
}

pub(crate) fn visible_text_cells(
    text: &str,
    pane_width: u16,
    width: impl Fn(char) -> u16,
) -> Vec<VisibleTextCell> {
    if pane_width == 0 {
        return Vec::new();
    }

    let mut cells = Vec::new();
    let mut screen_row = 0u16;
    let mut screen_col = 0u16;
    let mut logical_col = 0u16;
    let mut pending_wrap = false;
    for (byte_index, ch) in text.char_indices() {
        if ch == '\n' {
            screen_row = screen_row.saturating_add(1);
            screen_col = 0;
            logical_col = 0;
            pending_wrap = false;
            continue;
        }
        if pending_wrap {
            screen_row = screen_row.saturating_add(1);
            screen_col = 0;
            pending_wrap = false;
        }

        let width = width(ch);
        // The terminal moves a wide character to the next row instead of splitting it at the edge.
        if width > 1 && width <= pane_width && screen_col.saturating_add(width) > pane_width {
            screen_row = screen_row.saturating_add(1);
            screen_col = 0;
        }
        cells.push(VisibleTextCell {
            byte_index,
            ch,
            logical_col,
            screen_row,
            screen_col,
        });

        logical_col = logical_col.saturating_add(width);
        screen_col = screen_col.saturating_add(width);
        while screen_col > pane_width {
            screen_col -= pane_width;
            screen_row = screen_row.saturating_add(1);
        }
        if width > 0 && screen_col == pane_width {
            pending_wrap = true;
            screen_col = pane_width.saturating_sub(1);
        }
    }
    cells
}

pub(crate) fn logical_cell_for_visible_cell(
    text: &str,
    pane_width: u16,
    target_row: u16,
    target_col: u16,
    width: impl Fn(char) -> u16,
) -> Option<VisibleTextCell> {
    visible_text_cells(text, pane_width, &width)
        .into_iter()
        .find(|cell| {
            let width = width(cell.ch);
            cell.screen_row == target_row
                && if width == 0 {
                    target_col == cell.screen_col
                } else {
                    target_col >= cell.screen_col
                        && target_col < cell.screen_col.saturating_add(width)
                }
        })
}

fn token_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    if is_word_separator(cells[clicked_idx].ch) {
        return None;
    }

    let mut start = clicked_idx;
    while start > 0 && !is_word_separator(cells[start - 1].ch) {
        start -= 1;
    }

    let mut end = clicked_idx;
    while end + 1 < cells.len() && !is_word_separator(cells[end + 1].ch) {
        end += 1;
    }

    trim_token_edges(cells, CellSpan { start, end }).filter(|span| span.contains(clicked_idx))
}

fn text_cells(row: &str, width: impl Fn(char) -> u16) -> Vec<TextCell> {
    let mut next_col = 0u16;
    row.chars()
        .map(|ch| {
            let width = width(ch);
            let start_col = if width == 0 {
                next_col.saturating_sub(1)
            } else {
                next_col
            };
            if width > 0 {
                next_col = next_col.saturating_add(width);
            }
            TextCell {
                ch,
                start_col,
                end_col: next_col.saturating_sub(1),
            }
        })
        .collect()
}

fn cell_index_at_column(cells: &[TextCell], col: u16) -> Option<usize> {
    cells
        .iter()
        .position(|cell| cell.start_col <= col && col <= cell.end_col)
}

fn byte_index_for_cell(row: &str, cell_idx: usize) -> usize {
    row.char_indices()
        .nth(cell_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(row.len())
}

fn byte_index_after_cell(row: &str, cell_idx: usize) -> usize {
    row.char_indices()
        .nth(cell_idx.saturating_add(1))
        .map(|(idx, _)| idx)
        .unwrap_or(row.len())
}

fn url_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    let mut start = 0;
    while start < cells.len() {
        if starts_with_chars(&cells[start..], "http://")
            || starts_with_chars(&cells[start..], "https://")
        {
            let mut end = start;
            while end + 1 < cells.len() && !cells[end + 1].ch.is_whitespace() {
                end += 1;
            }
            if clicked_idx >= start && clicked_idx <= end {
                let span = trim_url_edges(cells, CellSpan { start, end })?;
                return span.contains(clicked_idx).then_some(span);
            }
            start = end + 1;
        } else {
            start += 1;
        }
    }
    None
}

fn trim_url_edges(cells: &[TextCell], span: CellSpan) -> Option<CellSpan> {
    let start = span.start;
    let mut end = span.end;
    while start <= end && should_trim_trailing_url_cell(cells, start, end) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn should_trim_trailing_url_cell(cells: &[TextCell], start: usize, end: usize) -> bool {
    match cells[end].ch {
        '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?' => true,
        ')' => !trailing_url_closer_is_balanced(cells, start, end, '(', ')'),
        ']' => !trailing_url_closer_is_balanced(cells, start, end, '[', ']'),
        '}' => !trailing_url_closer_is_balanced(cells, start, end, '{', '}'),
        _ => false,
    }
}

fn trailing_url_closer_is_balanced(
    cells: &[TextCell],
    start: usize,
    end: usize,
    open: char,
    close: char,
) -> bool {
    let mut balance = 0i32;
    for cell in &cells[start..end] {
        if cell.ch == open {
            balance += 1;
        } else if cell.ch == close {
            balance -= 1;
        }
    }
    balance > 0
}

fn quoted_path_span_at_column(cells: &[TextCell], clicked_idx: usize) -> Option<CellSpan> {
    let clicked = cells.get(clicked_idx)?.ch;
    if clicked == '"' || clicked == '\'' || clicked == '`' {
        return None;
    }

    for quote in ['"', '\'', '`'] {
        let mut start = None;
        for (idx, cell) in cells.iter().copied().enumerate() {
            let ch = cell.ch;
            if ch != quote || is_escaped(cells, idx) {
                continue;
            }
            if let Some(open) = start {
                if clicked_idx > open
                    && clicked_idx < idx
                    && cells[open + 1..idx].iter().any(|cell| cell.ch == '/')
                {
                    return Some(CellSpan {
                        start: open + 1,
                        end: idx - 1,
                    });
                }
                start = None;
            } else {
                start = Some(idx);
            }
        }
    }
    None
}

fn is_escaped(cells: &[TextCell], idx: usize) -> bool {
    let mut slashes = 0;
    let mut cursor = idx;
    while cursor > 0 && cells[cursor - 1].ch == '\\' {
        slashes += 1;
        cursor -= 1;
    }
    slashes % 2 == 1
}

fn starts_with_chars(cells: &[TextCell], prefix: &str) -> bool {
    prefix
        .chars()
        .enumerate()
        .all(|(idx, expected)| cells.get(idx).is_some_and(|cell| cell.ch == expected))
}

fn is_word_separator(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '|' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '!'
        )
}

fn trim_token_edges(cells: &[TextCell], span: CellSpan) -> Option<CellSpan> {
    let mut start = span.start;
    let mut end = span.end;
    while start <= end && is_leading_token_wrapper(cells[start].ch) {
        start += 1;
    }
    if start < end && cells[end].ch == '$' && is_trailing_token_wrapper(cells[end - 1].ch) {
        end -= 1;
    }
    while start <= end && is_trailing_token_wrapper(cells[end].ch) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    (start <= end).then_some(CellSpan { start, end })
}

fn is_leading_token_wrapper(ch: char) -> bool {
    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'' | '`')
}

fn is_trailing_token_wrapper(ch: char) -> bool {
    matches!(
        ch,
        ')' | ']' | '}' | '>' | '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?'
    )
}

#[cfg(test)]
#[path = "hit_test_support_test.rs"]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "hit_testing/tests/width_test.rs"]
mod width_tests;
