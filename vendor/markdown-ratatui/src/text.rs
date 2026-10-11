/// Returns text with terminal control characters removed.
///
/// Line feeds are preserved and tabs expand to four ASCII spaces. This removes
/// characters such as ESC, DEL, and C1 controls, so the function itself never
/// emits an ANSI or OSC sequence. It does not wrap text, escape shell syntax,
/// or normalize Unicode formatting characters.
pub fn sanitize_terminal_text(text: &str) -> String {
    let mut safe = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\t' => safe.push_str("    "),
            '\n' => safe.push(character),
            character if character.is_control() => {}
            character => safe.push(character),
        }
    }
    safe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_are_removed_without_flattening_lines() {
        let safe = sanitize_terminal_text("hi\x1b[2J\x07\u{009b}\x7f\t日本語\nend");
        assert!(
            !safe
                .chars()
                .any(|character| character.is_control() && character != '\n')
        );
        assert!(safe.contains("    日本語\nend"));
    }
}
