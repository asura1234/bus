#[test]
fn paste_text_uses_windows_line_endings() {
    assert_eq!(
        super::super::prepare_paste_text_for_pty_platform("one\ntwo\r\nthree\rfour".to_owned()),
        "one\r\ntwo\r\nthree\rfour"
    );
}

#[test]
fn windows_conpty_native_encoder_uses_canonical_phase_and_repeat_count() {
    let key = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Char('7'),
        crossterm::event::KeyModifiers::CONTROL,
    )
    .with_windows_record(crate::input::WindowsKeyRecord {
        key_down: true,
        repeat_count: 3,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: 0,
        control_key_state: 0x0008,
    });

    assert_eq!(
        super::super::encode_windows_conpty_fallback(&key),
        Some(b"\x1b[55;8;0;1;8;3_".to_vec())
    );
    let mut release = key.with_kind(crossterm::event::KeyEventKind::Release);
    release.repeat_count = 3;
    assert_eq!(
        super::super::encode_windows_conpty_fallback(&release),
        Some(b"\x1b[55;8;0;0;8;1_".to_vec())
    );
}

#[test]
fn windows_conpty_native_encoder_preserves_semantic_escape_fallback() {
    let escape = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::empty(),
    );

    assert_eq!(
        super::super::encode_windows_conpty_fallback(&escape),
        Some(b"\x1b[27;1;27;1;0;1_\x1b[27;1;27;0;0;1_".to_vec())
    );
    assert_eq!(
        super::super::encode_windows_conpty_fallback(
            &escape
                .clone()
                .with_kind(crossterm::event::KeyEventKind::Repeat),
        ),
        None
    );
    assert_eq!(
        super::super::encode_windows_conpty_fallback(
            &escape
                .clone()
                .with_kind(crossterm::event::KeyEventKind::Release),
        ),
        None
    );
    assert_eq!(
        super::super::encode_windows_conpty_fallback(&escape.clone().with_vt_bytes(vec![27])),
        None
    );
    assert_eq!(
        super::super::encode_windows_conpty_fallback(&crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Esc,
            crossterm::event::KeyModifiers::ALT,
        ),),
        None
    );
}

#[test]
fn windows_conpty_native_encoder_preserves_semantic_shift_enter_fallback() {
    let shift_enter = crate::input::TerminalKey::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::SHIFT,
    );

    assert_eq!(
        super::super::encode_windows_conpty_fallback(&shift_enter),
        Some(b"\x1b[13;28;13;1;16;1_".to_vec())
    );
    assert_eq!(
        super::super::encode_windows_conpty_fallback(&crate::input::TerminalKey::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::empty(),
        )),
        None
    );
}
