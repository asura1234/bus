use std::fmt::Write as _;

use crossterm::event::{KeyCode, KeyModifiers};

use super::key::KITTY_FLAG_REPORT_ALL_KEYS;
use super::{KeyboardProtocol, TerminalKey};

const KITTY_FLAG_REPORT_EVENT_TYPES: u16 = 0b0000_0010;
const KITTY_FLAG_REPORT_ALTERNATE_KEYS: u16 = 0b0000_0100;
const KITTY_FLAG_REPORT_ASSOCIATED_TEXT: u16 = 0b0001_0000;

pub fn encode_terminal_key(key: TerminalKey, protocol: KeyboardProtocol) -> Vec<u8> {
    // A zero Unicode value on this Windows character event means the host layout is
    // still composing a dead key. Kitty panes must not receive its physical fallback.
    // Legacy Windows panes take the native ConPTY fallback before reaching this encoder.
    if matches!(protocol, KeyboardProtocol::Kitty { .. }) && key.is_windows_shift_dead_key() {
        return Vec::new();
    }

    // REPORT_ALL_KEYS must retain physical press/repeat/release semantics instead of
    // reducing a native key to its layout-generated text.
    let preserve_physical_key = key.has_physical_identity() && protocol.reports_all_keys();
    if !preserve_physical_key && key.kind != crossterm::event::KeyEventKind::Release {
        if let Some(text) = &key.generated_text {
            return text.as_bytes().to_vec();
        }
    }

    // A release event only produces bytes when the pane protocol reports event
    // types (Kitty REPORT_EVENT_TYPES). Otherwise the child expects a single
    // legacy byte per keystroke, so re-emitting it on release would double keys
    // like Enter/Backspace. The Ghostty wrapper can route release events through
    // this fallback, so guard the fallback encoder too.
    if key.kind == crossterm::event::KeyEventKind::Release && !protocol.reports_event_types() {
        return Vec::new();
    }

    let kitty_first = protocol.reports_all_keys()
        || (key.kind == crossterm::event::KeyEventKind::Release && protocol.reports_event_types());

    if kitty_first {
        if let KeyboardProtocol::Kitty { flags } = protocol {
            if let Some(bytes) = try_encode_csi_u(&key, flags) {
                return bytes;
            }
        }
    }

    if let Some(bytes) = encode_text_input(&key) {
        return bytes;
    }

    if !kitty_first {
        if let KeyboardProtocol::Kitty { flags } = protocol {
            if let Some(bytes) = try_encode_csi_u(&key, flags) {
                return bytes;
            }
        }
    }
    if key.kind == crossterm::event::KeyEventKind::Release && protocol.reports_event_types() {
        return Vec::new();
    }
    encode_legacy(key)
}

