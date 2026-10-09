use super::*;

#[test]
fn vti_control_records_keep_physical_digit_identity() {
    let cases = [
        (0x20, 0x00, ' '),
        (0x31, 0x00, '1'),
        (0xdc, 0x1c, '\\'),
        (0xdd, 0x1d, ']'),
        (0x36, 0x1e, '6'),
        (0xbd, 0x1f, '-'),
    ];

    for (vk, unicode, expected) in cases {
        assert_eq!(
            translate([key_vk_with_utf16_mods(vk, unicode, 0x0008)]),
            vec![crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Char(expected),
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::wire::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::Synthesized,
            }],
            "vk={vk:#x} unicode={unicode:#x}"
        );
    }
}

#[test]
fn vti_ctrl_oem_record_uses_resolved_layout_character() {
    let pressed = WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0xbf,
        virtual_scan_code: 0x35,
        unicode: 0,
        control_key_state: 0x0028,
    };
    let released = WindowsKeyRecord {
        key_down: false,
        ..pressed
    };
    let mut mapper = WindowsInputMapper::default();
    let expected = |record, kind, ch| crate::protocol::wire::ClientInputEvent::Key {
        code: crate::protocol::wire::ClientKeyCode::Char(ch),
        modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
        kind,
        repeat_count: 1,
        generated_text: None,
        source: crate::protocol::wire::ClientKeySource::WindowsConsole { record },
    };
    for (record, kind, ch) in [
        (pressed, crate::protocol::wire::ClientKeyKind::Press, '/'),
        (released, crate::protocol::wire::ClientKeyKind::Release, '/'),
        (pressed, crate::protocol::wire::ClientKeyKind::Press, 'ß'),
    ] {
        assert_eq!(
            mapper.translate_semantic_key_events(record, Some(ch)),
            [expected(record, kind, ch)]
        );
    }
    assert!(mapper
        .translate_semantic_key_events(pressed, None)
        .is_empty());
}

#[test]
fn vti_us_international_dead_key_only_emits_composed_text() {
    fn encode_for_kitty(
        events: Vec<crate::protocol::wire::ClientInputEvent>,
        flags: u16,
    ) -> Vec<u8> {
        events
            .into_iter()
            .flat_map(|event| match event.to_raw_input_event() {
                crate::protocol::keys::host::RawInputEvent::Key(key) => {
                    crate::protocol::keys::encode_terminal_key(
                        key,
                        crate::protocol::keys::KeyboardProtocol::Kitty { flags },
                    )
                }
                crate::protocol::keys::host::RawInputEvent::Text(text) => {
                    text.as_str().as_bytes().to_vec()
                }
                _ => panic!("unexpected event while encoding dead-key input"),
            })
            .collect()
    }

    let dead_press = WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x36,
        virtual_scan_code: 0x07,
        unicode: 0,
        control_key_state: 0x0030,
    };
    let dead_release = WindowsKeyRecord {
        key_down: false,
        ..dead_press
    };
    let composed_press = WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x45,
        virtual_scan_code: 0x12,
        unicode: 'ê' as u16,
        control_key_state: 0x0020,
    };
    let composed_release = WindowsKeyRecord {
        key_down: false,
        unicode: 'e' as u16,
        ..composed_press
    };

    let mut translator = WindowsInputTranslator::default();
    for (record, kind) in [
        (dead_press, crate::protocol::wire::ClientKeyKind::Press),
        (dead_release, crate::protocol::wire::ClientKeyKind::Release),
    ] {
        let events = translator.translate(WindowsInputRecord::Key(record));
        assert!(matches!(
            events.as_slice(),
            [crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Char('6'),
                modifiers,
                kind: actual_kind,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::WindowsConsole {
                    record: actual_record,
                },
                ..
            }] if *modifiers == crossterm::event::KeyModifiers::SHIFT.bits()
                && *actual_kind == kind
                && *actual_record == record
        ));
        for flags in [1, 31] {
            assert!(encode_for_kitty(events.clone(), flags).is_empty());
        }
    }

    let events = translator.translate(WindowsInputRecord::Key(composed_press));
    assert!(matches!(
        events.as_slice(),
        [crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('ê'),
            kind: crate::protocol::wire::ClientKeyKind::Press,
            source: crate::protocol::wire::ClientKeySource::WindowsConsole {
                record: actual_record,
            },
            ..
        }] if *actual_record == composed_press
    ));
    assert_eq!(encode_for_kitty(events, 1), "ê".as_bytes());

    let events = translator.translate(WindowsInputRecord::Key(composed_release));
    assert!(encode_for_kitty(events, 1).is_empty());
    assert!(translator.idle().is_empty());

    let ordinary_shifted = WindowsKeyRecord {
        unicode: '^' as u16,
        ..dead_press
    };
    let events =
        WindowsInputTranslator::default().translate(WindowsInputRecord::Key(ordinary_shifted));
    assert_eq!(encode_for_kitty(events, 1), b"^");
}

