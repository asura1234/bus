use crossterm::event::KeyCode;

#[derive(Clone, Debug, Default)]
pub(super) struct Editor {
    pub text: String,
    pub cursor: usize,
}

impl Editor {
    pub fn new(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }
    pub fn insert(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
    }
    pub fn key(&mut self, key: KeyCode) {
        let previous = self.text[..self.cursor]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
        let next = self.text[self.cursor..]
            .chars()
            .next()
            .map_or(self.cursor, |c| self.cursor + c.len_utf8());
        match key {
            KeyCode::Left => self.cursor = previous,
            KeyCode::Right => self.cursor = next,
            KeyCode::Home => {
                self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1)
            }
            KeyCode::End => {
                self.cursor += self.text[self.cursor..]
                    .find('\n')
                    .unwrap_or(self.text.len() - self.cursor)
            }
            KeyCode::Backspace => {
                self.text.replace_range(previous..self.cursor, "");
                self.cursor = previous;
            }
            KeyCode::Delete => {
                self.text.replace_range(self.cursor..next, "");
            }
            KeyCode::Up | KeyCode::Down => {
                let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
                let col = self.text[start..self.cursor].chars().count();
                let target = if key == KeyCode::Up {
                    if start == 0 {
                        return;
                    }
                    self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1)
                } else {
                    let Some(end) = self.text[self.cursor..].find('\n') else {
                        return;
                    };
                    self.cursor + end + 1
                };
                let line = self.text[target..].split('\n').next().unwrap_or_default();
                self.cursor = target + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_unicode_at_cursor_without_destroying_neighboring_text() {
        let mut editor = Editor::default();
        editor.insert("a界c");
        editor.key(KeyCode::Left);
        editor.key(KeyCode::Backspace);
        editor.insert("文");
        assert_eq!(editor.text, "a文c");
        assert_eq!(editor.cursor, 4);
        editor.key(KeyCode::Delete);
        assert_eq!(editor.text, "a文");
    }

    #[test]
    fn multiline_home_end_and_vertical_movement_keep_valid_utf8_boundaries() {
        let mut editor = Editor::default();
        editor.insert("ab\n界x\nlast");
        editor.key(KeyCode::Home);
        editor.key(KeyCode::Up);
        editor.key(KeyCode::Right);
        editor.insert("!");
        assert_eq!(editor.text, "ab\n界!x\nlast");
        editor.key(KeyCode::End);
        editor.key(KeyCode::Delete);
        assert_eq!(editor.text, "ab\n界!xlast");
    }
}