/// CSI u encoding: \e[{codepoint};{modifiers}u
/// Used when the child has pushed Kitty keyboard enhancement.
/// Returns None if the key doesn't need CSI u (unmodified basic keys).
fn try_encode_csi_u(key: &TerminalKey, flags: u16) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let event_suffix = kitty_event_suffix(key, flags);
    let report_all_keys = flags & KITTY_FLAG_REPORT_ALL_KEYS != 0;

    if !report_all_keys
        && key.modifiers.is_empty()
        && matches!(key.code, KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace)
    {
        return None;
    }

    // Unmodified keys use legacy encoding (more compatible)
    if mods.is_empty() && event_suffix.is_none() && !report_all_keys {
        return None;
    }

    // Special keys (arrows, F-keys, etc.) have well-established legacy
    // xterm modified formats (\x1b[1;3A for Alt+Up, etc.) that are universally
    // understood. Even Ghostty sends these in legacy format with kitty mode on.
    // Only use CSI u for character keys and keys without legacy representations.
    match key.code {
        KeyCode::Up
        | KeyCode::Down
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Insert
        | KeyCode::Delete
        | KeyCode::F(_)
            if event_suffix.is_none() && !report_all_keys =>
        {
            return None; // let legacy handle these
        }
        _ => {}
    }

    // Kitty keeps the legacy letter/tilde finals for these keys and only adds
    // modifiers and the event type; codepoints 57417+ name the keypad variants.
    if let Some((number, final_char)) = kitty_functional_key(key.code) {
        let modifier = kitty_modifier(mods);
        let mut sequence = format!("\x1b[{number};{modifier}");
        if let Some(event) = event_suffix {
            write!(&mut sequence, ":{event}").ok()?;
        }
        sequence.push(final_char);
        return Some(sequence.into_bytes());
    }

    let (codepoint, alternate_shifted) = match key.code {
        KeyCode::Char(c) => {
            let base = canonical_kitty_char(c, mods);
            let shifted = alternate_shifted_codepoint(key, flags);
            (base as u32, shifted)
        }
        KeyCode::Enter => (13, None),
        KeyCode::Tab => (9, None),
        KeyCode::Backspace => (127, None),
        KeyCode::Esc => (27, None),
        _ => return None, // fall back to legacy for unhandled keys
    };

    let modifier = kitty_modifier(mods);

    let mut sequence = String::with_capacity(32);
    sequence.push_str("\x1b[");
    write!(&mut sequence, "{codepoint}").ok()?;
    if let Some(shifted) = alternate_shifted {
        write!(&mut sequence, ":{shifted}").ok()?;
    }
    write!(&mut sequence, ";{modifier}").ok()?;
    if let Some(event) = event_suffix {
        write!(&mut sequence, ":{event}").ok()?;
    }
    if flags & KITTY_FLAG_REPORT_ASSOCIATED_TEXT != 0 {
        if let Some(text) = text_codepoint_for_key(key) {
            write!(&mut sequence, ";{text}").ok()?;
        }
    }
    sequence.push('u');

    Some(sequence.into_bytes())
}

fn text_codepoint_for_key(key: &TerminalKey) -> Option<u32> {
    let ch = text_char_for_key(key)?;
    (!ch.is_control()).then_some(ch as u32)
}

/// Legacy terminal encoding (standard escape sequences).
fn encode_legacy(key: TerminalKey) -> Vec<u8> {
    let mods = key.modifiers;

    // Modified special keys (arrows, home, end, etc.) use xterm format:
    //   \x1b[1;{modifier}A  for arrows/home/end
    //   \x1b[{n};{modifier}~ for insert/delete/pgup/pgdn
    // The ESC-prefix hack doesn't work for these since they're already escape sequences.
    if !mods.is_empty() {
        if let Some(bytes) = encode_modified_special(key.code, mods) {
            return bytes;
        }
    }

    // Alt modifier on character keys: prefix with ESC
    if mods.contains(KeyModifiers::ALT) {
        let inner = key.with_modifiers(mods.difference(KeyModifiers::ALT));
        let mut bytes = vec![0x1b];
        bytes.extend(encode_legacy_inner(inner));
        return bytes;
    }
    encode_legacy_inner(key)
}

/// xterm-style encoding for modified special keys.
/// Modifier value: 1 + (shift?1:0) + (alt?2:0) + (ctrl?4:0)
fn encode_modified_special(code: KeyCode, mods: KeyModifiers) -> Option<Vec<u8>> {
    let modifier = xterm_modifier(mods);
    if modifier <= 1 {
        return None; // no modifiers to encode
    }

    let (number, final_char) = xterm_functional_key(code)?;
    Some(format!("\x1b[{number};{modifier}{final_char}").into_bytes())
}

/// Kitty functional keys: the xterm table, except F3 is `CSI 13 ~` because
/// `CSI 1;{mod}R` collides with a cursor position report.
fn kitty_functional_key(code: KeyCode) -> Option<(u8, char)> {
    match code {
        KeyCode::F(3) => Some((13, '~')),
        _ => xterm_functional_key(code),
    }
}

