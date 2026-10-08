use super::*;

#[test]
fn host_appearance_prefers_explicit_reports_over_background_inference() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.theme_runtime.auto_switch = true;

    let light = crate::utils::theme::client_palette_for_appearance(
        &state.config.theme_runtime,
        crate::terminal_theme::HostAppearance::Light,
    );
    let dark = crate::utils::theme::client_palette_for_appearance(
        &state.config.theme_runtime,
        crate::terminal_theme::HostAppearance::Dark,
    );

    let inferred = state.handle_raw_events(vec![RawInputEvent::HostDefaultColor {
        kind: crate::terminal_theme::DefaultColorKind::Background,
        color: crate::terminal_theme::RgbColor {
            r: 255,
            g: 255,
            b: 255,
        },
    }]);
    assert!(inferred.repaint);
    assert!(matches!(
        inferred.requests.as_slice(),
        [ClientMessage::ClientShellHostTheme {
            update: crate::protocol::ClientHostThemeUpdate::DefaultColor {
                kind: crate::protocol::ClientHostDefaultColorKind::Background,
                ..
            }
        }]
    ));
    assert_eq!(
        state.host_appearance,
        Some(crate::terminal_theme::HostAppearance::Light)
    );
    assert!(!state.host_appearance_explicit);
    assert_eq!(state.config.palette, light);

    let explicit = state.handle_raw_events(vec![RawInputEvent::HostColorSchemeChanged(
        crate::terminal_theme::HostAppearance::Dark,
    )]);
    assert!(explicit.repaint);
    assert!(explicit.query_host_theme);
    assert!(matches!(
        explicit.requests.as_slice(),
        [ClientMessage::ClientShellHostTheme {
            update: crate::protocol::ClientHostThemeUpdate::Appearance(
                crate::protocol::ClientHostAppearance::Dark
            )
        }]
    ));
    assert_eq!(
        state.host_appearance,
        Some(crate::terminal_theme::HostAppearance::Dark)
    );
    assert!(state.host_appearance_explicit);
    assert_eq!(state.config.palette, dark);

    let ignored = state.handle_raw_events(vec![RawInputEvent::HostDefaultColor {
        kind: crate::terminal_theme::DefaultColorKind::Background,
        color: crate::terminal_theme::RgbColor {
            r: 255,
            g: 255,
            b: 255,
        },
    }]);
    assert!(!ignored.repaint);
    assert_eq!(ignored.requests.len(), 1);
    assert_eq!(
        state.host_appearance,
        Some(crate::terminal_theme::HostAppearance::Dark)
    );
    assert_eq!(state.config.palette, dark);
}

#[test]
fn full_host_palette_response_is_sent_as_one_theme_update() {
    use std::fmt::Write as _;

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut responses = String::new();
    for index in 0..=u8::MAX {
        let _ = write!(responses, "\x1b]4;{index};rgb:1111/2222/3333\x1b\\");
    }

    let outcome = state.handle_input_bytes(responses.as_bytes());

    let [ClientMessage::ClientShellHostTheme {
        update: crate::protocol::ClientHostThemeUpdate::PaletteColors(colors),
    }] = outcome.requests.as_slice()
    else {
        panic!(
            "expected one batched palette update, got {} requests",
            outcome.requests.len()
        );
    };
    assert_eq!(colors.len(), 256);
    assert_eq!(
        colors.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        (0..=u8::MAX).collect::<Vec<_>>()
    );
}

#[test]
fn delayed_link_fallback_does_not_replay_against_changed_geometry() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("pane frame");
    let pane = state.hits.panes[0].clone();
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x + 2,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::CONTROL,
    };
    let activate = state.handle_raw_events(vec![RawInputEvent::Mouse(down)]);
    let request_id = match &activate.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("expected link activation request"),
    };
    state.hits.panes[0].inner_rect.x = state.hits.panes[0].inner_rect.x.saturating_add(1);

    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneLinkActivated {
            url: None,
            handled: false,
        }),
    );

    assert!(actions.is_empty());
    assert!(state.url_click_consumes_until_up);
}

#[test]
fn physical_release_uses_the_leased_press_code_with_current_modifiers() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let press = crate::input::TerminalKey::new(KeyCode::Char('x'), KeyModifiers::empty())
        .with_windows_record(crate::input::WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0x58,
            virtual_scan_code: 0x2d,
            unicode: 0,
            control_key_state: 0,
        });
    state.handle_raw_events(vec![RawInputEvent::Key(press)]);
    let release = crate::input::TerminalKey::new(KeyCode::Char('z'), KeyModifiers::SHIFT)
        .with_kind(crossterm::event::KeyEventKind::Release)
        .with_windows_record(crate::input::WindowsKeyRecord {
            key_down: false,
            repeat_count: 1,
            virtual_key_code: 0x5a,
            virtual_scan_code: 0x2d,
            unicode: 0,
            control_key_state: 0x0010,
        });

    let outcome = state.handle_raw_events(vec![RawInputEvent::Key(release)]);

    assert!(matches!(
        &outcome.requests[..],
        [ClientMessage::ClientShellPaneInput { events, .. }]
            if matches!(
                &events[..],
                [ClientPaneInputEvent::Key {
                    code: crate::protocol::ClientKeyCode::Char('x'),
                    modifiers,
                    kind: crate::protocol::ClientKeyKind::Release,
                    physical_key_id: Some(0x2d),
                    ..
                }] if *modifiers == KeyModifiers::SHIFT.bits()
            )
    ));
}

