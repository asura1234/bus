use super::mouse::{parse_default_mouse, parse_sgr_mouse};
use super::ESC;

pub(super) fn plausible_osc_tail(buffer: &[u8]) -> bool {
    buffer.iter().all(|byte| {
        byte.is_ascii_digit()
            || matches!(
                *byte,
                b';' | b':'
                    | b'/'
                    | b'#'
                    | b'?'
                    | b'.'
                    | b'_'
                    | b'-'
                    | b'+'
                    | b'r'
                    | b'g'
                    | b'b'
                    | b'R'
                    | b'G'
                    | b'B'
                    | ESC
            )
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ControlStringFamily {
    Osc,
    StTerminated,
    HostReplyCsi,
    OrphanedSgrMouseTail,
}

pub(super) enum ControlString {
    Complete { len: usize },
    Incomplete { family: ControlStringFamily },
}

pub(super) fn control_string(buffer: &[u8]) -> Option<ControlString> {
    let family = match buffer.get(..2)? {
        b"\x1b]" => ControlStringFamily::Osc,
        b"\x1bP" | b"\x1b_" | b"\x1b^" | b"\x1bX" => ControlStringFamily::StTerminated,
        _ => return None,
    };

    Some(match control_string_terminator_for_family(buffer, family) {
        Some(len) => ControlString::Complete { len },
        None => ControlString::Incomplete { family },
    })
}

pub(super) fn first_complete_utf8_char(buffer: &[u8]) -> Option<&str> {
    let width = utf8_char_width(*buffer.first()?)?;

    if buffer.len() < width {
        return None;
    }

    std::str::from_utf8(&buffer[..width]).ok()
}

pub(super) fn starts_with_incomplete_utf8_char(buffer: &[u8]) -> bool {
    match std::str::from_utf8(buffer) {
        Ok(_) => false,
        Err(err) => err.valid_up_to() == 0 && err.error_len().is_none(),
    }
}

pub(super) fn utf8_char_width(first: u8) -> Option<usize> {
    if first < 0x80 {
        Some(1)
    } else if first & 0b1110_0000 == 0b1100_0000 {
        Some(2)
    } else if first & 0b1111_0000 == 0b1110_0000 {
        Some(3)
    } else if first & 0b1111_1000 == 0b1111_0000 {
        Some(4)
    } else {
        None
    }
}

pub(super) fn complete_escape_sequence_len(buffer: &[u8]) -> Option<usize> {
    if buffer.len() == 1 {
        return None;
    }

    if buffer.starts_with(b"\x1b\x1b[<") {
        if let Some(mouse_len) = find_csi_final(&buffer[1..], b"Mm") {
            let mouse_sequence = std::str::from_utf8(&buffer[1..1 + mouse_len]).ok()?;
            if parse_sgr_mouse(mouse_sequence).is_some() {
                return Some(1);
            }
        }
    }

    if buffer.len() >= 7
        && buffer.starts_with(b"\x1b\x1b[M")
        && parse_default_mouse(&buffer[1..7]).is_some()
    {
        return Some(1);
    }

    if buffer.starts_with(b"\x1b\x1b") {
        return complete_escape_sequence_len(&buffer[1..]).map(|len| len + 1);
    }

    if buffer.starts_with(b"\x1b[") {
        if buffer.starts_with(b"\x1b[<") {
            return find_csi_final(buffer, b"Mm");
        }
        if buffer.starts_with(b"\x1b[M") {
            return (buffer.len() >= 6).then_some(6);
        }
        return find_csi_final(
            buffer,
            b"@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~",
        );
    }

    if let Some(control) = control_string(buffer) {
        return match control {
            ControlString::Complete { len } => Some(len),
            ControlString::Incomplete { .. } => None,
        };
    }

    if buffer.starts_with(b"\x1bO") {
        return (buffer.len() >= 3).then_some(3);
    }

    let escaped_char_width = utf8_char_width(buffer[1])?;
    if buffer.len() < 1 + escaped_char_width {
        return None;
    }
    std::str::from_utf8(&buffer[1..1 + escaped_char_width]).ok()?;
    Some(1 + escaped_char_width)
}

pub(super) fn osc_string_terminator(buffer: &[u8]) -> Option<usize> {
    let st = st_string_terminator(buffer);
    let bel = buffer
        .iter()
        .position(|byte| *byte == b'\x07')
        .map(|idx| idx + 1);

    match (st, bel) {
        (Some(st), Some(bel)) => Some(st.min(bel)),
        (Some(st), None) => Some(st),
        (None, Some(bel)) => Some(bel),
        (None, None) => None,
    }
}

pub(super) fn st_string_terminator(buffer: &[u8]) -> Option<usize> {
    find_subsequence(buffer, b"\x1b\\").map(|idx| idx + 2)
}

pub(super) fn control_string_terminator_for_family(
    buffer: &[u8],
    family: ControlStringFamily,
) -> Option<usize> {
    match family {
        ControlStringFamily::Osc => osc_string_terminator(buffer),
        ControlStringFamily::StTerminated => st_string_terminator(buffer),
        ControlStringFamily::HostReplyCsi => None,
        ControlStringFamily::OrphanedSgrMouseTail => buffer
            .iter()
            .position(|byte| matches!(*byte, b'M' | b'm'))
            .map(|idx| idx + 1),
    }
}

pub(super) fn find_csi_final(buffer: &[u8], finals: &[u8]) -> Option<usize> {
    for (idx, byte) in buffer.iter().enumerate().skip(2) {
        if finals.contains(byte) {
            return Some(idx + 1);
        }
    }
    None
}

pub(super) fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
