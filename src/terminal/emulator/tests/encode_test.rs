use super::*;

#[test]
fn ghostty_keyboard_protocol_tracks_live_terminal_flags() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>3u");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    assert_eq!(
        pane.keyboard_protocol(),
        Some(crate::protocol::keys::KeyboardProtocol::Kitty { flags: 3 })
    );
}

#[test]
fn ghostty_plain_text_chars_still_encode_as_text() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );

    assert_eq!(encoded, b"a");
}

#[test]
fn ghostty_backtab_preserves_shift_across_keyboard_protocols() {
    for (kitty_flags, expected) in [
        (None, b"\x1b[Z".as_slice()),
        (Some(1), b"\x1b[9;2u".as_slice()),
    ] {
        let (tx, _rx) = mpsc::channel(4);
        let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
        if let Some(flags) = kitty_flags {
            terminal.write(format!("\x1b[>{flags}u").as_bytes());
        }
        let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
        let protocol = pane.keyboard_protocol().unwrap();

        for modifiers in [
            crossterm::event::KeyModifiers::empty(),
            crossterm::event::KeyModifiers::SHIFT,
        ] {
            let encoded = pane.encode_terminal_key(
                crate::protocol::keys::TerminalKey::new(
                    crossterm::event::KeyCode::BackTab,
                    modifiers,
                ),
                protocol,
            );
            assert_eq!(encoded, expected, "backtab with modifiers {modifiers:?}");
        }
    }

    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let encoded = pane.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );
    assert_eq!(encoded, b"\t");
}

#[test]
fn ghostty_ctrl_tab_matches_the_pane_keyboard_protocol() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let legacy = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let key = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::CONTROL,
    );

    assert_eq!(
        legacy.encode_terminal_key(key.clone(), crate::protocol::keys::KeyboardProtocol::Legacy),
        b"\t"
    );

    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>3u");
    let kitty = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    assert_eq!(
        kitty.encode_terminal_key(
            key,
            crate::protocol::keys::KeyboardProtocol::Kitty { flags: 3 }
        ),
        b"\x1b[9;5u"
    );
}

#[test]
fn ghostty_enter_backspace_release_in_legacy_pane_emits_nothing() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    for code in [
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyCode::Backspace,
    ] {
        let press = pane.encode_terminal_key(
            crate::protocol::keys::TerminalKey::new(code, crossterm::event::KeyModifiers::empty()),
            crate::protocol::keys::KeyboardProtocol::Legacy,
        );
        let release = pane.encode_terminal_key(
            crate::protocol::keys::TerminalKey::new(code, crossterm::event::KeyModifiers::empty())
                .with_kind(crossterm::event::KeyEventKind::Release),
            crate::protocol::keys::KeyboardProtocol::Legacy,
        );
        assert!(!press.is_empty(), "{code:?} press should emit bytes");
        assert!(
            release.is_empty(),
            "{code:?} release should emit nothing in a legacy pane, got {release:?}"
        );
    }
}

#[test]
fn ghostty_report_event_pane_keeps_basic_compatibility_keys_legacy() {
    let (tx, _rx) = mpsc::channel(4);
    // Push kitty flags including REPORT_EVENT_TYPES (0b10) + DISAMBIGUATE (0b1).
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>3u");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    for (code, expected) in [
        (crossterm::event::KeyCode::Enter, b"\r".as_slice()),
        (crossterm::event::KeyCode::Backspace, b"\x7f".as_slice()),
    ] {
        let press = pane.encode_terminal_key(
            crate::protocol::keys::TerminalKey::new(code, crossterm::event::KeyModifiers::empty()),
            pane.keyboard_protocol().unwrap(),
        );
        assert_eq!(
            press, expected,
            "{code:?} press should stay legacy-compatible without REPORT_ALL_KEYS"
        );

        let release = pane.encode_terminal_key(
            crate::protocol::keys::TerminalKey::new(code, crossterm::event::KeyModifiers::empty())
                .with_kind(crossterm::event::KeyEventKind::Release),
            pane.keyboard_protocol().unwrap(),
        );
        assert!(
            release.is_empty(),
            "{code:?} release should not fall back to legacy bytes, got {release:?}"
        );
    }
}

#[test]
fn ghostty_char_keys_still_use_herdr_encoding() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>1u");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::SHIFT,
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );

    assert_eq!(encoded, vec![1]);
}

#[test]
fn ghostty_key_encoding_honors_application_cursor_mode() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal
        .mode_set(crate::terminal::vt::MODE_APPLICATION_CURSOR_KEYS, true)
        .unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );

    assert_eq!(encoded, b"\x1bOA");
}