#[test]
fn pixel_host_reports_use_cells_without_target_pixel_mode_and_release_outside() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].mouse_reporting = true;
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    let geometry =
        crate::input::mouse::HostGeometry::new(106, 20, 1060, 400).expect("host geometry");
    let x = u32::from(pane.inner_rect.x) * 10 + 21;
    let y = u32::from(pane.inner_rect.y) * 20 + 21;

    let down = state.handle_pixel_mouse(format!("\x1b[<0;{x};{y}M").as_bytes(), geometry);
    assert!(matches!(
        &down.requests[..],
        [ClientMessage::ClientShellPaneInput { events, .. }]
            if matches!(
                &events[..],
                [ClientPaneInputEvent::Mouse {
                    position: ClientMousePosition::Cell { column: 2, row: 1 },
                    ..
                }]
            )
    ));

    state.hits.panes.clear();
    let release = state.handle_pixel_mouse(b"\x1b[<0;1;1m", geometry);
    assert!(matches!(
        &release.requests[..],
        [ClientMessage::ClientShellPaneInput { pane_id, events }]
            if pane_id == "pane_1"
                && matches!(
                    &events[..],
                    [ClientPaneInputEvent::Mouse {
                        kind: crate::protocol::ClientMouseKind::Up(
                            crate::protocol::ClientMouseButton::Left
                        ),
                        position: ClientMousePosition::Cell { .. },
                        ..
                    }]
                )
    ));
    assert!(state.pane_mouse_gesture.is_none());
}

#[test]
fn shell_forwards_all_keys_to_the_focused_pane() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));

    let text = state.handle_input_bytes(b"hello");
    assert_eq!(text.requests.len(), 1);
    let ClientMessage::ClientShellPaneInput { pane_id, events } = &text.requests[0] else {
        panic!("expected targeted pane input");
    };
    assert_eq!(pane_id, "pane_1");
    assert_eq!(events.len(), 5);
    assert!(matches!(
        &events[0],
        ClientPaneInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Char('h'),
            generated_text: Some(text),
            ..
        } if text == "h"
    ));

    let interrupt = state.handle_input_bytes(b"\x1b[99;5u");
    assert_eq!(interrupt.requests.len(), 1);
    let ClientMessage::ClientShellPaneInput { events, .. } = &interrupt.requests[0] else {
        panic!("expected semantic interrupt");
    };
    assert!(matches!(
        &events[..],
        [ClientPaneInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Char('c'),
            modifiers,
            kind: crate::protocol::ClientKeyKind::Press,
            ..
        }] if *modifiers == KeyModifiers::CONTROL.bits()
    ));

    let alt = state.handle_input_bytes(b"\x1b[120;3u");
    let ClientMessage::ClientShellPaneInput { events, .. } = &alt.requests[0] else {
        panic!("expected semantic alt key");
    };
    assert!(matches!(
        &events[..],
        [ClientPaneInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Char('x'),
            modifiers,
            ..
        }] if *modifiers == KeyModifiers::ALT.bits()
    ));
    // The former herdr prefix is ordinary pane input: no client-owned modes remain.
    let prefix = state.handle_input_bytes(&[0x02]);
    assert!(!prefix.detach);
    assert_eq!(prefix.requests.len(), 1);
    let quit = state.handle_input_bytes(b"q");
    assert!(!quit.detach);
    assert_eq!(quit.requests.len(), 1);
}

#[test]
fn pane_key_release_keeps_the_press_target() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));

    let press = state.handle_input_bytes(b"\x1b[99;5u");
    let release = state.handle_input_bytes(b"\x1b[99;5:3u");
    let ClientMessage::ClientShellPaneInput {
        pane_id: press_target,
        ..
    } = &press.requests[0]
    else {
        panic!("expected targeted press");
    };
    let ClientMessage::ClientShellPaneInput {
        pane_id: release_target,
        events,
    } = &release.requests[0]
    else {
        panic!("expected targeted release");
    };
    assert_eq!(release_target, press_target);
    assert!(matches!(
        &events[..],
        [ClientPaneInputEvent::Key {
            kind: crate::protocol::ClientKeyKind::Release,
            ..
        }]
    ));
}

#[test]
fn styled_client_composition_preserves_pane_hyperlinks() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    let linked = Buffer::with_lines(["LIVE", "PANE"]);
    pane_surface.frame = FrameData::from_ratatui_buffer_with_hyperlinks(
        &linked,
        None,
        &[((0, 0), "L".into(), "https://example.test".into())],
    );
    state.set_pane_surface(pane_surface);
    let mut selection =
        crate::selection::Selection::absolute_range("pane_1".to_owned(), (0, 0), (0, 1));
    assert!(selection.finish());
    state.selection = Some(selection);
    let frame = state.compose(106, 20).expect("composed frame");
    let hit = &state.hits.panes[0];
    let index =
        usize::from(hit.inner_rect.y) * usize::from(frame.width) + usize::from(hit.inner_rect.x);
    let link = frame.cells[index].hyperlink.expect("linked cell") as usize;
    assert_eq!(frame.hyperlinks[link], "https://example.test");
}
