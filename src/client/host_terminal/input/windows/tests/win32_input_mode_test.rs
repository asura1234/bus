use super::*;

#[test]
fn vti_win32_input_mode_bracketed_paste_key_records_emit_single_paste() {
    let records = win32_input_mode_encoded_key_bytes(
        b"\x1b[200~About\ragent multiplexer that lives in your terminal.\x1b[201~",
    );

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "About\ragent multiplexer that lives in your terminal.".into(),
        }]
    );
}

#[test]
fn vti_win32_input_mode_decoded_paste_handles_shift_repeats_and_releases() {
    let mut records = win32_input_mode_encoded_key_bytes(b"\x1b[200~");
    records.extend(win32_input_mode_encoded_record(WindowsKeyRecord {
        key_down: true,
        repeat_count: 2,
        virtual_key_code: 0x41,
        virtual_scan_code: 30,
        unicode: b'A'.into(),
        control_key_state: 0x0010,
    }));
    records.extend(win32_input_mode_encoded_record(WindowsKeyRecord {
        key_down: false,
        repeat_count: 1,
        virtual_key_code: 0x41,
        virtual_scan_code: 30,
        unicode: b'A'.into(),
        control_key_state: 0x0010,
    }));
    records.extend(win32_input_mode_encoded_key_bytes(b"\x1b[201~"));

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste { text: "AA".into() }]
    );
}

#[test]
fn vti_win32_input_mode_marks_ime_commit_as_text() {
    for control_key_state in [0, 0x0010, 0x0008] {
        let records = win32_input_mode_encoded_record(WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0,
            virtual_scan_code: 0,
            unicode: '你' as u16,
            control_key_state,
        });

        let events = translate(records);
        match events.as_slice() {
            [crate::protocol::ClientInputEvent::TextCommit(text)] => {
                assert_eq!(text, "你");
            }
            [crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('你'),
                repeat_count: 1,
                generated_text: Some(text),
                source: crate::protocol::ClientKeySource::Synthesized,
                ..
            }] => assert_eq!(text, "你"),
            other => panic!("unexpected VK=0 result: {other:?}"),
        }
    }
}

#[test]
fn vti_win32_input_mode_decoded_paste_flag_clears_after_raw_completion() {
    let mut records = win32_input_mode_encoded_key_bytes(b"\x1b[200~");
    records.extend("one\x1b[201~".chars().map(key_char));
    records.extend(
        "\x1b[200~x\x1b[65;30;97;1;0;1_y\x1b[201~"
            .chars()
            .map(key_char),
    );

    assert_eq!(
        translate(records),
        vec![
            crate::protocol::ClientInputEvent::Paste { text: "one".into() },
            crate::protocol::ClientInputEvent::Paste {
                text: "x\x1b[65;30;97;1;0;1_y".into(),
            },
        ]
    );
}

#[test]
fn vti_win32_input_mode_modifier_only_key_records_do_not_emit_terminal_input() {
    let records = "\x1b[17;0;0;1;8;3_".chars().map(key_char);

    assert!(translate(records).is_empty());
}

#[test]
fn vti_win32_input_mode_key_release_does_not_emit_raw_text() {
    let records = "\x1b[0;0;97;0;0;1_".chars().map(key_char);
    assert!(translate(records).is_empty());
}

