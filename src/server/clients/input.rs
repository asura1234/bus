use bytes::Bytes;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};

use crate::protocol::wire::ClientPaneInputEvent;

pub(in crate::server) fn downgrade_ineligible_pixel_mouse(
    events: &mut [ClientPaneInputEvent],
    pixel_mouse: bool,
    runtime_size: (u16, u16),
    runtime_pixels: Option<(u32, u32)>,
) {
    let (runtime_rows, runtime_cols) = runtime_size;
    for event in events {
        let ClientPaneInputEvent::Mouse {
            position, geometry, ..
        } = event
        else {
            continue;
        };
        let crate::protocol::wire::ClientMousePosition::Pixels { x, y, column, row } = *position
        else {
            continue;
        };
        let exact = pixel_mouse
            && geometry.is_some_and(|geometry| {
                (runtime_rows, runtime_cols) == (geometry.rows, geometry.cols)
                    && runtime_pixels == Some((geometry.width_px, geometry.height_px))
                    && column < geometry.cols
                    && row < geometry.rows
                    && x > 0
                    && y > 0
                    && x <= geometry.width_px
                    && y <= geometry.height_px
            });
        if !exact {
            *position = crate::protocol::wire::ClientMousePosition::Cell { column, row };
            *geometry = None;
        }
    }
}

fn apply_scroll(
    runtime: &crate::terminal::TerminalRuntime,
    wheel_kind: MouseEventKind,
    lines: u16,
    position: crate::protocol::keys::mouse::Position,
    modifiers: u8,
) -> Result<(), String> {
    match runtime.wheel_routing() {
        Some(crate::terminal::runtime::WheelRouting::MouseReport) => {
            runtime.scroll_reset();
            let Some(bytes) = runtime.encode_mouse_wheel(
                wheel_kind,
                position,
                KeyModifiers::from_bits_truncate(modifiers),
            ) else {
                return Err(format!(
                    "failed to encode pane mouse wheel event: {wheel_kind:?}"
                ));
            };
            runtime
                .try_send_bytes(Bytes::from(bytes))
                .map_err(|err| format!("pane mouse wheel input failed: {err}"))?;
        }
        Some(crate::terminal::runtime::WheelRouting::AlternateScroll) => {
            runtime.scroll_reset();
            let Some(bytes) = runtime.encode_alternate_scroll(wheel_kind) else {
                return Ok(());
            };
            runtime
                .try_send_bytes(Bytes::from(bytes))
                .map_err(|err| format!("pane alternate scroll input failed: {err}"))?;
        }
        Some(crate::terminal::runtime::WheelRouting::HostScroll) | None => match wheel_kind {
            MouseEventKind::ScrollUp => runtime.scroll_up(lines.max(1) as usize),
            MouseEventKind::ScrollDown => runtime.scroll_down(lines.max(1) as usize),
            _ => unreachable!("only vertical wheel events reach apply_scroll"),
        },
    }
    Ok(())
}

pub(in crate::server) fn apply_client_pane_input_events(
    runtime: &crate::terminal::TerminalRuntime,
    events: &[ClientPaneInputEvent],
) -> Result<(), String> {
    for event in events {
        if let ClientPaneInputEvent::Mouse {
            kind,
            position,
            modifiers,
            lines,
            ..
        } = event
        {
            apply_client_pane_mouse_input(runtime, *kind, position, *modifiers, *lines)?;
            continue;
        }

        match event.to_raw_input_event() {
            crate::protocol::keys::host::RawInputEvent::Key(key) => {
                let key_event = key.as_key_event();
                if matches!(key_event.code, KeyCode::PageUp | KeyCode::PageDown)
                    && key_event.modifiers.is_empty()
                    && runtime.plain_page_keys_use_host_scrollback() == Some(true)
                {
                    match key_event.kind {
                        KeyEventKind::Release => continue,
                        KeyEventKind::Press | KeyEventKind::Repeat => {
                            let lines = runtime.current_size().0.max(1) as usize;
                            if key_event.code == KeyCode::PageUp {
                                runtime.scroll_up(lines);
                            } else {
                                runtime.scroll_down(lines);
                            }
                            continue;
                        }
                    }
                }

                runtime.scroll_reset();
                let bytes = runtime.encode_terminal_key(key);
                if !bytes.is_empty() {
                    runtime
                        .try_send_bytes(Bytes::from(bytes))
                        .map_err(|err| format!("targeted pane key input failed: {err}"))?;
                }
            }
            crate::protocol::keys::host::RawInputEvent::Text(text) => {
                runtime.scroll_reset();
                runtime
                    .try_send_bytes(Bytes::copy_from_slice(text.as_str().as_bytes()))
                    .map_err(|err| format!("targeted pane text input failed: {err}"))?;
            }
            crate::protocol::keys::host::RawInputEvent::Paste(text) => {
                runtime.scroll_reset();
                runtime
                    .try_send_paste(text)
                    .map_err(|err| format!("targeted pane paste failed: {err}"))?;
            }
            crate::protocol::keys::host::RawInputEvent::Mouse(_)
            | crate::protocol::keys::host::RawInputEvent::OuterFocusGained
            | crate::protocol::keys::host::RawInputEvent::OuterFocusLost
            | crate::protocol::keys::host::RawInputEvent::HostDefaultColor { .. }
            | crate::protocol::keys::host::RawInputEvent::HostPaletteColors { .. }
            | crate::protocol::keys::host::RawInputEvent::HostColorSchemeChanged(_)
            | crate::protocol::keys::host::RawInputEvent::HostCellSizeReport { .. }
            | crate::protocol::keys::host::RawInputEvent::Unsupported => {
                return Err("non-pane input reached targeted pane input".to_owned());
            }
        }
    }
    Ok(())
}

