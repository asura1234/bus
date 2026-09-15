//! Mouse text selection in the room notes, history and composer. Selections are
//! anchored to source text rather than screen cells, so they survive scrolling;
//! releasing the button copies the selected text to the host clipboard.
use super::render::{cell_offset, wrap_ranges, Action};
use super::*;
use crate::client::shell::{ClientShellAction, ClientShellInput};
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Region {
    Notes,
    History,
    Composer,
}

/// A byte offset within one wrapped history row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Point {
    pub line: usize,
    pub offset: usize,
}

/// Rendered Markdown does not have stable character offsets back into its
/// source. Expand either endpoint that touches a rendered reply so the visible
/// highlight describes the whole raw Markdown message copied to the clipboard.
pub(super) fn normalize_history_selection(
    lines: &[super::history::Line],
    selection: Option<(Point, Point)>,
) -> Option<(Point, Point)> {
    let (anchor, head) = selection?;
    let (mut start, mut end) = (anchor.min(head), anchor.max(head));
    if start == end {
        return Some((start, end));
    }
    if let Some((request, _)) = lines
        .get(start.line)
        .and_then(|line| line.raw_markdown.as_ref())
    {
        let mut last = start.line;
        while lines
            .get(last + 1)
            .and_then(|line| line.raw_markdown.as_ref())
            .is_some_and(|(candidate, _)| candidate == request)
        {
            last += 1;
        }
        if start.offset < lines[start.line].text.len() || start.line < last {
            while start.line > 0
                && lines[start.line - 1]
                    .raw_markdown
                    .as_ref()
                    .is_some_and(|(candidate, _)| candidate == request)
            {
                start.line -= 1;
            }
            start.offset = 0;
        }
    }
    if let Some((request, _)) = lines
        .get(end.line)
        .and_then(|line| line.raw_markdown.as_ref())
    {
        let mut first = end.line;
        while first > 0
            && lines[first - 1]
                .raw_markdown
                .as_ref()
                .is_some_and(|(candidate, _)| candidate == request)
        {
            first -= 1;
        }
        if end.offset > 0 || end.line > first {
            while lines
                .get(end.line + 1)
                .and_then(|line| line.raw_markdown.as_ref())
                .is_some_and(|(candidate, _)| candidate == request)
            {
                end.line += 1;
            }
            end.offset = lines[end.line].text.len();
        }
    }
    Some((start, end))
}

fn row_offset(text: &str, rect: Rect, column: u16, inclusive: bool) -> usize {
    column.checked_sub(rect.x).map_or(0, |column| {
        cell_offset(text, usize::from(column), inclusive)
    })
}

