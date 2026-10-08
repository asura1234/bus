use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use super::{
    PaneTerminal, TerminalSearchDirection, TerminalSearchWindow, TerminalTextMatch,
    TerminalTextPoint, TerminalWordMotion,
};

const COPY_MODE_WORD_SEPARATORS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^`{|}~";

impl PaneTerminal {
    pub(crate) fn search_text_window(
        &self,
        query: &str,
        case_sensitive: bool,
        direction: TerminalSearchDirection,
        cursor: TerminalTextPoint,
        previous: Option<(TerminalTextPoint, TerminalTextPoint)>,
        limit: usize,
    ) -> TerminalSearchWindow {
        let Some((buffer, active_screen)) = self.retained_text_buffer() else {
            return TerminalSearchWindow {
                matches: Vec::new(),
                current: None,
                current_global: None,
                total: 0,
            };
        };
        buffer.search_window(
            query,
            case_sensitive,
            active_screen,
            direction,
            cursor,
            previous,
            limit,
        )
    }

    pub(crate) fn word_motion_target(
        &self,
        row: u32,
        col: u16,
        motion: TerminalWordMotion,
    ) -> Option<TerminalTextPoint> {
        let core = self.ghostty.core.lock().ok()?;
        let cols = core.terminal.cols().ok()?;
        let total_rows = core.terminal.total_rows().ok()?;
        let row = usize::try_from(row).ok()?;
        if row >= total_rows {
            return None;
        }

        let mut window_rows = 64usize;
        loop {
            let (start_row, end_row) = match motion {
                TerminalWordMotion::PreviousStart => {
                    (row.saturating_sub(window_rows.saturating_sub(1)), row + 1)
                }
                TerminalWordMotion::NextStart | TerminalWordMotion::NextEnd => {
                    (row, row.saturating_add(window_rows).min(total_rows))
                }
                TerminalWordMotion::PreviousBigStart => {
                    (row.saturating_sub(window_rows.saturating_sub(1)), row + 1)
                }
                TerminalWordMotion::NextBigStart | TerminalWordMotion::NextBigEnd => {
                    (row, row.saturating_add(window_rows).min(total_rows))
                }
            };
            let rows = core
                .terminal
                .screen_text_rows_range(start_row, end_row)
                .ok()?;
            let starts_in_continuation = rows
                .first()
                .is_some_and(|row| row.wrap_continuation && start_row > 0);
            let ends_in_continuation = rows
                .last()
                .is_some_and(|row| row.soft_wrapped && end_row < total_rows);
            let buffer = RetainedTextBuffer::new_words(cols, rows, u32::try_from(start_row).ok()?);
            let target = buffer.word_motion(u32::try_from(row).ok()?, col, motion);
            let needs_more_history = (motion == TerminalWordMotion::PreviousStart
                || motion == TerminalWordMotion::PreviousBigStart)
                && target
                    .is_some_and(|target| starts_in_continuation && target.row == start_row as u32);
            let needs_more_future = (motion == TerminalWordMotion::NextEnd
                || motion == TerminalWordMotion::NextBigEnd)
                && ends_in_continuation
                && target.is_some_and(|target| buffer.point_is_final_atom(target));
            if target.is_some() && !needs_more_history && !needs_more_future {
                return target;
            }

            let reached_edge = match motion {
                TerminalWordMotion::PreviousStart => start_row == 0,
                TerminalWordMotion::NextStart | TerminalWordMotion::NextEnd => {
                    end_row == total_rows
                }
                TerminalWordMotion::PreviousBigStart => start_row == 0,
                TerminalWordMotion::NextBigStart | TerminalWordMotion::NextBigEnd => {
                    end_row == total_rows
                }
            };
            if reached_edge {
                return target;
            }
            window_rows = window_rows.saturating_mul(2).min(total_rows);
        }
    }

    pub(crate) fn dimensions(&self) -> Option<(u16, u16)> {
        let core = self.ghostty.core.lock().ok()?;
        Some((core.terminal.cols().ok()?, core.terminal.rows().ok()?))
    }

    pub(crate) fn paragraph_motion_target(
        &self,
        row: u32,
        direction: i8,
    ) -> Option<TerminalTextPoint> {
        let core = self.ghostty.core.lock().ok()?;
        let total_rows = core.terminal.total_rows().ok()?;
        let current = usize::try_from(row).ok()?;
        if current >= total_rows || direction == 0 {
            return None;
        }
        let limit = total_rows.min(1000);
        for distance in 1..limit {
            let candidate = if direction < 0 {
                current.checked_sub(distance)?
            } else {
                let candidate = current.saturating_add(distance);
                if candidate >= total_rows {
                    return None;
                }
                candidate
            };
            let rows = core
                .terminal
                .screen_text_rows_range(candidate, candidate.saturating_add(1))
                .ok()?;
            let row = rows.first()?;
            let is_blank = row.cells.iter().all(|cell| {
                terminal_cell_text(&cell.graphemes)
                    .chars()
                    .all(char::is_whitespace)
            });
            if is_blank {
                return Some(TerminalTextPoint {
                    row: u32::try_from(candidate).ok()?,
                    col: 0,
                });
            }
        }
        None
    }

    fn retained_text_buffer(&self) -> Option<(RetainedTextBuffer, crate::ghostty::ActiveScreen)> {
        let (cols, rows, active_screen) = {
            let core = self.ghostty.core.lock().ok()?;
            let cols = core.terminal.cols().ok()?;
            let rows = core.terminal.screen_text_rows().ok()?;
            let active_screen = core.terminal.active_screen().ok()?;
            (cols, rows, active_screen)
        };
        Some((RetainedTextBuffer::new_search(cols, rows, 0), active_screen))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextClass {
    Whitespace,
    Separator,
    Word,
}

#[derive(Debug)]
struct TextAtom {
    point: Option<TerminalTextPoint>,
    end_col: u16,
    class: TextClass,
}

#[derive(Debug)]
struct TextSpan {
    byte_start: usize,
    byte_end: usize,
    start: TerminalTextPoint,
    end: TerminalTextPoint,
}

#[derive(Debug, Default)]
struct LogicalTextLine {
    text: String,
    spans: Vec<TextSpan>,
}

#[derive(Debug)]
pub(super) struct RetainedTextBuffer {
    cols: u16,
    lines: Vec<LogicalTextLine>,
    atoms: Vec<TextAtom>,
}

impl RetainedTextBuffer {
    #[cfg(test)]
    pub(super) fn new(cols: u16, rows: Vec<crate::ghostty::ScreenTextRow>) -> Self {
        Self::build(cols, rows, 0, true, true)
    }

    fn new_search(cols: u16, rows: Vec<crate::ghostty::ScreenTextRow>, row_offset: u32) -> Self {
        Self::build(cols, rows, row_offset, true, false)
    }

    fn new_words(cols: u16, rows: Vec<crate::ghostty::ScreenTextRow>, row_offset: u32) -> Self {
        Self::build(cols, rows, row_offset, false, true)
    }

    fn build(
        cols: u16,
        rows: Vec<crate::ghostty::ScreenTextRow>,
        row_offset: u32,
        build_lines: bool,
        build_atoms: bool,
    ) -> Self {
        let mut lines = Vec::new();
        let mut line = LogicalTextLine::default();
        let mut atoms: Vec<TextAtom> = Vec::new();

        for (row_idx, row) in rows.into_iter().enumerate() {
            let Some(row_idx) = u32::try_from(row_idx).ok() else {
                break;
            };
            let row_idx = row_offset.saturating_add(row_idx);
            for (col, cell) in row.cells.into_iter().enumerate() {
                let Ok(col) = u16::try_from(col) else {
                    break;
                };
                if cell.wide == crate::ghostty::CellWide::SpacerTail {
                    continue;
                }
                if cell.wide == crate::ghostty::CellWide::SpacerHead {
                    if build_atoms {
                        atoms.push(TextAtom {
                            point: Some(TerminalTextPoint { row: row_idx, col }),
                            end_col: col,
                            class: atoms
                                .last()
                                .map_or(TextClass::Whitespace, |atom| atom.class),
                        });
                    }
                    continue;
                }
                let width = if cell.wide == crate::ghostty::CellWide::Wide {
                    2
                } else {
                    1
                };
                let text = terminal_cell_text(&cell.graphemes);
                let start = TerminalTextPoint { row: row_idx, col };
                let end = TerminalTextPoint {
                    row: row_idx,
                    col: col.saturating_add(width - 1),
                };
                if build_lines {
                    let byte_start = line.text.len();
                    line.text.push_str(&text);
                    let byte_end = line.text.len();
                    line.spans.push(TextSpan {
                        byte_start,
                        byte_end,
                        start,
                        end,
                    });
                }
                if build_atoms {
                    atoms.push(TextAtom {
                        point: Some(start),
                        end_col: end.col,
                        class: text_class(&text),
                    });
                }
            }

            if row.soft_wrapped {
                continue;
            }

            if build_lines {
                let trimmed_len = line.text.trim_end().len();
                while line
                    .spans
                    .last()
                    .is_some_and(|span| span.byte_start >= trimmed_len)
                {
                    line.spans.pop();
                }
                line.text.truncate(trimmed_len);
                lines.push(std::mem::take(&mut line));
            }
            if build_atoms {
                atoms.push(TextAtom {
                    point: None,
                    end_col: 0,
                    class: TextClass::Whitespace,
                });
            }
        }

        if build_lines && (!line.text.is_empty() || !line.spans.is_empty()) {
            lines.push(line);
        }

        Self { cols, lines, atoms }
    }

    pub(super) fn search_window(
        &self,
        query: &str,
        case_sensitive: bool,
        active_screen: crate::ghostty::ActiveScreen,
        direction: TerminalSearchDirection,
        cursor: TerminalTextPoint,
        previous: Option<(TerminalTextPoint, TerminalTextPoint)>,
        limit: usize,
    ) -> TerminalSearchWindow {
        if query.is_empty() || limit == 0 {
            return TerminalSearchWindow {
                matches: Vec::new(),
                current: None,
                current_global: None,
                total: 0,
            };
        }
        let Ok(regex) = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(!case_sensitive)
            .build()
        else {
            return TerminalSearchWindow {
                matches: Vec::new(),
                current: None,
                current_global: None,
                total: 0,
            };
        };
        let to_match = |line: &LogicalTextLine, found: regex::Match<'_>| {
            let start_index = line
                .spans
                .binary_search_by_key(&found.start(), |span| span.byte_start)
                .ok()?;
            let end_index = line
                .spans
                .binary_search_by_key(&found.end(), |span| span.byte_end)
                .ok()?;
            let start_span = &line.spans[start_index];
            let end_span = &line.spans[end_index];
            Some(TerminalTextMatch {
                start: start_span.start,
                end: end_span.end,
                source_fingerprint: text_fingerprint(found.as_str()),
                scan_cols: self.cols,
                scan_screen: active_screen,
            })
        };

        let origin = match direction {
            TerminalSearchDirection::Forward => previous.map_or(cursor, |(_, end)| end),
            TerminalSearchDirection::Backward => previous.map_or(cursor, |(start, _)| start),
        };
        let mut total = 0usize;
        let mut target = None;
        for line in &self.lines {
            for found in regex.find_iter(&line.text) {
                let Some(text_match) = to_match(line, found) else {
                    continue;
                };
                match direction {
                    TerminalSearchDirection::Forward
                        if target.is_none() && text_match.start > origin =>
                    {
                        target = Some(total);
                    }
                    TerminalSearchDirection::Backward if text_match.end < origin => {
                        target = Some(total);
                    }
                    _ => {}
                }
                total = total.saturating_add(1);
            }
        }
        if total == 0 {
            return TerminalSearchWindow {
                matches: Vec::new(),
                current: None,
                current_global: None,
                total: 0,
            };
        }
        let target = target.unwrap_or(match direction {
            TerminalSearchDirection::Forward => 0,
            TerminalSearchDirection::Backward => total - 1,
        });
        let retained = limit.min(total);
        let start = target
            .saturating_sub(retained / 2)
            .min(total.saturating_sub(retained));
        let end = start.saturating_add(retained);
        let mut index = 0usize;
        let mut matches = Vec::with_capacity(retained);
        for line in &self.lines {
            for found in regex.find_iter(&line.text) {
                let Some(text_match) = to_match(line, found) else {
                    continue;
                };
                if index >= start && index < end {
                    matches.push(text_match);
                }
                index = index.saturating_add(1);
                if index >= end {
                    break;
                }
            }
            if index >= end {
                break;
            }
        }
        TerminalSearchWindow {
            matches,
            current: Some(target - start),
            current_global: Some(target),
            total,
        }
    }

    pub(super) fn word_motion(
        &self,
        row: u32,
        col: u16,
        motion: TerminalWordMotion,
    ) -> Option<TerminalTextPoint> {
        let current = self.atoms.iter().position(|atom| {
            atom.point
                .is_some_and(|point| point.row == row && col >= point.col && col <= atom.end_col)
        })?;
        match motion {
            TerminalWordMotion::NextStart => self.next_word_start(current),
            TerminalWordMotion::PreviousStart => self.previous_word_start(current),
            TerminalWordMotion::NextEnd => self.next_word_end(current),
            TerminalWordMotion::NextBigStart => self.next_big_word_start(current),
            TerminalWordMotion::PreviousBigStart => self.previous_big_word_start(current),
            TerminalWordMotion::NextBigEnd => self.next_big_word_end(current),
        }
    }

    fn next_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let current_class = self.atoms.get(current)?.class;
        let mut next = current.saturating_add(1);
        if current_class != TextClass::Whitespace {
            while self
                .atoms
                .get(next)
                .is_some_and(|atom| atom.class == current_class)
            {
                next += 1;
            }
        }
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        self.next_point(next)
    }

    fn previous_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut previous = current.checked_sub(1)?;
        while self
            .atoms
            .get(previous)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            previous = previous.checked_sub(1)?;
        }
        let class = self.atoms.get(previous)?.class;
        while previous > 0
            && self
                .atoms
                .get(previous - 1)
                .is_some_and(|atom| atom.class == class)
        {
            previous -= 1;
        }
        self.previous_point(previous)
    }

    fn next_word_end(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut next = current.saturating_add(1);
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        let class = self.atoms.get(next)?.class;
        while self
            .atoms
            .get(next + 1)
            .is_some_and(|atom| atom.class == class)
        {
            next += 1;
        }
        self.previous_point(next)
    }

    fn next_big_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut next = current.saturating_add(1);
        if self
            .atoms
            .get(current)
            .is_some_and(|atom| atom.class != TextClass::Whitespace)
        {
            while self
                .atoms
                .get(next)
                .is_some_and(|atom| atom.class != TextClass::Whitespace)
            {
                next += 1;
            }
        }
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        self.next_point(next)
    }

    fn previous_big_word_start(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut previous = current.checked_sub(1)?;
        while self
            .atoms
            .get(previous)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            previous = previous.checked_sub(1)?;
        }
        while previous > 0
            && self
                .atoms
                .get(previous - 1)
                .is_some_and(|atom| atom.class != TextClass::Whitespace)
        {
            previous -= 1;
        }
        self.previous_point(previous)
    }

    fn next_big_word_end(&self, current: usize) -> Option<TerminalTextPoint> {
        let mut next = current.saturating_add(1);
        while self
            .atoms
            .get(next)
            .is_some_and(|atom| atom.class == TextClass::Whitespace)
        {
            next += 1;
        }
        self.atoms.get(next)?;
        while self
            .atoms
            .get(next + 1)
            .is_some_and(|atom| atom.class != TextClass::Whitespace)
        {
            next += 1;
        }
        self.previous_point(next)
    }

    fn next_point(&self, mut index: usize) -> Option<TerminalTextPoint> {
        while let Some(atom) = self.atoms.get(index) {
            if let Some(point) = atom.point {
                return Some(point);
            }
            index += 1;
        }
        None
    }

    fn previous_point(&self, mut index: usize) -> Option<TerminalTextPoint> {
        loop {
            if let Some(point) = self.atoms.get(index)?.point {
                return Some(point);
            }
            index = index.checked_sub(1)?;
        }
    }

    fn point_is_final_atom(&self, point: TerminalTextPoint) -> bool {
        // Word motion targets are atom start points, so compare against the
        // final atom's start point. Comparing against `end_col` would never
        // match a wide glyph, whose end column is one past its start.
        self.atoms
            .iter()
            .rev()
            .find(|atom| atom.point.is_some())
            .is_some_and(|atom| atom.point == Some(point))
    }
}

fn terminal_cell_text(graphemes: &[u32]) -> String {
    if graphemes.is_empty()
        || graphemes.first().copied() == Some(crate::ghostty::KITTY_UNICODE_PLACEHOLDER)
    {
        return " ".to_string();
    }
    graphemes
        .iter()
        .map(|codepoint| char::from_u32(*codepoint).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn text_class(text: &str) -> TextClass {
    let Some(ch) = text.chars().next() else {
        return TextClass::Whitespace;
    };
    if ch.is_whitespace() {
        TextClass::Whitespace
    } else if ch.is_ascii() && COPY_MODE_WORD_SEPARATORS.contains(ch) {
        TextClass::Separator
    } else {
        TextClass::Word
    }
}

fn text_fingerprint(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
