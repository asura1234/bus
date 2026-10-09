use super::framer::{MAX_DISCARDED_CONTROL_TAIL_BYTES, MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES};
use super::sequence::{control_string_terminator_for_family, ControlStringFamily};
use super::ESC;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub(super) fn starts_with_incomplete_sgr_mouse_sequence(buffer: &[u8]) -> bool {
    buffer.starts_with(b"\x1b[<")
        && buffer[3..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

#[cfg(unix)]
pub(super) fn starts_with_incomplete_default_mouse_sequence(buffer: &[u8]) -> bool {
    buffer.starts_with(b"\x1b[M") && buffer.len() < 6
}

pub(super) fn starts_with_incomplete_orphaned_sgr_mouse_tail(buffer: &[u8]) -> bool {
    if buffer.len() > MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES {
        return false;
    }
    buffer.len() < 3 && b"[<".starts_with(buffer)
        || buffer.starts_with(b"[<")
            && buffer[2..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b';')
}

pub(super) fn discard_complete_orphaned_sgr_mouse_tail(buffer: &mut Vec<u8>) -> bool {
    let Some(terminator_len) =
        control_string_terminator_for_family(buffer, ControlStringFamily::OrphanedSgrMouseTail)
    else {
        return false;
    };
    if terminator_len > MAX_ORPHANED_SGR_MOUSE_TAIL_BYTES {
        return false;
    }
    let mut sequence = Vec::with_capacity(terminator_len + 1);
    sequence.push(ESC);
    sequence.extend_from_slice(&buffer[..terminator_len]);
    let Ok(sequence) = std::str::from_utf8(&sequence) else {
        return false;
    };
    if parse_sgr_mouse(sequence).is_none() {
        return false;
    }
    buffer.drain(..terminator_len);
    true
}

pub(super) fn discard_or_buffer_orphaned_sgr_mouse_tail(
    buffer: &mut Vec<u8>,
    discard_until: &mut Option<ControlStringFamily>,
    discarded_tail_bytes: &mut usize,
) {
    if !discard_complete_orphaned_sgr_mouse_tail(buffer) {
        *discarded_tail_bytes = buffer.len();
        *discard_until = (*discarded_tail_bytes <= MAX_DISCARDED_CONTROL_TAIL_BYTES)
            .then_some(ControlStringFamily::OrphanedSgrMouseTail);
        buffer.clear();
    }
}

pub(super) fn discard_orphaned_sgr_mouse_tail(
    buffer: &mut Vec<u8>,
    discarded_tail_bytes: &mut usize,
) -> bool {
    let remaining = MAX_DISCARDED_CONTROL_TAIL_BYTES.saturating_sub(*discarded_tail_bytes);
    let inspected = buffer.len().min(remaining);

    for index in 0..inspected {
        match buffer[index] {
            b'0'..=b'9' | b';' => {}
            b'M' | b'm' => {
                buffer.drain(..=index);
                return true;
            }
            _ => {
                buffer.drain(..index);
                return true;
            }
        }
    }

    *discarded_tail_bytes = discarded_tail_bytes.saturating_add(inspected);
    if buffer.len() > inspected {
        buffer.clear();
        return true;
    }

    buffer.clear();
    false
}

pub(super) fn parse_default_mouse(sequence: &[u8]) -> Option<MouseEvent> {
    let &[ESC, b'[', b'M', encoded_cb, encoded_column, encoded_row] = sequence else {
        return None;
    };
    let cb = encoded_cb.checked_sub(32)?;
    let column = u16::from(encoded_column).checked_sub(33)?;
    let row = u16::from(encoded_row).checked_sub(33)?;
    let (kind, modifiers) = parse_mouse_cb(cb)?;

    Some(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

pub(super) fn parse_sgr_mouse(sequence: &str) -> Option<MouseEvent> {
    let body = sequence.strip_prefix("\x1b[<")?;
    let final_char = body.chars().last()?;
    if final_char != 'M' && final_char != 'm' {
        return None;
    }

    let payload = &body[..body.len() - 1];
    let mut parts = payload.split(';');
    let cb = parts.next()?.parse::<u8>().ok()?;
    let column = parts.next()?.parse::<u16>().ok()?.checked_sub(1)?;
    let row = parts.next()?.parse::<u16>().ok()?.checked_sub(1)?;
    let (kind, modifiers) = parse_mouse_cb(cb)?;

    let kind = if final_char == 'm' {
        match kind {
            MouseEventKind::Down(button) => MouseEventKind::Up(button),
            other => other,
        }
    } else {
        kind
    };

    Some(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    })
}

pub(super) fn parse_mouse_cb(cb: u8) -> Option<(MouseEventKind, KeyModifiers)> {
    let button_number = (cb & 0b0000_0011) | ((cb & 0b1100_0000) >> 4);
    let dragging = cb & 0b0010_0000 == 0b0010_0000;

    let kind = match (button_number, dragging) {
        (0, false) => MouseEventKind::Down(MouseButton::Left),
        (1, false) => MouseEventKind::Down(MouseButton::Middle),
        (2, false) => MouseEventKind::Down(MouseButton::Right),
        (0, true) => MouseEventKind::Drag(MouseButton::Left),
        (1, true) => MouseEventKind::Drag(MouseButton::Middle),
        (2, true) => MouseEventKind::Drag(MouseButton::Right),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        // Crossterm cannot represent extended-button drags. Preserve their
        // position as motion so a stuck host button cannot suppress hover.
        (3, true) | (4, true) | (5, true) | (8, true) | (9, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return None,
    };

    let mut modifiers = KeyModifiers::empty();
    if cb & 0b0000_0100 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 0b0000_1000 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 0b0001_0000 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }

    Some((kind, modifiers))
}