impl BusUi {
    /// Handles left-button selection gestures and reports whether it consumed
    /// the event. Presses on actionable rows are left to their actions.
    pub(super) fn selection_mouse(
        &mut self,
        mouse: &MouseEvent,
        hit: Option<&Action>,
        outcome: &mut ClientShellInput,
    ) -> bool {
        let (column, row) = (mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.form.is_some() || self.terminal.is_some() || self.rename.is_some() {
                    return false;
                }
                let point = (column, row).into();
                let region = match hit {
                    Some(Action::Notes) if self.view.notes.contains(point) => Region::Notes,
                    Some(Action::Composer) if self.view.composer.contains(point) => {
                        Region::Composer
                    }
                    None if self.view.history_text.contains(point) => Region::History,
                    _ => return false,
                };
                self.clear_selection();
                if region == Region::History {
                    let point = self.history_point(column, row, false);
                    self.history_selection = Some((point, point));
                } else {
                    let offset = self.editor_offset(region, column, row, false);
                    self.action(if region == Region::Notes {
                        Action::Notes
                    } else {
                        Action::Composer
                    });
                    let scroll = self.view.composer_scroll;
                    if let (Some(offset), Some(local)) = (
                        offset,
                        self.room.and_then(|room| self.locals.get_mut(&room)),
                    ) {
                        if region == Region::Notes {
                            local.notes.press(offset);
                        } else {
                            // Hold the viewport still while the caret follows the pointer.
                            local.composer_scroll = Some(scroll);
                            local.text.press(offset);
                        }
                    }
                }
                self.drag = Some(region);
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(region) = self.drag else {
                    return false;
                };
                if region == Region::History {
                    if let Some((anchor, _)) = self.history_selection {
                        let before = self.history_point(column, row, false);
                        let head = if before >= anchor {
                            self.history_point(column, row, true)
                        } else {
                            before
                        };
                        self.history_selection = Some((anchor, head));
                    }
                } else if let (Some(before), Some(after)) = (
                    self.editor_offset(region, column, row, false),
                    self.editor_offset(region, column, row, true),
                ) {
                    if let Some(local) = self.room.and_then(|room| self.locals.get_mut(&room)) {
                        let editor = if region == Region::Notes {
                            &mut local.notes
                        } else {
                            &mut local.text
                        };
                        let anchor = editor.anchor.unwrap_or(editor.cursor);
                        editor.drag(if before >= anchor { after } else { before });
                    }
                }
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if self.drag.take().is_none() {
                    return false;
                }
                match self.selected_text() {
                    Some(text) => outcome
                        .actions
                        .push(ClientShellAction::ClipboardWrite(text.into_bytes())),
                    None => self.clear_selection(),
                }
                true
            }
            _ => false,
        }
    }

    /// Editor byte offset under the pointer, using the last frame's viewport.
    /// Pointers above or below the editor select to its visible edge.
    fn editor_offset(
        &self,
        region: Region,
        column: u16,
        row: u16,
        inclusive: bool,
    ) -> Option<usize> {
        let local = self.locals.get(&self.room?)?;
        let (editor, rect, skip) = match region {
            Region::Notes => (&local.notes, self.view.notes, self.view.notes_scroll),
            Region::Composer => (&local.text, self.view.composer, self.view.composer_scroll),
            Region::History => return None,
        };
        if rect.height == 0 {
            return None;
        }
        let lines = wrap_ranges(&editor.text, rect.width);
        let last = (skip + usize::from(rect.height))
            .min(lines.len())
            .checked_sub(1)?;
        Some(if row < rect.y {
            lines[skip.min(last)].start
        } else if row >= rect.bottom() {
            lines[last].end
        } else {
            let line = &lines[(skip + usize::from(row - rect.y)).min(last)];
            line.start + row_offset(&editor.text[line.clone()], rect, column, inclusive)
        })
    }

    fn history_point(&self, column: u16, row: u16, inclusive: bool) -> Point {
        let rect = self.view.history_text;
        let lines = self.history.cached();
        let Some(last) = lines.len().checked_sub(1) else {
            return Point::default();
        };
        if row < rect.y {
            return Point {
                line: self.main_scroll.min(last),
                offset: 0,
            };
        }
        let visible = usize::from(row - rect.y).min(usize::from(rect.height.saturating_sub(1)));
        let line = (self.main_scroll + visible).min(last);
        let text = &lines[line].text;
        Point {
            line,
            offset: if row >= rect.bottom() {
                text.len()
            } else {
                row_offset(text, rect, column, inclusive)
            },
        }
    }

    /// Ordered history selection, if one is active.
    pub(super) fn history_selection_range(&self) -> Option<(Point, Point)> {
        normalize_history_selection(self.history.cached(), self.history_selection)
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        if let Some((start, end)) = self.history_selection_range() {
            let mut text = String::new();
            let mut copied_markdown = BTreeSet::new();
            for (index, line) in self
                .history
                .cached()
                .iter()
                .enumerate()
                .take(end.line.saturating_add(1))
                .skip(start.line)
            {
                let from = if index == start.line { start.offset } else { 0 };
                let to = if index == end.line {
                    end.offset
                } else {
                    line.text.len()
                };
                if let Some((request, source)) = &line.raw_markdown {
                    if from < to && copied_markdown.insert(*request) {
                        if index > start.line && !line.continued && !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(source);
                    } else if index == end.line
                        && to == 0
                        && index > start.line
                        && !line.continued
                        && !text.is_empty()
                    {
                        // The visible selection includes the preceding hard
                        // line end even though it stops before this reply.
                        text.push('\n');
                    }
                    continue;
                }
                // Soft-wrapped rows rejoin; real line ends stay line ends.
                if index > start.line && !line.continued {
                    text.push('\n');
                }
                text.push_str(line.text.get(from..to).unwrap_or_default());
            }
            return (!text.is_empty()).then_some(text);
        }
        let local = self.locals.get(&self.room?)?;
        [&local.notes, &local.text].into_iter().find_map(|editor| {
            editor
                .selection()
                .map(|range| editor.text[range].to_owned())
        })
    }

    pub(super) fn clear_selection(&mut self) {
        self.drag = None;
        self.history_selection = None;
        for local in self.locals.values_mut() {
            local.notes.anchor = None;
            local.text.anchor = None;
        }
    }

    /// Keyboard edits apply only to the focused editor's selection; any other
    /// highlight would no longer describe what a copy or edit acts on.
    pub(super) fn keep_focused_selection(&mut self, key: Option<KeyCode>) {
        if matches!(key, Some(KeyCode::Modifier(_))) {
            return;
        }
        self.drag = None;
        self.history_selection = None;
        let notes_focus = self.notes_focus;
        if let Some(local) = self.room.and_then(|room| self.locals.get_mut(&room)) {
            if notes_focus {
                local.text.anchor = None;
            } else {
                local.notes.anchor = None;
            }
        }
    }
}