/// xterm `CSI {number};{mod}{final}` parameters for modifiable special keys.
fn xterm_functional_key(code: KeyCode) -> Option<(u8, char)> {
    match code {
        // CSI 1;{mod}{letter} format
        KeyCode::Up => Some((1, 'A')),
        KeyCode::Down => Some((1, 'B')),
        KeyCode::Right => Some((1, 'C')),
        KeyCode::Left => Some((1, 'D')),
        KeyCode::Home => Some((1, 'H')),
        KeyCode::End => Some((1, 'F')),
        // CSI {n};{mod}~ format
        KeyCode::Insert => Some((2, '~')),
        KeyCode::Delete => Some((3, '~')),
        KeyCode::PageUp => Some((5, '~')),
        KeyCode::PageDown => Some((6, '~')),
        // F1-F4: CSI 1;{mod}{P-S}
        KeyCode::F(1) => Some((1, 'P')),
        KeyCode::F(2) => Some((1, 'Q')),
        KeyCode::F(3) => Some((1, 'R')),
        KeyCode::F(4) => Some((1, 'S')),
        // F5-F12: CSI {n};{mod}~
        KeyCode::F(5) => Some((15, '~')),
        KeyCode::F(6) => Some((17, '~')),
        KeyCode::F(7) => Some((18, '~')),
        KeyCode::F(8) => Some((19, '~')),
        KeyCode::F(9) => Some((20, '~')),
        KeyCode::F(10) => Some((21, '~')),
        KeyCode::F(11) => Some((23, '~')),
        KeyCode::F(12) => Some((24, '~')),
        _ => None,
    }
}

/// xterm modifier encoding: 1 + shift(1) + alt(2) + ctrl(4)
/// Used for legacy modified special keys (arrows, function keys, etc.)
fn xterm_modifier(mods: KeyModifiers) -> u32 {
    let mut m = 1u32;
    if mods.contains(KeyModifiers::SHIFT) {
        m += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        m += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        m += 4;
    }
    m
}

/// Kitty protocol modifier encoding: 1 + shift(1) + alt(2) + ctrl(4) + super(8) + hyper(16) + meta(32)
/// Superset of xterm — adds Super/Hyper/Meta bits.
fn kitty_modifier(mods: KeyModifiers) -> u32 {
    let mut m = xterm_modifier(mods);
    if mods.contains(KeyModifiers::SUPER) {
        m += 8;
    }
    if mods.contains(KeyModifiers::HYPER) {
        m += 16;
    }
    if mods.contains(KeyModifiers::META) {
        m += 32;
    }
    m
}

fn encode_text_input(key: &TerminalKey) -> Option<Vec<u8>> {
    let ch = text_char_for_key(key)?;
    let mut buf = [0u8; 4];
    Some(ch.encode_utf8(&mut buf).as_bytes().to_vec())
}

fn text_char_for_key(key: &TerminalKey) -> Option<char> {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return None;
    }

    let KeyCode::Char(ch) = key.code else {
        return None;
    };

    if key.modifiers.is_empty() {
        return Some(ch);
    }
    if key.modifiers == KeyModifiers::SHIFT {
        return shifted_text_char(key, ch);
    }
    None
}

fn shifted_text_char(key: &TerminalKey, ch: char) -> Option<char> {
    if let Some(shifted) = key.shifted_codepoint.and_then(char::from_u32) {
        return Some(shifted);
    }

    if ch.is_ascii_uppercase() {
        return Some(ch);
    }

    if ch.is_ascii_lowercase() {
        return Some(ch.to_ascii_uppercase());
    }

    if is_shifted_ascii_punctuation(ch) {
        return Some(ch);
    }

    None
}

fn is_shifted_ascii_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '!' | '@'
            | '#'
            | '$'
            | '%'
            | '^'
            | '&'
            | '*'
            | '('
            | ')'
            | '_'
            | '+'
            | '{'
            | '}'
            | '|'
            | ':'
            | '"'
            | '<'
            | '>'
            | '?'
            | '~'
    )
}

fn canonical_kitty_char(ch: char, mods: KeyModifiers) -> char {
    if mods.contains(KeyModifiers::SHIFT) && ch.is_ascii_uppercase() {
        ch.to_ascii_lowercase()
    } else {
        ch
    }
}

fn alternate_shifted_codepoint(key: &TerminalKey, flags: u16) -> Option<u32> {
    if flags & KITTY_FLAG_REPORT_ALTERNATE_KEYS == 0 {
        return None;
    }

    if let Some(shifted) = key.shifted_codepoint {
        return Some(shifted);
    }

    match key.code {
        KeyCode::Char(ch)
            if key.modifiers.contains(KeyModifiers::SHIFT) && ch.is_ascii_uppercase() =>
        {
            Some(ch as u32)
        }
        _ => None,
    }
}

