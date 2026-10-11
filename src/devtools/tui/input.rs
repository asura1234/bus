//! Bytes the driver writes to the Bus client's PTY: Bus's own key encoder for
//! chords, and SGR 1006 mouse reports, the format the client enables.
use crate::protocol::keys::{encode_terminal_key, KeyboardProtocol, TerminalKey};

/// Encodes one chord such as `ctrl+shift+r`, `enter`, `esc` or `f6` for the
/// keyboard protocol the client negotiated. With the kitty flags Bus pushes,
/// Esc becomes `CSI 27 u`, so it never waits out the lone-Esc timeout.
pub(super) fn encode_key(name: &str, protocol: KeyboardProtocol) -> Result<Vec<u8>, String> {
    let normalized = match name.trim() {
        "+" => "plus",
        other => other,
    };
    let (code, modifiers) = crate::utils::config::parse_key_combo(normalized)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "left" => Ok(Self::Left),
            "middle" => Ok(Self::Middle),
            "right" => Ok(Self::Right),
            _ => Err(format!(
                "unknown button {name:?}; use left, middle or right"
            )),
        }
    }

    fn code(self) -> u16 {
        match self {
            Self::Left => 0,
            Self::Middle => 1,
            Self::Right => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    pub fn parse(list: &str) -> Result<Self, String> {
        let mut mods = Self::default();
        for part in list.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part {
                "shift" => mods.shift = true,
                "alt" | "option" => mods.alt = true,
                "ctrl" | "control" => mods.ctrl = true,
                other => return Err(format!("unknown modifier {other:?}; use shift, alt, ctrl")),
            }
        }
        Ok(mods)
    }

    fn bits(self) -> u16 {
        u16::from(self.shift) * 4 + u16::from(self.alt) * 8 + u16::from(self.ctrl) * 16
    }
}

/// One SGR report at a zero-based cell; `press` false means release.
fn sgr(code: u16, row: u16, col: u16, press: bool) -> Vec<u8> {
    format!(
        "\x1b[<{code};{};{}{}",
        col + 1,
        row + 1,
        if press { 'M' } else { 'm' }
    )
    .into_bytes()
}

pub(super) fn click(row: u16, col: u16, button: Button, mods: Mods) -> Vec<u8> {
    let code = button.code() + mods.bits();
    let mut bytes = sgr(code, row, col, true);
    bytes.extend(sgr(code, row, col, false));
    bytes
}

/// Press at `from`, move through a few cells, release at `to`.
pub(super) fn drag(from: (u16, u16), to: (u16, u16), button: Button, mods: Mods) -> Vec<Vec<u8>> {
    let code = button.code() + mods.bits();
    let mut steps = vec![sgr(code, from.0, from.1, true)];
    let distance = from.0.abs_diff(to.0).max(from.1.abs_diff(to.1));
    let count = distance.clamp(1, 8);
    for step in 1..=count {
        let lerp = |a: u16, b: u16| -> u16 {
            let (a, b) = (i32::from(a), i32::from(b));
            (a + (b - a) * i32::from(step) / i32::from(count)) as u16
        };
        steps.push(sgr(code + 32, lerp(from.0, to.0), lerp(from.1, to.1), true));
    }
    steps.push(sgr(code, to.0, to.1, false));
    steps
}

/// Wheel notches at a cell; Bus reads one report per notch.
pub(super) fn scroll(row: u16, col: u16, up: bool, notches: u16, mods: Mods) -> Vec<u8> {
    let code = if up { 64 } else { 65 } + mods.bits();
    (0..notches)
        .flat_map(|_| sgr(code, row, col, true))
        .collect()
}

/// Wraps text in bracketed paste when the client enabled it.
pub(super) fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

#[cfg(test)]
#[path = "tests/input_test.rs"]
mod tests;
