//! Key-combo strings such as `ctrl+c` used by the pane `send_keys` API.

use crossterm::event::{KeyCode, KeyModifiers};

pub(crate) type KeyCombo = (KeyCode, KeyModifiers);

fn parse_modifier_token(token: &str) -> Option<KeyModifiers> {
    match token.to_lowercase().as_str() {
        "ctrl" | "control" => Some(KeyModifiers::CONTROL),
        "shift" => Some(KeyModifiers::SHIFT),
        "alt" | "option" | "meta" => Some(KeyModifiers::ALT),
        "cmd" | "command" | "super" => Some(KeyModifiers::SUPER),
        "hyper" => Some(KeyModifiers::HYPER),
        _ => None,
    }
}

pub(crate) fn parse_key_combo(s: &str) -> Option<KeyCombo> {
    let parts: Vec<&str> = s.split('+').collect();
    let mut modifiers = KeyModifiers::empty();
    let mut key_str: Option<&str> = None;

    for part in &parts {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return None;
        }
        if let Some(modifier) = parse_modifier_token(trimmed) {
            modifiers |= modifier;
        } else if key_str.is_some() {
            return None;
        } else {
            key_str = Some(trimmed);
        }
    }

    let key_str = key_str?;
    let single_char = single_key_char(key_str);
    let lower = key_str.to_lowercase();
    let code = match lower.as_str() {
        "space" | " " => KeyCode::Char(' '),
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" if modifiers.contains(KeyModifiers::SHIFT) => {
            modifiers.remove(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        "tab" => KeyCode::Tab,
        "backspace" | "bs" => KeyCode::Backspace,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "minus" => KeyCode::Char('-'),
        "comma" => KeyCode::Char(','),
        "period" => KeyCode::Char('.'),
        "slash" => KeyCode::Char('/'),
        "backslash" => KeyCode::Char('\\'),
        "quote" => KeyCode::Char('\''),
        "double_quote" | "double-quote" => KeyCode::Char('"'),
        "semicolon" => KeyCode::Char(';'),
        "colon" => KeyCode::Char(':'),
        "percent" => KeyCode::Char('%'),
        "ampersand" => KeyCode::Char('&'),
        "backtick" => KeyCode::Char('`'),
        "plus" => KeyCode::Char('+'),
        _ if single_char.is_some() => {
            let ch = single_char?;
            if ch.is_ascii_uppercase() {
                modifiers |= KeyModifiers::SHIFT;
                KeyCode::Char(ch.to_ascii_lowercase())
            } else {
                KeyCode::Char(ch)
            }
        }
        s if s.starts_with('f') => s[1..].parse::<u8>().ok().map(KeyCode::F)?,
        _ => return None,
    };

    Some(normalize_key_combo((code, modifiers)))
}

fn single_key_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let ch = chars.next()?;
    if chars.next().is_none() {
        Some(ch)
    } else {
        None
    }
}

fn normalize_key_combo((mut code, mut modifiers): KeyCombo) -> KeyCombo {
    if matches!(code, KeyCode::Tab) && modifiers.contains(KeyModifiers::SHIFT) {
        code = KeyCode::BackTab;
        modifiers.remove(KeyModifiers::SHIFT);
    } else if matches!(code, KeyCode::BackTab) {
        modifiers.remove(KeyModifiers::SHIFT);
    }
    (code, modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_char_combo() {
        assert_eq!(
            parse_key_combo("v"),
            Some((KeyCode::Char('v'), KeyModifiers::empty()))
        );
    }

    #[test]
    fn parse_unicode_char_combo() {
        assert_eq!(
            parse_key_combo("ö"),
            Some((KeyCode::Char('ö'), KeyModifiers::empty()))
        );
        assert_eq!(
            parse_key_combo("alt+é"),
            Some((KeyCode::Char('é'), KeyModifiers::ALT))
        );
    }

    #[test]
    fn parse_shift_tab_as_backtab() {
        assert_eq!(
            parse_key_combo("shift+tab"),
            Some((KeyCode::BackTab, KeyModifiers::empty()))
        );
    }

    #[test]
    fn parse_named_punctuation() {
        assert_eq!(
            parse_key_combo("minus"),
            Some((KeyCode::Char('-'), KeyModifiers::empty()))
        );
        assert_eq!(
            parse_key_combo("comma"),
            Some((KeyCode::Char(','), KeyModifiers::empty()))
        );
        assert_eq!(
            parse_key_combo("ampersand"),
            Some((KeyCode::Char('&'), KeyModifiers::empty()))
        );
    }
}