#[test]
fn grouped_key_repeats_expand_at_the_destination() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let key = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Char('x'),
        crossterm::event::KeyModifiers::empty(),
    )
    .with_repeat_count(3);

    assert_eq!(
        pane.encode_terminal_key(key, crate::protocol::keys::KeyboardProtocol::Legacy),
        b"xxx"
    );

    let shifted = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Char('/'),
        crossterm::event::KeyModifiers::SHIFT,
    )
    .with_generated_text(Some("/".to_owned()))
    .with_windows_record(crate::protocol::keys::WindowsKeyRecord {
        key_down: true,
        repeat_count: 3,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: u16::from(b'/'),
        control_key_state: 0x0010,
    });
    #[cfg(windows)]
    let legacy_expected = b"\x1b[55;8;47;1;16;3_".as_slice();
    #[cfg(not(windows))]
    let legacy_expected = b"///".as_slice();
    assert_eq!(
        pane.encode_terminal_key(
            shifted.clone(),
            crate::protocol::keys::KeyboardProtocol::Legacy,
        ),
        legacy_expected
    );
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>15u");
    let (tx, _rx) = mpsc::channel(4);
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    assert_eq!(
        pane.encode_terminal_key(
            shifted,
            crate::protocol::keys::KeyboardProtocol::Kitty { flags: 15 },
        ),
        b"\x1b[47;2:1u\x1b[47;2:2u\x1b[47;2:2u"
    );
}

#[test]
fn grouped_release_is_encoded_once() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[>11u");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let protocol = pane.keyboard_protocol().unwrap();
    let release = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Up,
        crossterm::event::KeyModifiers::empty(),
    )
    .with_kind(crossterm::event::KeyEventKind::Release);
    let expected = pane.encode_terminal_key(release.clone(), protocol);

    assert!(!expected.is_empty());
    let mut malformed_release = release;
    malformed_release.repeat_count = 3;
    assert_eq!(
        pane.encode_terminal_key(malformed_release, protocol),
        expected
    );
}

#[test]
fn ghostty_key_encoder_updates_after_terminal_mode_changes() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let before = pane.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );
    assert_eq!(before, b"\x1b[A");

    pane.process_pty_bytes(pane_id, 0, b"\x1b[?1h", &tx, |_| None);

    let after = pane.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );
    assert_eq!(after, b"\x1bOA");
}

#[test]
fn ghostty_key_encoder_updates_after_kitty_flag_changes() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let key = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::SHIFT,
    );

    let before =
        pane.encode_terminal_key(key.clone(), crate::protocol::keys::KeyboardProtocol::Legacy);
    pane.process_pty_bytes(pane_id, 0, b"\x1b[>1u", &tx, |_| None);
    let after =
        pane.encode_terminal_key(key.clone(), crate::protocol::keys::KeyboardProtocol::Legacy);

    assert_ne!(before, after);
    assert_eq!(after, b"\x1b[13;6u");
}

#[test]
fn ghostty_kitty_pane_encodes_shift_enter_as_csi_u() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, 0, b"\x1b[>5u", &tx, |_| None);

    let key = crate::protocol::keys::parse_terminal_key_sequence("\x1b[13;2u").unwrap();
    let encoded =
        pane.encode_terminal_key(key.clone(), crate::protocol::keys::KeyboardProtocol::Legacy);

    assert_eq!(
        pane.keyboard_protocol(),
        Some(crate::protocol::keys::KeyboardProtocol::Kitty { flags: 5 })
    );
    assert_eq!(encoded, b"\x1b[13;2u");
}

#[cfg(windows)]
#[test]
fn windows_ghostty_default_pane_preserves_synthesized_shift_enter_fallback() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let key = crate::protocol::keys::parse_terminal_key_sequence("\x1b[13;2u").unwrap();

    assert_eq!(
        pane.encode_terminal_key(key, crate::protocol::keys::KeyboardProtocol::Legacy),
        b"\x1b[13;28;13;1;16;1_"
    );
}

#[test]
fn ghostty_modify_other_keys_mode_one_preserves_shift_enter() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();
    let key = crate::protocol::keys::parse_terminal_key_sequence("\x1b[13;2u").unwrap();

    pane.seed_history_ansi("\x1b[>4;1m");
    assert_eq!(pane.modify_other_keys_level(), 1);
    let encoded =
        pane.encode_terminal_key(key.clone(), crate::protocol::keys::KeyboardProtocol::Legacy);

    assert_eq!(encoded, b"\x1b[27;2;13~");
}

#[test]
fn ghostty_kitty_pane_encodes_parsed_legacy_alt_backspace_as_csi_u() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, 0, b"\x1b[>1u", &tx, |_| None);

    let key = crate::protocol::keys::parse_terminal_key_sequence("\x1b\x7f").unwrap();
    let encoded =
        pane.encode_terminal_key(key.clone(), crate::protocol::keys::KeyboardProtocol::Legacy);

    assert_eq!(encoded, b"\x1b[127;3u");
}

