//! Modifier, virtual-key, and control/OEM layout lookup.
use crate::protocol::keys::WindowsKeyRecord;

pub(super) fn windows_key_modifiers(control_key_state: u32) -> crossterm::event::KeyModifiers {
    const RIGHT_ALT_PRESSED: u32 = 0x0001;
    const LEFT_ALT_PRESSED: u32 = 0x0002;
    const RIGHT_CTRL_PRESSED: u32 = 0x0004;
    const LEFT_CTRL_PRESSED: u32 = 0x0008;
    const SHIFT_PRESSED: u32 = 0x0010;

    let mut modifiers = crossterm::event::KeyModifiers::empty();
    let alt_gr = control_key_state & RIGHT_ALT_PRESSED != 0
        && control_key_state & LEFT_CTRL_PRESSED != 0
        && control_key_state & RIGHT_CTRL_PRESSED == 0;
    if control_key_state & SHIFT_PRESSED != 0 {
        modifiers |= crossterm::event::KeyModifiers::SHIFT;
    }
    if !alt_gr && control_key_state & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
        modifiers |= crossterm::event::KeyModifiers::CONTROL;
    }
    // Treat right Alt as AltGr rather than a terminal Alt prefix.
    if control_key_state & LEFT_ALT_PRESSED != 0
        || (control_key_state & RIGHT_ALT_PRESSED != 0 && !alt_gr)
    {
        modifiers |= crossterm::event::KeyModifiers::ALT;
    }
    modifiers
}

pub(super) fn windows_virtual_key_to_key_code(
    vk: u16,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<crate::protocol::wire::ClientKeyCode> {
    use crate::protocol::wire::ClientKeyCode;
    Some(match vk {
        0x08 => ClientKeyCode::Backspace,
        0x09 if modifiers.contains(crossterm::event::KeyModifiers::SHIFT) => ClientKeyCode::BackTab,
        0x09 => ClientKeyCode::Tab,
        0x0d => ClientKeyCode::Enter,
        0x1b => ClientKeyCode::Esc,
        0x21 => ClientKeyCode::PageUp,
        0x22 => ClientKeyCode::PageDown,
        0x23 => ClientKeyCode::End,
        0x24 => ClientKeyCode::Home,
        0x25 => ClientKeyCode::Left,
        0x26 => ClientKeyCode::Up,
        0x27 => ClientKeyCode::Right,
        0x28 => ClientKeyCode::Down,
        0x2d => ClientKeyCode::Insert,
        0x2e => ClientKeyCode::Delete,
        0x70..=0x87 => ClientKeyCode::F((vk - 0x6f) as u8),
        _ => return None,
    })
}

pub(super) fn windows_virtual_key_to_char_code(
    vk: u16,
    unicode: u16,
    modifiers: crossterm::event::KeyModifiers,
) -> Option<crate::protocol::wire::ClientKeyCode> {
    use crate::protocol::wire::ClientKeyCode;

    if let Some(ch) = char::from_u32(unicode as u32).filter(|ch| !ch.is_control()) {
        return Some(ClientKeyCode::Char(ch));
    }

    let ch = match vk {
        0x30..=0x39 => char::from_u32(vk as u32)?,
        0x41..=0x5a
            if modifiers.contains(crossterm::event::KeyModifiers::SHIFT)
                && !modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            char::from_u32(vk as u32)?
        }
        0x41..=0x5a => char::from_u32(vk as u32 + 32)?,
        _ => return None,
    };
    Some(ClientKeyCode::Char(ch))
}

pub(super) fn ctrl_key_code(
    vk: u16,
    u: u16,
    oem: Option<char>,
) -> Option<crate::protocol::wire::ClientKeyCode> {
    use crate::protocol::wire::ClientKeyCode;
    Some(match (vk, u) {
        (0xbf, 0x00) => ClientKeyCode::Char(oem?),
        (_, 0x00) => ClientKeyCode::Char(' '),
        (_, 0x1b) => ClientKeyCode::Char('['),
        (_, 0x1c) => ClientKeyCode::Char('\\'),
        (_, 0x1d) => ClientKeyCode::Char(']'),
        (_, 0x1e) => ClientKeyCode::Char('^'),
        (_, 0x1f) => ClientKeyCode::Char('-'),
        _ => return None,
    })
}

pub(super) fn resolve_ctrl_oem_char(key: WindowsKeyRecord) -> Option<char> {
    if key.virtual_key_code == 0xbf
        && key.unicode == 0
        && windows_key_modifiers(key.control_key_state)
            .contains(crossterm::event::KeyModifiers::CONTROL)
    {
        #[cfg(windows)]
        return crate::platform::resolve_base_printable_key(
            key.virtual_key_code,
            key.virtual_scan_code,
        );
    }
    None
}

#[cfg(any(windows, test))]
pub(super) fn windows_input_trace_enabled() -> bool {
    std::env::var_os("HERDR_WINDOWS_INPUT_TRACE").is_some()
}