#[test]
fn vti_win32_input_mode_non_us_shifted_text_preserves_generated_text() {
    let pressed = WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x37,
        virtual_scan_code: 0x08,
        unicode: b'/'.into(),
        control_key_state: 0x0010,
    };
    let released = WindowsKeyRecord {
        key_down: false,
        ..pressed
    };
    let mut records = win32_input_mode_encoded_record(pressed);
    records.extend(win32_input_mode_encoded_record(released));

    let events = translate_with_provenance(records);
    assert_eq!(
        events,
        vec![
            crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Char('/'),
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::wire::ClientKeyKind::Press,
                repeat_count: 1,
                generated_text: Some("/".into()),
                source: crate::protocol::wire::ClientKeySource::WindowsConsole { record: pressed },
            },
            crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Char('/'),
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::wire::ClientKeyKind::Release,
                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::WindowsConsole { record: released },
            },
        ]
    );

    let crate::protocol::keys::host::RawInputEvent::Key(key) = events[0].to_raw_input_event()
    else {
        panic!("expected translated key");
    };
    assert_eq!(
        crate::protocol::keys::encode_terminal_key(
            key.clone(),
            crate::protocol::keys::KeyboardProtocol::Legacy
        ),
        b"/"
    );
    assert_eq!(
        crate::protocol::keys::encode_terminal_key(
            key.clone(),
            crate::protocol::keys::KeyboardProtocol::Kitty { flags: 7 },
        ),
        b"/"
    );
    assert_eq!(
        crate::protocol::keys::encode_terminal_key(
            key,
            crate::protocol::keys::KeyboardProtocol::Kitty { flags: 15 },
        ),
        b"\x1b[47;2:1u"
    );
}

#[test]
fn vti_native_layout_text_and_command_modifiers_encode_by_role() {
    let cases = [
        (
            "plain unicode",
            0x4c,
            0x26,
            'λ' as u16,
            0,
            0,
            None,
            "λ".as_bytes(),
        ),
        (
            "shifted latin",
            0x41,
            0x1e,
            'A' as u16,
            0x0010,
            crossterm::event::KeyModifiers::SHIFT.bits(),
            Some("A"),
            b"A".as_slice(),
        ),
        (
            "shifted non-ascii",
            0x4c,
            0x26,
            'Λ' as u16,
            0x0010,
            crossterm::event::KeyModifiers::SHIFT.bits(),
            Some("Λ"),
            "Λ".as_bytes(),
        ),
        (
            "altgr",
            0x45,
            0x12,
            '€' as u16,
            0x0009,
            0,
            None,
            "€".as_bytes(),
        ),
        (
            "shift altgr",
            0x37,
            0x08,
            '{' as u16,
            0x0019,
            crossterm::event::KeyModifiers::SHIFT.bits(),
            Some("{"),
            b"{".as_slice(),
        ),
        (
            "left alt command",
            0x41,
            0x1e,
            'a' as u16,
            0x0002,
            crossterm::event::KeyModifiers::ALT.bits(),
            None,
            b"\x1ba".as_slice(),
        ),
        (
            "control command",
            0x41,
            0x1e,
            0x01,
            0x0008,
            crossterm::event::KeyModifiers::CONTROL.bits(),
            None,
            b"\x01".as_slice(),
        ),
    ];

    for (name, virtual_key_code, virtual_scan_code, unicode, state, modifiers, text, expected) in
        cases
    {
        let events = translate_with_provenance(win32_input_mode_encoded_record(WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code,
            virtual_scan_code,
            unicode,
            control_key_state: state,
        }));
        let [event] = events.as_slice() else {
            panic!("{name}: expected one translated event, got {events:?}");
        };
        let crate::protocol::keys::host::RawInputEvent::Key(key) = event.to_raw_input_event()
        else {
            panic!("{name}: expected translated key");
        };

        assert_eq!(key.modifiers.bits(), modifiers, "{name}: modifiers");
        assert_eq!(key.generated_text.as_deref(), text, "{name}: text");
        assert_eq!(
            crate::protocol::keys::encode_terminal_key(
                key,
                crate::protocol::keys::KeyboardProtocol::Legacy
            ),
            expected,
            "{name}: encoding"
        );
    }
}

