use super::*;

#[test]
fn client_input_roundtrip() {
    let msg = ClientMessage::Input {
        data: vec![0x1b, 0x5b, 0x41], // ESC [ A (up arrow)
    };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}

#[test]
fn client_shell_pane_input_roundtrips_semantic_and_windows_keys() {
    let windows_record = crate::protocol::keys::WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: 0,
        control_key_state: 0x0008,
    };
    let message = ClientMessage::ClientShellPaneInput {
        pane_id: "w1:p2".into(),
        events: vec![
            ClientPaneInputEvent::Key {
                code: ClientKeyCode::Char('l'),
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: ClientKeyKind::Release,
                repeat_count: 1,
                shifted_codepoint: Some('L' as u32),
                generated_text: None,
                tracks_release: true,
                physical_key_id: None,
                windows_record: None,
            },
            ClientPaneInputEvent::Key {
                code: ClientKeyCode::Char('7'),
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: ClientKeyKind::Press,
                repeat_count: 1,
                shifted_codepoint: None,
                generated_text: None,
                tracks_release: true,
                physical_key_id: Some(0x08),
                windows_record: Some(windows_record),
            },
        ],
    };
    let encoded = bincode::serde::encode_to_vec(&message, bincode::config::standard())
        .expect("encode targeted semantic input");
    let (decoded, _): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .expect("decode targeted semantic input");
    assert_eq!(decoded, message);
    let ClientMessage::ClientShellPaneInput { events, .. } = decoded else {
        panic!("expected targeted semantic input");
    };
    let crate::protocol::keys::host::RawInputEvent::Key(semantic) = events[0].to_raw_input_event()
    else {
        panic!("expected semantic key");
    };
    assert_eq!(semantic.shifted_codepoint, Some('L' as u32));
    assert_eq!(semantic.kind, crossterm::event::KeyEventKind::Release);
    let crate::protocol::keys::host::RawInputEvent::Key(key) = events[1].to_raw_input_event()
    else {
        panic!("expected Windows key");
    };
    assert_eq!(key.code, crossterm::event::KeyCode::Char('7'));
    assert_eq!(key.modifiers, crossterm::event::KeyModifiers::CONTROL);
    assert_eq!(key.windows_record(), Some(windows_record));
}

#[test]
fn client_shell_pane_input_reconstructs_windows_dead_key_without_native_source() {
    let event = ClientPaneInputEvent::Key {
        code: ClientKeyCode::Char('6'),
        modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
        kind: ClientKeyKind::Press,
        repeat_count: 1,
        shifted_codepoint: None,
        generated_text: None,
        tracks_release: true,
        physical_key_id: Some(0x07),
        windows_record: Some(crate::protocol::keys::WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0x36,
            virtual_scan_code: 0x07,
            unicode: 0,
            control_key_state: 0x0030,
        }),
    };
    let crate::protocol::keys::host::RawInputEvent::Key(key) =
        event.to_raw_input_event_with_windows_source(false)
    else {
        panic!("pane dead key should remain a key");
    };
    assert!(key.is_windows_shift_dead_key());
    assert_eq!(key.windows_record(), None);
    assert!(crate::protocol::keys::encode_terminal_key(
        key,
        crate::protocol::keys::KeyboardProtocol::Kitty { flags: 1 },
    )
    .is_empty());
}

#[test]
fn client_shell_key_roundtrip_preserves_physical_generated_text_encoding() {
    let key = crate::protocol::keys::TerminalKey::new(
        crossterm::event::KeyCode::Char('/'),
        crossterm::event::KeyModifiers::SHIFT,
    )
    .with_generated_text(Some("/".into()))
    .with_windows_record(crate::protocol::keys::WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: '/' as u16,
        control_key_state: 0,
    });
    let event = ClientPaneInputEvent::from_terminal_key(key).expect("semantic pane key");
    assert!(matches!(
        event,
        ClientPaneInputEvent::Key {
            tracks_release: true,
            ..
        }
    ));
    let crate::protocol::keys::host::RawInputEvent::Key(roundtripped) = event.to_raw_input_event()
    else {
        panic!("pane key should remain a key");
    };

    assert!(roundtripped.has_physical_identity());
    let encoded = crate::protocol::keys::encode_terminal_key(
        roundtripped,
        crate::protocol::keys::KeyboardProtocol::Kitty { flags: 8 },
    );
    assert_ne!(encoded, b"/");
    assert!(encoded.starts_with(b"\x1b["));
}

#[test]
fn client_input_large_multilingual_payload_roundtrip() {
    let text = "你好，今天我们测试一段比较长的语音输入。こんにちは。안녕하세요.🙂".repeat(1024);
    assert!(text.len() > 64 * 1024);
    assert!(text.len() < MAX_FRAME_SIZE);
    let msg = ClientMessage::Input {
        data: text.as_bytes().to_vec(),
    };

    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, consumed): (ClientMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();

    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded, msg);
}

#[test]
fn client_shell_keyboard_report_all_roundtrip() {
    let msg = ServerMessage::ClientShellKeyboardReportAll { enabled: true };
    let encoded = bincode::serde::encode_to_vec(&msg, bincode::config::standard()).unwrap();
    let (decoded, _): (ServerMessage, _) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard()).unwrap();
    assert_eq!(msg, decoded);
}