#[test]
fn ghostty_kitty_pane_preserves_legacy_ctrl_alt_letter() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.process_pty_bytes(pane_id, 0, b"\x1b[>5u", &tx, |_| None);

    let mut events = crate::protocol::keys::host::parse_raw_input_bytes_sync(b"\x1b\x06");
    let crate::protocol::keys::host::RawInputEvent::Key(key) = events.remove(0) else {
        panic!("expected key event");
    };
    let encoded = pane.encode_terminal_key(key, pane.keyboard_protocol().unwrap());

    assert_eq!(encoded, b"\x1b[102;7u");
}

#[test]
fn ghostty_pane_characterizes_ctrl_backspace_encoding() {
    let (tx, _rx) = mpsc::channel(4);
    let legacy = GhosttyPaneTerminal::new(
        crate::terminal::vt::Terminal::new(80, 24, 0).unwrap(),
        tx.clone(),
    )
    .unwrap();

    let ctrl_backspace = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert_eq!(
        legacy.encode_terminal_key(
            ctrl_backspace.clone(),
            crate::protocol::keys::KeyboardProtocol::Legacy
        ),
        b"\x08"
    );

    let plain_backspace = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Backspace,
        crossterm::event::KeyModifiers::empty(),
    );
    assert_eq!(
        legacy.encode_terminal_key(
            plain_backspace,
            crate::protocol::keys::KeyboardProtocol::Legacy
        ),
        b"\x7f"
    );

    let kitty = GhosttyPaneTerminal::new(
        crate::terminal::vt::Terminal::new(80, 24, 0).unwrap(),
        tx.clone(),
    )
    .unwrap();
    let pane_id = PaneId::from_raw(1);
    kitty.process_pty_bytes(pane_id, 0, b"\x1b[>1u", &tx, |_| None);

    assert_eq!(
        kitty.encode_terminal_key(
            ctrl_backspace,
            crate::protocol::keys::KeyboardProtocol::Legacy
        ),
        b"\x1b[127;5u"
    );
}

#[test]
fn ghostty_key_encoders_are_isolated_per_pane() {
    let (tx, _rx) = mpsc::channel(4);
    let first = GhosttyPaneTerminal::new(
        crate::terminal::vt::Terminal::new(80, 24, 0).unwrap(),
        tx.clone(),
    )
    .unwrap();
    let second = GhosttyPaneTerminal::new(
        crate::terminal::vt::Terminal::new(80, 24, 0).unwrap(),
        tx.clone(),
    )
    .unwrap();

    first.process_pty_bytes(PaneId::from_raw(1), 0, b"\x1b[?1h", &tx, |_| None);

    let first_encoded = first.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );
    let second_encoded = second.encode_terminal_key(
        crate::protocol::keys::TerminalKey::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::empty(),
        ),
        crate::protocol::keys::KeyboardProtocol::Legacy,
    );

    assert_eq!(first_encoded, b"\x1bOA");
    assert_eq!(second_encoded, b"\x1b[A");
}

#[test]
fn ghostty_mouse_button_encoding_uses_live_terminal_state() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1000h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_mouse_button(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        crate::protocol::keys::mouse::Position::Cell { column: 11, row: 9 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<0;12;10m"[..]));
}

#[test]
fn ghostty_mouse_drag_encoding_uses_motion_reporting_state() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1002h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_mouse_button(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        crate::protocol::keys::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::SHIFT,
    );

    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<36;5;7M"[..]));
}

#[test]
fn ghostty_mouse_drag_without_motion_reporting_is_not_forwarded() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1000h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_mouse_button(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        crate::protocol::keys::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(encoded, None);
}

#[test]
fn ghostty_mouse_moved_encoding_uses_any_motion_state() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.write(b"\x1b[?1003h\x1b[?1006h");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let encoded = pane.encode_mouse_motion(
        crossterm::event::MouseEventKind::Moved,
        crate::protocol::keys::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(encoded.as_deref(), Some(&b"\x1b[<35;5;7M"[..]));
}

#[test]
fn ghostty_mouse_sgr_pixels_preserves_exact_and_downgrades_cell_input() {
    let (tx, _rx) = mpsc::channel(4);
    let mut terminal = crate::terminal::vt::Terminal::new(80, 24, 0).unwrap();
    terminal.resize(80, 24, 10, 20).unwrap();
    terminal.write(b"\x1b[?1003h\x1b[?1006h\x1b[?1016h");
    let pane = GhosttyPaneTerminal::new(terminal, tx).unwrap();

    let exact = pane.encode_mouse_motion(
        crossterm::event::MouseEventKind::Moved,
        crate::protocol::keys::mouse::Position::Pixels { x: 48, y: 139 },
        crossterm::event::KeyModifiers::empty(),
    );
    let fallback = pane.encode_mouse_motion(
        crossterm::event::MouseEventKind::Moved,
        crate::protocol::keys::mouse::Position::Cell { column: 4, row: 6 },
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(exact.as_deref(), Some(&b"\x1b[<35;48;139M"[..]));
    assert_eq!(fallback.as_deref(), Some(&b"\x1b[<35;5;7M"[..]));
}
