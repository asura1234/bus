/// The caller supplies its terminal's width rules; whitespace still advances a cell.
pub(crate) fn first_non_blank_col(text: &str, width: impl Fn(char) -> u16) -> Option<u16> {
    let mut col = 0u16;
    for ch in text.chars() {
        if !ch.is_whitespace() {
            return Some(col);
        }
        col = col.saturating_add(width(ch).max(1));
    }
    None
}

pub(crate) fn last_character_col(text: &str, width: impl Fn(char) -> u16) -> Option<u16> {
    let mut col = 0u16;
    let mut last_col = None;
    for ch in text.chars() {
        let width = width(ch);
        if width > 0 {
            last_col = Some(col);
            col = col.saturating_add(width);
        }
    }
    last_col
}

#[cfg(test)]
mod tests {
    use super::*;

    fn width(ch: char) -> u16 {
        match ch {
            '界' => 2,
            '\u{301}' => 0,
            _ => 1,
        }
    }

    #[test]
    fn copy_motion_uses_supplied_width_and_ignores_trailing_zero_width_marks() {
        assert_eq!(first_non_blank_col("  界\u{301}", width), Some(2));
        assert_eq!(last_character_col("  界\u{301}", width), Some(2));
        assert_eq!(last_character_col("界x\u{301}", width), Some(2));
        assert_eq!(last_character_col("\u{301}", width), None);
        assert_eq!(last_character_col("", width), None);
    }

    #[test]
    fn first_non_blank_keeps_minimum_whitespace_advance_and_saturates() {
        assert_eq!(first_non_blank_col("  x", |_| 0), Some(2));
        assert_eq!(first_non_blank_col("   ", width), None);
        assert_eq!(first_non_blank_col("  x", |_| u16::MAX), Some(u16::MAX));
        assert_eq!(last_character_col("abc", |_| u16::MAX), Some(u16::MAX));
    }
}