#[test]
fn vti_right_alt_special_key_preserves_alt_modifier() {
    assert_eq!(
        translate([key_vk(0x26, 0x0001)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Up,
            modifiers: crossterm::event::KeyModifiers::ALT.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_altgr_printable_key_is_not_terminal_alt_prefix() {
    assert_eq!(
        translate([key_vk_with_unicode(0x32, '@', 0x0001 | 0x0008)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('@'),
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_alt_code_unicode_on_alt_release_is_preserved() {
    assert_eq!(
        translate([key_vk_up_with_unicode(0x12, 'é', 0)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('é'),
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,
            repeat_count: 1,
            generated_text: Some("é".into()),
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_semantic_surrogate_pair_preserves_emoji() {
    let mut translator = WindowsInputTranslator::default();
    let high = key_vk_with_utf16(0xe7, 0xd83d);
    let low = key_vk_with_utf16(0xe7, 0xde42);

    assert!(translator.translate(high).is_empty());
    assert_eq!(
        translator.translate(low),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('🙂'),
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_surrogate_pair_paste_preserves_emoji() {
    let mut translator = WindowsInputTranslator::default();
    let records = [
        key_char('\x1b'),
        key_char('['),
        key_char('2'),
        key_char('0'),
        key_char('0'),
        key_char('~'),
        key_vk_with_utf16(0, 0xd83d),
        key_vk_with_utf16(0, 0xde42),
        key_char('\x1b'),
        key_char('['),
        key_char('2'),
        key_char('0'),
        key_char('1'),
        key_char('~'),
    ];

    let events = records
        .into_iter()
        .flat_map(|record| translator.translate(record))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![crate::protocol::wire::ClientInputEvent::Paste {
            text: "🙂".into()
        }]
    );
}

#[test]
fn vti_nonzero_vk_surrogate_pair_inside_paste_preserves_emoji() {
    let mut translator = WindowsInputTranslator::default();
    let high = key_vk_with_utf16(0xe7, 0xd83d);
    let low = key_vk_with_utf16(0xe7, 0xde42);
    let records = "\x1b[200~"
        .chars()
        .map(key_char)
        .chain([high, low])
        .chain("\x1b[201~".chars().map(key_char));

    let events = records
        .into_iter()
        .flat_map(|record| translator.translate(record))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        vec![crate::protocol::wire::ClientInputEvent::Paste {
            text: "🙂".into()
        }]
    );
}

#[test]
fn vti_mouse_press_after_focus_return_does_not_require_an_unseen_release() {
    use crate::protocol::wire::{ClientInputEvent, ClientMouseButton, ClientMouseKind};

    let mut translator = WindowsInputTranslator::default();
    let pressed = WindowsInputRecord::Mouse(WindowsMouseRecord {
        x: 3,
        y: 2,
        button_state: 0x0001,
        control_key_state: 0,
        event_flags: 0,
    });
    let expected = vec![ClientInputEvent::Mouse {
        kind: ClientMouseKind::Down(ClientMouseButton::Left),
        column: 3,
        row: 2,
        modifiers: 0,
    }];
    assert_eq!(translator.translate(pressed), expected);
    assert_eq!(
        translator.translate(WindowsInputRecord::Focus(false)),
        vec![ClientInputEvent::FocusLost]
    );

    // Console mouse records require focus and an in-window pointer, so a release
    // after switching away does not reach the console input buffer.
    assert_eq!(
        translator.translate(WindowsInputRecord::Focus(true)),
        vec![ClientInputEvent::FocusGained]
    );
    assert_eq!(
        translator.translate(pressed),
        expected,
        "the first click after focus returns must start a new mouse gesture"
    );
}