#[test]
fn vti_win32_input_mode_shift_enter_preserves_shift_modifier() {
    let records =
        "\x1b[16;42;0;1;16;1_\x1b[13;28;13;1;16;1_\x1b[13;28;13;0;16;1_\x1b[16;42;0;0;0;1_"
            .chars()
            .map(key_char);

    assert_eq!(
        translate(records),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::ClientKeyKind::Release,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_win32_input_mode_plain_enter_stays_plain_enter() {
    let records = "\x1b[13;28;13;1;0;1_\x1b[13;28;13;0;0;1_"
        .chars()
        .map(key_char);

    assert_eq!(
        translate(records),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Release,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_win32_input_mode_backspace_stays_backspace() {
    let records = "\x1b[8;14;8;1;0;1_\x1b[8;14;8;0;0;1_".chars().map(key_char);

    assert_eq!(
        translate(records),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Backspace,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Backspace,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Release,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_win32_input_mode_ctrl_j_preserves_lf_control_key() {
    let records = "\x1b[74;36;10;1;8;1_\x1b[74;36;10;0;8;1_"
        .chars()
        .map(key_char);

    assert_eq!(
        translate(records),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('j'),
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('j'),
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Release,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_alacritty_ctrl_j_return_lf_record_preserves_lf_control_key() {
    assert_eq!(
        translate([
            key_vk_with_scan_unicode(0x0d, 0x24, '\n', 0x0008),
            key_vk_up_with_scan_unicode(0x0d, 0x24, '\n', 0x0008),
        ]),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('j'),
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Char('j'),
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Release,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_ctrl_enter_return_lf_record_preserves_ctrl_enter() {
    assert_eq!(
        translate([
            key_vk_with_scan_unicode(0x0d, 0x1c, '\n', 0x0008),
            key_vk_up_with_scan_unicode(0x0d, 0x1c, '\n', 0x0008),
        ]),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
                kind: crate::protocol::ClientKeyKind::Release,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_win32_input_mode_printable_keys_preserve_physical_records() {
    assert_eq!(
        translate(win32_input_mode_encoded_record(WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0,
            virtual_scan_code: 0,
            unicode: b'a'.into(),
            control_key_state: 0,
        })),
        vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Char('a'),
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,
            repeat_count: 1,
            generated_text: Some("a".into()),
            source: crate::protocol::ClientKeySource::Synthesized,
        }]
    );

    use crate::protocol::ClientKeyKind::{Press, Release};
    for repeat_count in [1, 3] {
        let mut records = win32_input_mode_encoded_record(WindowsKeyRecord {
            key_down: true,
            repeat_count,
            virtual_key_code: 0x41,
            virtual_scan_code: 30,
            unicode: b'a'.into(),
            control_key_state: 0,
        });
        records.extend(win32_input_mode_encoded_record(WindowsKeyRecord {
            key_down: false,
            repeat_count: 1,
            virtual_key_code: 0x41,
            virtual_scan_code: 30,
            unicode: b'a'.into(),
            control_key_state: 0,
        }));

        assert_eq!(
            translate_with_provenance(records)
                .iter()
                .map(|event| match event {
                    crate::protocol::ClientInputEvent::Key {
                        code: crate::protocol::ClientKeyCode::Char('a'),
                        modifiers: 0,
                        kind,
                        repeat_count: event_repeat_count,
                        generated_text: None,
                        source:
                            crate::protocol::ClientKeySource::WindowsConsole {
                                record:
                                    WindowsKeyRecord {
                                        repeat_count: record_repeat_count,
                                        virtual_key_code: 0x41,
                                        virtual_scan_code: 30,
                                        unicode,
                                        control_key_state: 0,
                                        key_down,
                                    },
                            },
                    } if *unicode == u16::from(b'a') => {
                        (*kind, *event_repeat_count, *key_down, *record_repeat_count)
                    }
                    event => panic!("unexpected printable key event: {event:?}"),
                })
                .collect::<Vec<_>>(),
            [
                (Press, repeat_count, true, repeat_count),
                (Release, 1, false, 1)
            ],
            "repeat_count={repeat_count}"
        );
    }
}

#[test]
fn vti_win32_input_mode_sequence_inside_bracketed_paste_stays_payload() {
    let records = "\x1b[200~alpha\x1b[13;28;13;1;16;1_bravo\x1b[201~"
        .chars()
        .map(key_char);

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alpha\x1b[13;28;13;1;16;1_bravo".into(),
        }]
    );
}

#[test]
fn vti_win32_input_mode_raw_sequence_inside_bracketed_paste_stays_payload() {
    let records = "\x1b[200~alpha\x1b[0;0;97;1;0;1_bravo\x1b[201~"
        .chars()
        .map(key_char);

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alpha\x1b[0;0;97;1;0;1_bravo".into(),
        }]
    );
}

#[test]
fn vti_win32_input_mode_leaves_mouse_sequence_for_raw_parser() {
    let records = "\x1b[<0;3;4M".chars().map(key_char);

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Mouse {
            kind: crate::protocol::ClientMouseKind::Down(crate::protocol::ClientMouseButton::Left,),
            column: 2,
            row: 3,
            modifiers: 0,
        }]
    );
}

#[test]
fn vti_win32_input_mode_physical_escape_keeps_native_ownership_after_idle() {
    let record = WindowsKeyRecord {
        key_down: true,
        repeat_count: 1,
        virtual_key_code: 0x1b,
        virtual_scan_code: 0x01,
        unicode: 0x1b,
        control_key_state: 0,
    };
    let records = win32_input_mode_encoded_record(record);
    let mut translator = WindowsInputTranslator::default();

    assert!(records
        .into_iter()
        .flat_map(|record| translator.translate(record))
        .collect::<Vec<_>>()
        .is_empty());
    assert_eq!(
        translator.idle(),
        vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Esc,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,
            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::ClientKeySource::WindowsConsole { record },
        }]
    );
}

#[test]
fn vti_win32_input_mode_encoded_mouse_sequence() {
    assert_eq!(
        translate(win32_input_mode_encoded_raw_bytes(b"\x1b[<35;48;26M")),
        vec![crate::protocol::ClientInputEvent::Mouse {
            kind: crate::protocol::ClientMouseKind::Moved,
            column: 47,
            row: 25,
            modifiers: 0,
        }]
    );
}
