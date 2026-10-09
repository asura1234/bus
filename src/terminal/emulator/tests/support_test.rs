use super::*;

pub(super) fn rgb(r: u8, g: u8, b: u8) -> crate::terminal::vt::RgbColor {
    crate::terminal::vt::RgbColor { r, g, b }
}

pub(super) fn current_palette_color(
    pane: &GhosttyPaneTerminal,
    index: u8,
) -> crate::terminal::vt::RgbColor {
    let mut core = pane.core.lock().unwrap();
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = &mut *core;
    render_state.update(terminal).unwrap();
    render_state.colors().unwrap().palette[usize::from(index)]
}

pub(super) fn current_default_colors(
    pane: &GhosttyPaneTerminal,
) -> crate::terminal::vt::RenderColors {
    let mut core = pane.core.lock().unwrap();
    let GhosttyPaneCore {
        terminal,
        render_state,
        ..
    } = &mut *core;
    render_state.update(terminal).unwrap();
    render_state.colors().unwrap()
}

pub(super) fn expected_osc_rgb_response(
    command: &str,
    color: crate::terminal::vt::RgbColor,
    terminator: &str,
) -> Bytes {
    let r = u16::from(color.r) * 257;
    let g = u16::from(color.g) * 257;
    let b = u16::from(color.b) * 257;
    Bytes::from(format!(
        "\x1b]{command};rgb:{r:04x}/{g:04x}/{b:04x}{terminator}"
    ))
}

#[cfg(windows)]
pub(super) fn process_windows_powershell_prompt_bytes(
    bytes: &[u8],
    cols: u16,
    rows: u16,
    enabled: bool,
) -> ProcessBytesResult {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(cols, rows, 100).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    pane.set_windows_powershell_prompt_cwd_reporting(enabled);
    pane.process_pty_bytes(PaneId::from_raw(1), 0, bytes, &tx, |_| None)
}

pub(super) fn expected_xtgettcap_response(cap_hex: &str, value: Option<&[u8]>) -> Bytes {
    let mut response = format!("\x1bP1+r{cap_hex}").into_bytes();
    if let Some(value) = value {
        response.push(b'=');
        append_upper_hex(value, &mut response);
    }
    response.extend_from_slice(b"\x1b\\");
    Bytes::from(response)
}

pub(super) fn append_upper_hex(bytes: &[u8], output: &mut Vec<u8>) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
}
