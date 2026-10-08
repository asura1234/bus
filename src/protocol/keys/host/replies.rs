use super::framer::MAX_DISCARDED_CONTROL_TAIL_BYTES;
use super::sequence::{control_string, ControlString, ControlStringFamily};
use super::{GHOSTTY_COLOR_SCHEME_DARK_REPORT, GHOSTTY_COLOR_SCHEME_LIGHT_REPORT};
use crate::utils::theme::color::HostAppearance;

pub(super) fn parse_host_color_scheme_report(buffer: &[u8]) -> Option<HostAppearance> {
    match buffer {
        GHOSTTY_COLOR_SCHEME_DARK_REPORT => Some(HostAppearance::Dark),
        GHOSTTY_COLOR_SCHEME_LIGHT_REPORT => Some(HostAppearance::Light),
        _ => None,
    }
}

/// Parses an XTWINOPS cell size report (`CSI 6 ; height ; width t`) into
/// `(width_px, height_px)`; note the reply orders height first.
pub(super) fn parse_host_cell_size_report(buffer: &[u8]) -> Option<(u32, u32)> {
    let body = buffer.strip_prefix(b"\x1b[")?.strip_suffix(b"t")?;
    let text = std::str::from_utf8(body).ok()?;
    let mut params = text.split(';');
    if params.next()? != "6" {
        return None;
    }
    let height_px = params.next()?.parse::<u32>().ok()?;
    let width_px = params.next()?.parse::<u32>().ok()?;
    if params.next().is_some() || width_px == 0 || height_px == 0 {
        return None;
    }
    Some((width_px, height_px))
}

pub(super) fn starts_with_incomplete_default_color_response(buffer: &[u8]) -> bool {
    matches!(
        control_string(buffer),
        Some(ControlString::Incomplete {
            family: ControlStringFamily::Osc
        })
    ) && matches!(buffer.get(..5), Some(b"\x1b]10;" | b"\x1b]11;"))
}

pub(super) fn starts_with_incomplete_host_color_scheme_report(buffer: &[u8]) -> bool {
    buffer.starts_with(b"\x1b[?")
        && (GHOSTTY_COLOR_SCHEME_DARK_REPORT.starts_with(buffer)
            || GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.starts_with(buffer))
        && buffer.len() < GHOSTTY_COLOR_SCHEME_DARK_REPORT.len()
}

pub(super) fn starts_with_incomplete_host_cell_size_report(buffer: &[u8]) -> bool {
    let Some(body) = buffer.strip_prefix(b"\x1b[") else {
        return false;
    };
    if body.is_empty() || body.last() == Some(&b't') {
        return false;
    }

    let mut params = body.split(|byte| *byte == b';');
    if params.next() != Some(b"6".as_slice()) {
        return false;
    }
    let height = params.next();
    let width = params.next();
    params.next().is_none()
        && height.is_none_or(|value| value.iter().all(u8::is_ascii_digit))
        && width.is_none_or(|value| value.iter().all(u8::is_ascii_digit))
        && !(height.is_some_and(<[u8]>::is_empty) && width.is_some())
}

pub(super) fn discard_host_reply_csi_tail(
    buffer: &mut Vec<u8>,
    discarded_tail_bytes: &mut usize,
) -> bool {
    let remaining = MAX_DISCARDED_CONTROL_TAIL_BYTES.saturating_sub(*discarded_tail_bytes);
    let inspected = buffer.len().min(remaining);

    for index in 0..inspected {
        match buffer[index] {
            0x20..=0x3f => {}
            0x40..=0x7e => {
                buffer.drain(..=index);
                return true;
            }
            _ => {
                buffer.drain(..index);
                return true;
            }
        }
    }

    buffer.drain(..inspected);
    *discarded_tail_bytes = discarded_tail_bytes.saturating_add(inspected);
    *discarded_tail_bytes >= MAX_DISCARDED_CONTROL_TAIL_BYTES
}