fn kitty_event_suffix(key: &TerminalKey, flags: u16) -> Option<u8> {
    if flags & KITTY_FLAG_REPORT_EVENT_TYPES == 0 {
        return None;
    }

    Some(match key.kind {
        crossterm::event::KeyEventKind::Press => 1,
        crossterm::event::KeyEventKind::Repeat => 2,
        crossterm::event::KeyEventKind::Release => 3,
    })
}

fn encode_legacy_inner(key: TerminalKey) -> Vec<u8> {
    match key.code {
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let upper = ch.to_ascii_uppercase();
                match upper {
                    'A'..='Z' => vec![upper as u8 - 64],
                    ' ' | '@' | '2' => vec![0],
                    '[' | '3' => vec![27],
                    '\\' | '4' => vec![28],
                    ']' | '5' => vec![29],
                    '^' | '6' => vec![30],
                    '_' | '/' | '7' | '-' => vec![31],
                    _ => ch.to_string().into_bytes(),
                }
            } else {
                let ch = if key.modifiers == KeyModifiers::SHIFT {
                    shifted_text_char(&key, ch).unwrap_or(ch)
                } else {
                    ch
                };
                let mut buf = [0u8; 4];
                ch.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![127],
        KeyCode::Tab => vec![9],
        KeyCode::BackTab => vec![27, 91, 90],
        KeyCode::Esc => vec![27],
        KeyCode::Left => vec![27, 91, 68],
        KeyCode::Right => vec![27, 91, 67],
        KeyCode::Up => vec![27, 91, 65],
        KeyCode::Down => vec![27, 91, 66],
        KeyCode::Home => vec![27, 91, 72],
        KeyCode::End => vec![27, 91, 70],
        KeyCode::PageUp => vec![27, 91, 53, 126],
        KeyCode::PageDown => vec![27, 91, 54, 126],
        KeyCode::Delete => vec![27, 91, 51, 126],
        KeyCode::Insert => vec![27, 91, 50, 126],
        KeyCode::F(n) => encode_f_key(n),
        _ => vec![],
    }
}

fn encode_f_key(n: u8) -> Vec<u8> {
    match n {
        1 => vec![27, 79, 80],
        2 => vec![27, 79, 81],
        3 => vec![27, 79, 82],
        4 => vec![27, 79, 83],
        5 => vec![27, 91, 49, 53, 126],
        6 => vec![27, 91, 49, 55, 126],
        7 => vec![27, 91, 49, 56, 126],
        8 => vec![27, 91, 49, 57, 126],
        9 => vec![27, 91, 50, 48, 126],
        10 => vec![27, 91, 50, 49, 126],
        11 => vec![27, 91, 50, 51, 126],
        12 => vec![27, 91, 50, 52, 126],
        _ => vec![],
    }
}

/// Encode native or targeted semantic Win32 input for a compatible ConPTY destination.
#[cfg(windows)]
pub(crate) fn encode_windows_conpty_fallback(key: &TerminalKey) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

    let (virtual_key_code, virtual_scan_code, unicode, control_key_state) =
        if let Some(record) = key.windows_record() {
            (
                record.virtual_key_code,
                record.virtual_scan_code,
                record.unicode,
                record.control_key_state,
            )
        } else if key.code == KeyCode::Esc
            && key.modifiers.is_empty()
            && key.kind == KeyEventKind::Press
            && key.vt_bytes().is_none()
        {
            return Some(b"\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_".to_vec());
        } else if key.code == KeyCode::Enter && key.modifiers == KeyModifiers::SHIFT {
            (13, 28, 13, 16)
        } else {
            return None;
        };
    let key_down = key.kind != KeyEventKind::Release;
    let repeat_count = if key_down { key.repeat_count.max(1) } else { 1 };

    Some(
        format!(
            "\x1b[{virtual_key_code};{virtual_scan_code};{unicode};{};{control_key_state};{repeat_count}_",
            u8::from(key_down),
        )
        .into_bytes(),
    )
}

#[cfg(test)]
#[path = "tests/encode_test.rs"]
mod tests;
