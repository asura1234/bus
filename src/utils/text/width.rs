// The native Unicode table is pure and shared by text consumers and VT. This
// narrow ABI declaration does not load the terminal component or its state.
unsafe extern "C" {
    fn ghostty_unicode_codepoint_width(codepoint: u32) -> u8;
}

/// Preserve the native table's codepoint rules, including invalid codepoints.
pub fn unicode_codepoint_width(codepoint: u32) -> u8 {
    // SAFETY: the native function is total for u32, thread-safe, and has no
    // pointer, initialization, handle, or lifetime preconditions.
    unsafe { ghostty_unicode_codepoint_width(codepoint) }
}

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(crate) fn truncate_end(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let prefix = take_prefix_width(text, max_width.saturating_sub(1));
    format!("{prefix}…")
}

fn take_prefix_width(text: &str, max_width: usize) -> String {
    let mut output = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        output.push(ch);
        width += ch_width;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_codepoint_width_preserves_control_combining_wide_and_invalid_rules() {
        for (codepoint, expected) in [
            ('A' as u32, 1),
            ('\u{301}' as u32, 0),
            ('界' as u32, 2),
            (0, 0),
            (0xd800, 0),
            (0x11_0000, 1),
        ] {
            assert_eq!(unicode_codepoint_width(codepoint), expected);
        }
    }

    #[test]
    fn truncate_end_uses_display_width() {
        let text = truncate_end("提交 bus 的反馈", 14);

        assert_eq!(text, "提交 bus 的反…");
        assert!(display_width(&text) <= 14);
    }
}
