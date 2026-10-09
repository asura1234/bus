use super::mouse::{parse_default_mouse, parse_sgr_mouse};
use super::replies::{parse_host_cell_size_report, parse_host_color_scheme_report};
use super::sequence::{complete_escape_sequence_len, find_subsequence, first_complete_utf8_char};
use super::{BRACKETED_PASTE_END, BRACKETED_PASTE_START, ESC};
use crate::protocol::keys::{parse_terminal_key_sequence, TerminalKey, TextCommit};
use crate::utils::theme::color::{
    parse_default_color_response, parse_palette_color_response, DefaultColorKind, HostAppearance,
    RgbColor,
};
use crossterm::event::MouseEvent;

#[derive(Debug)]
pub enum RawInputEvent {
    Key(TerminalKey),
    Text(TextCommit),
    Paste(String),
    Mouse(MouseEvent),
    OuterFocusGained,
    OuterFocusLost,
    HostDefaultColor {
        kind: DefaultColorKind,
        color: RgbColor,
    },
    HostPaletteColors {
        colors: Vec<(u8, RgbColor)>,
    },
    HostColorSchemeChanged(HostAppearance),
    // The dimensions are only read by the Unix client.
    #[cfg_attr(not(any(unix, test)), allow(dead_code))]
    HostCellSizeReport {
        width_px: u32,
        height_px: u32,
    },
    Unsupported,
}

pub(super) fn extract_one_event(buffer: &[u8]) -> Option<(RawInputEvent, usize)> {
    if buffer.is_empty() {
        return None;
    }

    if buffer.starts_with(BRACKETED_PASTE_START) {
        let end = find_subsequence(buffer, BRACKETED_PASTE_END)?;
        let content = std::str::from_utf8(&buffer[BRACKETED_PASTE_START.len()..end]).ok()?;
        return Some((
            RawInputEvent::Paste(content.to_string()),
            end + BRACKETED_PASTE_END.len(),
        ));
    }

    if buffer[0] == ESC {
        let seq_len = complete_escape_sequence_len(buffer)?;
        if buffer[..seq_len].starts_with(b"\x1b[M") {
            let event = parse_default_mouse(&buffer[..seq_len])
                .map(RawInputEvent::Mouse)
                .unwrap_or(RawInputEvent::Unsupported);
            return Some((event, seq_len));
        }
        let seq = std::str::from_utf8(&buffer[..seq_len]).ok()?;

        if let Some((kind, color)) = parse_default_color_response(seq) {
            return Some((RawInputEvent::HostDefaultColor { kind, color }, seq_len));
        }
        if let Some((index, color)) = parse_palette_color_response(seq) {
            return Some((
                RawInputEvent::HostPaletteColors {
                    colors: vec![(index, color)],
                },
                seq_len,
            ));
        }

        match seq {
            "\x1b[I" => return Some((RawInputEvent::OuterFocusGained, seq_len)),
            "\x1b[O" => return Some((RawInputEvent::OuterFocusLost, seq_len)),
            _ => {}
        }

        if let Some(appearance) = parse_host_color_scheme_report(&buffer[..seq_len]) {
            return Some((RawInputEvent::HostColorSchemeChanged(appearance), seq_len));
        }

        if let Some((width_px, height_px)) = parse_host_cell_size_report(&buffer[..seq_len]) {
            return Some((
                RawInputEvent::HostCellSizeReport {
                    width_px,
                    height_px,
                },
                seq_len,
            ));
        }

        if let Some(mouse) = parse_sgr_mouse(seq) {
            return Some((RawInputEvent::Mouse(mouse), seq_len));
        }

        if let Some(key) = parse_terminal_key_sequence(seq) {
            return Some((
                RawInputEvent::Key(key.with_vt_bytes(buffer[..seq_len].to_vec())),
                seq_len,
            ));
        }

        tracing::debug!(sequence = ?seq, "dropping unsupported escape sequence");
        return Some((RawInputEvent::Unsupported, seq_len));
    }

    let text = first_complete_utf8_char(buffer)?;
    let consumed = text.len();
    let key = parse_terminal_key_sequence(text)?
        .with_text_commit()
        .with_vt_bytes(buffer[..consumed].to_vec());
    Some((RawInputEvent::Key(key), consumed))
}
