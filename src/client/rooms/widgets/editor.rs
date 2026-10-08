use crossterm::event::{KeyCode, KeyModifiers};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KillDir {
    Backward,
    Forward,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Editor {
    pub text: String,
    pub cursor: usize,
    /// Fixed end of a mouse selection; the selection spans anchor..cursor.
    pub anchor: Option<usize>,
    undo: Vec<(String, usize, Option<usize>)>,
    kill: Vec<String>,
    kill_index: usize,
    last_kill: Option<KillDir>,
    last_yank: Option<(usize, usize)>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    pub fn new(text: String) -> Self {
        Self {
            cursor: text.len(),
            text,
            ..Self::default()
        }
    }
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor.filter(|anchor| *anchor != self.cursor)?;
        Some(anchor.min(self.cursor)..anchor.max(self.cursor))
    }
    /// Starts a mouse selection at a UTF-8 boundary returned by the view.
    pub fn press(&mut self, offset: usize) {
        self.cursor = offset.min(self.text.len());
        self.anchor = Some(self.cursor);
    }
    pub fn drag(&mut self, offset: usize) {
        self.anchor.get_or_insert(self.cursor);
        self.cursor = offset.min(self.text.len());
    }
    pub fn at_first_line(&self) -> bool {
        !self.text[..self.cursor].contains('\n')
    }
    pub fn at_last_line(&self) -> bool {
        !self.text[self.cursor..].contains('\n')
    }
    fn take_selection(&mut self) -> Option<Range<usize>> {
        let selection = self.selection();
        self.anchor = None;
        selection
    }
    fn record_undo(&mut self) {
        if self.undo.len() >= 100 {
            self.undo.remove(0);
        }
        self.undo
            .push((self.text.clone(), self.cursor, self.anchor));
    }
    pub fn insert(&mut self, text: &str) {
        self.record_undo();
        self.insert_raw(text);
        self.last_kill = None;
        self.last_yank = None;
    }
    fn insert_raw(&mut self, text: &str) {
        if let Some(range) = self.take_selection() {
            self.text.replace_range(range.clone(), "");
            self.cursor = range.start;
        }
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }
    fn line_start(&self) -> usize {
        self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
    }
    fn line_end(&self) -> usize {
        self.cursor
            + self.text[self.cursor..]
                .find('\n')
                .unwrap_or(self.text.len() - self.cursor)
    }
    fn space_start(&self) -> usize {
        let prefix = &self.text[..self.cursor];
        let end = prefix.trim_end_matches(char::is_whitespace).len();
        prefix[..end]
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0)
    }
    fn kill_range(&mut self, range: Range<usize>, direction: KillDir) {
        if range.is_empty() {
            return;
        }
        self.record_undo();
        let killed = self.text[range.clone()].to_string();
        match (self.last_kill, direction) {
            (Some(KillDir::Backward), KillDir::Backward) => {
                if let Some(last) = self.kill.last_mut() {
                    last.insert_str(0, &killed);
                } else {
                    self.kill.push(killed);
                }
            }
            (Some(KillDir::Forward), KillDir::Forward) => {
                if let Some(last) = self.kill.last_mut() {
                    last.push_str(&killed);
                } else {
                    self.kill.push(killed);
                }
            }
            _ => self.kill.push(killed),
        }
        self.kill_index = self.kill.len().saturating_sub(1);
        self.last_kill = Some(direction);
        self.last_yank = None;
        self.text.replace_range(range.clone(), "");
        self.cursor = range.start;
        self.anchor = None;
    }
    fn kill_to_line_start(&mut self) {
        let start = self.line_start();
        if self.cursor > start {
            self.kill_range(start..self.cursor, KillDir::Backward);
        } else if self.cursor > 0 {
            let prev = self.text[..self.cursor - 1]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            self.kill_range(prev..self.cursor, KillDir::Backward);
        }
    }
    fn kill_to_line_end(&mut self) {
        let end = self.line_end();
        if self.cursor < end {
            self.kill_range(self.cursor..end, KillDir::Forward);
        } else if self.cursor < self.text.len() {
            self.kill_range(self.cursor..self.cursor + 1, KillDir::Forward);
        }
    }
    fn yank(&mut self) {
        let Some(text) = self.kill.last().cloned() else {
            return;
        };
        self.record_undo();
        self.last_kill = None;
        let start = self.cursor;
        self.insert_raw(&text);
        self.last_yank = Some((start, text.len()));
        self.kill_index = self.kill.len().saturating_sub(1);
    }
    fn yank_pop(&mut self) {
        let Some((start, len)) = self.last_yank else {
            return;
        };
        if self.kill.len() < 2 || start + len > self.text.len() {
            return;
        }
        self.record_undo();
        self.kill_index = (self.kill_index + self.kill.len() - 1) % self.kill.len();
        let replacement = self.kill[self.kill_index].clone();
        self.text.replace_range(start..start + len, &replacement);
        self.cursor = start + replacement.len();
        self.last_yank = Some((start, replacement.len()));
    }
    fn undo(&mut self) {
        self.last_kill = None;
        self.last_yank = None;
        if let Some((text, cursor, anchor)) = self.undo.pop() {
            self.text = text;
            self.cursor = cursor;
            self.anchor = anchor;
        }
    }
    fn word_start(&self) -> usize {
        let mut start = self.cursor;
        let mut seen_word = false;
        for (index, c) in self.text[..self.cursor].char_indices().rev() {
            if is_word(c) {
                seen_word = true;
            } else if seen_word {
                break;
            }
            start = index;
        }
        start
    }
    fn word_end(&self) -> usize {
        let mut seen_word = false;
        for (index, c) in self.text[self.cursor..].char_indices() {
            if is_word(c) {
                seen_word = true;
            } else if seen_word {
                return self.cursor + index;
            }
        }
        self.text.len()
    }
    pub fn key(&mut self, key: KeyCode, modifiers: KeyModifiers) -> bool {
        let before = self.text.clone();
        // Option is Alt with most macOS terminal settings and Meta with some;
        // legacy terminals send Option+arrows as the readline chords Alt+B/F.
        let option = modifiers.intersects(KeyModifiers::ALT | KeyModifiers::META);
        let control = modifiers.contains(KeyModifiers::CONTROL) && !option;
        if (control && matches!(key, KeyCode::Char('u')))
            || (matches!(key, KeyCode::Backspace) && modifiers.contains(KeyModifiers::SUPER))
        {
            self.kill_to_line_start();
            return self.text != before;
        }
        if control && matches!(key, KeyCode::Char('k')) {
            self.kill_to_line_end();
            return self.text != before;
        }
        if control && matches!(key, KeyCode::Char('w')) {
            if let Some(range) = self.selection() {
                self.kill_range(range, KillDir::Backward);
            } else {
                self.kill_range(self.space_start()..self.cursor, KillDir::Backward);
            }
            return self.text != before;
        }
        if control && matches!(key, KeyCode::Char('y')) {
            self.yank();
            return self.text != before;
        }
        if option && matches!(key, KeyCode::Char('y')) {
            self.yank_pop();
            return self.text != before;
        }
        if control && matches!(key, KeyCode::Char('_' | '-')) {
            self.undo();
            return self.text != before;
        }
        let (key, word) = match key {
            KeyCode::Char('a') if control => (KeyCode::Home, false),
            KeyCode::Char('e') if control => (KeyCode::End, false),
            KeyCode::Char('b') if option => (KeyCode::Left, true),
            KeyCode::Char('f') if option => (KeyCode::Right, true),
            KeyCode::Char('d') if option => (KeyCode::Delete, true),
            KeyCode::Left | KeyCode::Right | KeyCode::Backspace | KeyCode::Delete => {
                (key, option || modifiers.contains(KeyModifiers::CONTROL))
            }
            _ => (key, false),
        };
        if let Some(range) = self.take_selection() {
            match key {
                KeyCode::Backspace | KeyCode::Delete => {
                    self.record_undo();
                    self.text.replace_range(range.clone(), "");
                    self.cursor = range.start;
                    return true;
                }
                KeyCode::Left => {
                    self.cursor = range.start;
                    return false;
                }
                KeyCode::Right => {
                    self.cursor = range.end;
                    return false;
                }
                _ => {}
            }
        }
        let previous = if word {
            self.word_start()
        } else {
            self.text[..self.cursor]
                .char_indices()
                .last()
                .map_or(0, |(i, _)| i)
        };
        let next = if word {
            self.word_end()
        } else {
            self.text[self.cursor..]
                .chars()
                .next()
                .map_or(self.cursor, |c| self.cursor + c.len_utf8())
        };
        match key {
            KeyCode::Left => self.cursor = previous,
            KeyCode::Right => self.cursor = next,
            KeyCode::Home => self.cursor = self.line_start(),
            KeyCode::End => self.cursor = self.line_end(),
            KeyCode::Backspace => {
                if previous != self.cursor {
                    self.record_undo();
                    self.text.replace_range(previous..self.cursor, "");
                    self.cursor = previous;
                }
            }
            KeyCode::Delete => {
                if next != self.cursor {
                    self.record_undo();
                    self.text.replace_range(self.cursor..next, "");
                }
            }
            KeyCode::Up | KeyCode::Down => {
                let start = self.line_start();
                let col = self.text[start..self.cursor].chars().count();
                let target = if key == KeyCode::Up {
                    if start == 0 {
                        return false;
                    }
                    self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1)
                } else {
                    let Some(end) = self.text[self.cursor..].find('\n') else {
                        return false;
                    };
                    self.cursor + end + 1
                };
                let line = self.text[target..].split('\n').next().unwrap_or_default();
                self.cursor = target + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
            }
            _ => {}
        }
        self.text != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_unicode_at_cursor_without_destroying_neighboring_text() {
        let mut editor = Editor::default();
        editor.insert("a界c");
        editor.key(KeyCode::Left, KeyModifiers::NONE);
        editor.key(KeyCode::Backspace, KeyModifiers::NONE);
        editor.insert("文");
        assert_eq!(editor.text, "a文c");
        assert_eq!(editor.cursor, 4);
        editor.key(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(editor.text, "a文");
    }

    #[test]
    fn multiline_home_end_and_vertical_movement_keep_valid_utf8_boundaries() {
        let mut editor = Editor::default();
        editor.insert("ab\n界x\nlast");
        editor.key(KeyCode::Home, KeyModifiers::NONE);
        editor.key(KeyCode::Up, KeyModifiers::NONE);
        editor.key(KeyCode::Right, KeyModifiers::NONE);
        editor.insert("!");
        assert_eq!(editor.text, "ab\n界!x\nlast");
        editor.key(KeyCode::End, KeyModifiers::NONE);
        editor.key(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(editor.text, "ab\n界!xlast");
    }

    #[test]
    fn option_arrows_move_by_word_across_punctuation_and_unicode() {
        let mut editor = Editor::new("say 你好, foo_bar.baz  ".into());
        let mut stops = Vec::new();
        for _ in 0..5 {
            editor.key(KeyCode::Left, KeyModifiers::ALT);
            stops.push(editor.cursor);
        }
        assert_eq!(stops, [20, 12, 4, 0, 0]);
        for (code, modifiers, expected) in [
            (KeyCode::Right, KeyModifiers::ALT, 3),
            (KeyCode::Char('f'), KeyModifiers::ALT, 10),
            (KeyCode::Right, KeyModifiers::META, 19),
            (KeyCode::Right, KeyModifiers::CONTROL, 23),
            (KeyCode::Char('b'), KeyModifiers::ALT, 20),
        ] {
            editor.key(code, modifiers);
            assert_eq!(editor.cursor, expected, "{code:?} {modifiers:?}");
        }
    }

    #[test]
    fn option_delete_removes_one_word_in_either_direction() {
        let mut editor = Editor::new("keep this  word".into());
        editor.key(KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(editor.text, "keep this  ");
        editor.key(KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(editor.text, "keep ");
        editor.insert("that one");
        editor.key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "keep that ");
        editor.key(KeyCode::Home, KeyModifiers::NONE);
        editor.key(KeyCode::Delete, KeyModifiers::ALT);
        assert_eq!(editor.text, " that ");
        editor.key(KeyCode::Char('d'), KeyModifiers::ALT);
        assert_eq!(editor.text, " ");
        assert_eq!(editor.cursor, 0);
    }

    #[test]
    fn typing_or_deleting_replaces_a_mouse_selection_and_arrows_collapse_it() {
        let mut editor = Editor::new("hello brave world".into());
        editor.press(6);
        editor.drag(12);
        assert_eq!(editor.selection(), Some(6..12));
        editor.insert("new ");
        assert_eq!(editor.text, "hello new world");
        assert_eq!(editor.selection(), None);

        editor.press(10);
        editor.drag(6);
        editor.key(KeyCode::Backspace, KeyModifiers::ALT);
        assert_eq!(editor.text, "hello world");
        assert_eq!(editor.cursor, 6);

        editor.press(0);
        editor.drag(5);
        editor.key(KeyCode::Left, KeyModifiers::NONE);
        assert_eq!((editor.cursor, editor.selection()), (0, None));
        editor.key(KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(editor.cursor, 1, "a collapsed click never extends later");
    }

    #[test]
    fn ctrl_a_e_jump_the_current_line_and_ctrl_w_kills_a_path_at_whitespace() {
        let mut editor = Editor::new("ab\ncd ef\ngh".into());
        editor.cursor = 8;
        editor.key(KeyCode::Char('a'), KeyModifiers::CONTROL);
        assert_eq!(editor.cursor, 3);
        editor.key(KeyCode::Char('e'), KeyModifiers::CONTROL);
        assert_eq!(editor.cursor, 8);
        editor.key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "ab\ncd \ngh");
        editor.insert("/tmp/foo/bar.txt");
        editor.key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "ab\ncd \ngh");
    }

    #[test]
    fn ctrl_u_k_and_cmd_backspace_kill_then_yank_and_rotate() {
        let mut editor = Editor::new("keep\nfirst second".into());
        editor.cursor = 11;
        editor.key(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "keep\nfirst ");
        editor.key(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "keep\n");
        editor.key(KeyCode::Char('u'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "");
        editor.key(KeyCode::Char('y'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "keep\nfirst ");
        editor.key(KeyCode::Char('y'), KeyModifiers::ALT);
        assert_eq!(editor.text, "second");
        editor.insert("\ntrail");
        editor.key(KeyCode::Backspace, KeyModifiers::SUPER);
        assert_eq!(editor.text, "second\n");
    }

    #[test]
    fn ctrl_underscore_undoes_the_last_edit() {
        let mut editor = Editor::default();
        editor.insert("hello");
        editor.key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        editor.key(KeyCode::Char('_'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "hello");
        editor.key(KeyCode::Char('-'), KeyModifiers::CONTROL);
        assert_eq!(editor.text, "");
    }
}
