use super::framer::MAX_DISCARDED_CONTROL_TAIL_BYTES;
use super::sequence::{control_string, find_subsequence, ControlString, ControlStringFamily};
use super::{
    BRACKETED_PASTE_END, BRACKETED_PASTE_START, GHOSTTY_COLOR_SCHEME_DARK_REPORT,
    GHOSTTY_COLOR_SCHEME_LIGHT_REPORT,
};
use crate::utils::theme::color::{
    parse_default_color_response, parse_palette_color_response, HostAppearance,
};

/// Host colour queries and bracketed paste share stdin, so replies can arrive
/// inside the paste delimiters. Route recognized replies separately while
/// retaining all other paste bytes in a single bracketed edit.
pub(super) fn split_paste_color_replies(buffer: &[u8]) -> Option<(Vec<Vec<u8>>, usize)> {
    let content = buffer.strip_prefix(BRACKETED_PASTE_START)?;
    let end = find_subsequence(content, BRACKETED_PASTE_END)?;
    let content = &content[..end];
    let mut chunks = Vec::new();
    let mut paste = BRACKETED_PASTE_START.to_vec();
    let mut scan = 0;
    let mut retained_start = 0;
    while let Some(offset) = find_subsequence(&content[scan..], b"\x1b]") {
        let start = scan + offset;
        let Some(ControlString::Complete { len }) = control_string(&content[start..]) else {
            scan = start + 2;
            continue;
        };
        let reply = &content[start..start + len];
        if std::str::from_utf8(reply).ok().is_some_and(|text| {
            parse_default_color_response(text).is_some()
                || parse_palette_color_response(text).is_some()
        }) {
            paste.extend_from_slice(&content[retained_start..start]);
            chunks.push(reply.to_vec());
            retained_start = start + len;
        }
        scan = start + len;
    }
    if chunks.is_empty() {
        return None;
    }
    paste.extend_from_slice(&content[retained_start..]);
    paste.extend_from_slice(BRACKETED_PASTE_END);
    chunks.push(paste);
    Some((
        chunks,
        BRACKETED_PASTE_START.len() + end + BRACKETED_PASTE_END.len(),
    ))
}

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

pub(super) fn starts_with_incomplete_palette_color_response(buffer: &[u8]) -> bool {
    matches!(
        control_string(buffer),
        Some(ControlString::Incomplete {
            family: ControlStringFamily::Osc
        })
    ) && buffer.starts_with(b"\x1b]4;")
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
