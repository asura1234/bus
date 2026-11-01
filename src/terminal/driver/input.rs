//! One key and text encoder for terminal panes and the Bus UI.
use crate::protocol::keys::{encode_terminal_key, KeyboardProtocol, TerminalKey};

/// Encodes one chord such as `ctrl+shift+r`, `enter`, `esc` or `f6` for the
/// keyboard protocol the client negotiated. With the kitty flags Bus pushes,
/// Esc becomes `CSI 27 u`, so it never waits out the lone-Esc timeout.
pub(crate) fn encode_key(name: &str, protocol: KeyboardProtocol) -> Result<Vec<u8>, String> {
    let normalized = match name.trim() {
        "+" => "plus",
        "C-c" | "c-c" => "ctrl+c",
        other => other,
    };
    let (code, modifiers) = parse_chord(normalized)
        .ok_or_else(|| format!("unknown key {name:?}; examples: enter, esc, ctrl+n, alt+up, f6"))?;
    // A lone ESC byte waits out Bus's escape timeout (up to 150 ms while mouse
    // capture is on); `CSI 27 u` is unambiguous once kitty keys are on.
    if code == crossterm::event::KeyCode::Esc
        && modifiers.is_empty()
        && matches!(protocol, KeyboardProtocol::Kitty { .. })
    {
        return Ok(b"\x1b[27u".to_vec());
    }
    let bytes = encode_terminal_key(TerminalKey::new(code, modifiers), protocol);
    if bytes.is_empty() {
        return Err(format!(
            "key {name:?} encodes to nothing in this keyboard mode"
        ));
    }
    Ok(bytes)
}

/// Pane callers preserve terminal modes (application cursor keys and Kitty)
/// by using the live emulator's encoder after the same chord parsing.
pub(crate) fn encode_key_with(
    name: &str,
    encode: impl FnOnce(TerminalKey) -> Vec<u8>,
) -> Result<Vec<u8>, String> {
    let normalized = match name.trim() {
        "+" => "plus",
        "C-c" | "c-c" => "ctrl+c",
        other => other,
    };
    let (code, modifiers) = parse_chord(normalized)
        .ok_or_else(|| format!("unknown key {name:?}; examples: enter, esc, ctrl+n, alt+up, f6"))?;
    let bytes = encode(TerminalKey::new(code, modifiers));
    if bytes.is_empty() {
        return Err(format!(
            "key {name:?} encodes to nothing in this keyboard mode"
        ));
    }
    Ok(bytes)
}

/// Bus's key-combo syntax, plus the navigation keys Bus binds but its
/// config syntax has no names for.
pub(crate) fn parse_chord(
    chord: &str,
) -> Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)> {
    use crossterm::event::KeyCode;
    let (mods, key) = match chord.rsplit_once('+') {
        Some((mods, key)) if !key.is_empty() => (Some(mods), key),
        _ => (None, chord),
    };
    let code = match key.trim().to_ascii_lowercase().as_str() {
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        _ => return crate::utils::config::parse_key_combo(chord),
    };
    let modifiers = match mods {
        // Reuse Bus's modifier parsing on a stand-in key.
        Some(mods) => crate::utils::config::parse_key_combo(&format!("{mods}+a"))?.1,
        None => crossterm::event::KeyModifiers::empty(),
    };
    Some((code, modifiers))
}

/// Wraps text in bracketed paste when the client enabled it.
pub(crate) fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}