fn apply_client_pane_mouse_input(
    runtime: &crate::terminal::TerminalRuntime,
    kind: crate::protocol::wire::ClientMouseKind,
    position: &crate::protocol::wire::ClientMousePosition,
    modifiers: u8,
    lines: u16,
) -> Result<(), String> {
    let kind = kind.to_crossterm();
    let modifiers = KeyModifiers::from_bits_truncate(modifiers);
    let position = match position {
        crate::protocol::wire::ClientMousePosition::Cell { column, row } => {
            crate::protocol::keys::mouse::Position::Cell {
                column: *column,
                row: *row,
            }
        }
        crate::protocol::wire::ClientMousePosition::Pixels { x, y, column, row } => {
            if runtime.sgr_pixel_mouse_enabled() {
                crate::protocol::keys::mouse::Position::Pixels { x: *x, y: *y }
            } else {
                crate::protocol::keys::mouse::Position::Cell {
                    column: *column,
                    row: *row,
                }
            }
        }
    };
    let bytes = match kind {
        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
            apply_scroll(runtime, kind, lines.max(1), position, modifiers.bits())?;
            return Ok(());
        }
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => runtime
            .encode_mouse_wheel(kind, position, modifiers)
            .unwrap_or_default(),
        MouseEventKind::Down(_) | MouseEventKind::Up(_) | MouseEventKind::Drag(_) => runtime
            .encode_mouse_button(kind, position, modifiers)
            .unwrap_or_default(),
        MouseEventKind::Moved => runtime
            .encode_mouse_motion(kind, position, modifiers)
            .unwrap_or_default(),
    };
    if !bytes.is_empty() {
        if kind != MouseEventKind::Moved {
            runtime.scroll_reset();
        }
        runtime
            .try_send_bytes(Bytes::from(bytes))
            .map_err(|err| format!("targeted pane mouse input failed: {err}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ineligible_shell_pixel_mouse_uses_its_canonical_cell_position() {
        let mut events = vec![ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Down(
                crate::protocol::wire::ClientMouseButton::Left,
            ),
            position: crate::protocol::wire::ClientMousePosition::Pixels {
                x: 121,
                y: 81,
                column: 12,
                row: 4,
            },
            geometry: Some(crate::protocol::wire::ClientMouseGeometry {
                cols: 20,
                rows: 5,
                width_px: 200,
                height_px: 100,
            }),
            modifiers: 0,
            lines: 1,
        }];

        downgrade_ineligible_pixel_mouse(&mut events, false, (5, 20), Some((200, 100)));

        assert!(matches!(
            events.as_slice(),
            [ClientPaneInputEvent::Mouse {
                position: crate::protocol::wire::ClientMousePosition::Cell { column: 12, row: 4 },
                ..
            }]
        ));
    }

    #[test]
    fn eligible_shell_pixel_mouse_remains_exact() {
        let position = crate::protocol::wire::ClientMousePosition::Pixels {
            x: 121,
            y: 81,
            column: 12,
            row: 4,
        };
        let mut events = vec![ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position,
            geometry: Some(crate::protocol::wire::ClientMouseGeometry {
                cols: 20,
                rows: 5,
                width_px: 200,
                height_px: 100,
            }),
            modifiers: 0,
            lines: 1,
        }];

        downgrade_ineligible_pixel_mouse(&mut events, true, (5, 20), Some((200, 100)));

        assert!(matches!(
            events.as_slice(),
            [ClientPaneInputEvent::Mouse {
                position: current,
                ..
            }] if *current == position
        ));
    }

    #[test]
    fn stale_shell_pixel_geometry_downgrades_to_its_canonical_cell() {
        let mut events = vec![ClientPaneInputEvent::Mouse {
            kind: crate::protocol::wire::ClientMouseKind::Moved,
            position: crate::protocol::wire::ClientMousePosition::Pixels {
                x: 121,
                y: 81,
                column: 12,
                row: 4,
            },
            geometry: Some(crate::protocol::wire::ClientMouseGeometry {
                cols: 20,
                rows: 5,
                width_px: 200,
                height_px: 100,
            }),
            modifiers: 0,
            lines: 1,
        }];

        downgrade_ineligible_pixel_mouse(&mut events, true, (6, 20), Some((200, 120)));

        assert!(matches!(
            events.as_slice(),
            [ClientPaneInputEvent::Mouse {
                position: crate::protocol::wire::ClientMousePosition::Cell { column: 12, row: 4 },
                geometry: None,
                ..
            }]
        ));
    }
}
