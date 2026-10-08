//! Find in a room's chat history (Ctrl+F). It searches every rendered history
//! row, not just the visible ones, so the terminal's own find, which sees only
//! the screen, is not needed. Header rows are searched too, so author names
//! match as well as message text.
use super::BusUi;
use crossterm::event::{KeyCode, KeyModifiers};
use std::ops::Range;

#[derive(Clone, Debug)]
pub(super) struct ChatSearch {
    pub query: String,
    /// The selected match, an index into `View::search_matches`.
    pub current: usize,
    /// Set when the query changes: the next render selects the newest match
    /// and scrolls to it, since only a render knows the rows' current layout.
    pub jump_to_newest: bool,
    /// Where focus was when the search opened, restored when it closes.
    notes_focus: bool,
}

/// Every case-insensitive occurrence of `query` in the history rows, top to
/// bottom, as (row index, byte range in the row's text).
pub(super) fn find_matches(
    lines: &[super::history::Line],
    query: &str,
) -> Vec<(usize, Range<usize>)> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        // Lowercase per character, keeping each character's byte offset, so a
        // match maps back to the original text even where lowercasing changes
        // a character's length.
        let folded: Vec<(usize, char)> = line
            .text
            .char_indices()
            .flat_map(|(at, c)| c.to_lowercase().map(move |lower| (at, lower)))
            .collect();
        let mut start = 0;
        while start + needle.len() <= folded.len() {
            if folded[start..start + needle.len()]
                .iter()
                .map(|(_, c)| *c)
                .eq(needle.iter().copied())
            {
                let from = folded[start].0;
                let last = folded[start + needle.len() - 1].0;
                let to = last + line.text[last..].chars().next().map_or(0, char::len_utf8);
                matches.push((row, from..to));
                start += needle.len();
            } else {
                start += 1;
            }
        }
    }
    matches
}

impl BusUi {
    pub(super) fn open_chat_search(&mut self) {
        if self.room.is_none() {
            return;
        }
        self.chat_search = Some(ChatSearch {
            query: String::new(),
            current: 0,
            jump_to_newest: false,
            notes_focus: self.notes_focus,
        });
        self.notes_focus = false;
    }

    fn close_chat_search(&mut self) {
        if let Some(search) = self.chat_search.take() {
            self.notes_focus = search.notes_focus && self.room_has_notes();
        }
        self.view.search_matches.clear();
    }

    /// Keys while the search panel is open. Typing edits the query, which
    /// starts at the newest match. Enter or Up steps to the next older match
    /// (up the history) and Shift+Enter or Down to the next newer one, both
    /// wrapping; Down is there because some terminals report Shift+Enter as a
    /// plain Enter. Esc closes the panel; the history stays at the last match.
    pub(super) fn chat_search_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match (code, modifiers) {
            (KeyCode::Esc, _) => self.close_chat_search(),
            (KeyCode::Enter, KeyModifiers::SHIFT) | (KeyCode::Down, _) => {
                self.step_chat_search(true)
            }
            (KeyCode::Enter, _) | (KeyCode::Up, _) => self.step_chat_search(false),
            (KeyCode::Char('f' | 'F'), KeyModifiers::CONTROL) => self.step_chat_search(false),
            (KeyCode::Backspace, _) => self.edit_chat_query(|query| {
                query.pop();
            }),
            (KeyCode::Char(c), modifiers)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.edit_chat_query(|query| query.push(c))
            }
            _ => {}
        }
    }

    fn edit_chat_query(&mut self, edit: impl FnOnce(&mut String)) {
        if let Some(search) = &mut self.chat_search {
            edit(&mut search.query);
            search.jump_to_newest = true;
        }
    }

    /// `newer` moves down the history; matches are ordered top to bottom.
    fn step_chat_search(&mut self, newer: bool) {
        let count = self.view.search_matches.len();
        let Some(search) = self.chat_search.as_mut().filter(|_| count > 0) else {
            return;
        };
        search.current = if newer {
            (search.current + 1) % count
        } else {
            (search.current + count - 1) % count
        };
        let current = search.current;
        self.scroll_to_match(current);
    }

    /// Scrolls the history so the match sits a third of the way down the pane.
    pub(super) fn scroll_to_match(&mut self, index: usize) {
        let Some((row, _)) = self.view.search_matches.get(index) else {
            return;
        };
        let lead = usize::from(self.view.history.height) / 3;
        self.history_follow_tail = false;
        self.main_scroll = row.saturating_sub(lead).min(self.view.history_max_scroll);
    }
}
