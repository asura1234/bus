use super::*;

#[test]
fn vti_ctrl_c_record_becomes_ctrl_c_key() {
    assert_eq!(
        translate([key_char('\u{3}')]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('c'),
            modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_ctrl_j_record_preserves_lf_control_key() {
    assert_eq!(
        translate([key_vk_with_unicode(0x4a, '\n', 0x0008)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('j'),
            modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_modifier_only_key_records_do_not_emit_terminal_input() {
    let modifier_records = [
        key_vk_with_utf16_mods(0x11, 0, 0x0008),
        key_vk_with_utf16_mods(0xa2, 0, 0x0008),
        key_vk_with_utf16_mods(0xa3, 0, 0x0004),
        key_vk_with_repeat(0x11, 3),
    ];

    assert!(translate(modifier_records).is_empty());
}

#[test]
fn vti_ctrl_bracket_record_flushes_to_escape_after_idle() {
    let mut translator = WindowsInputTranslator::default();
    assert!(translator
        .translate(key_vk_with_utf16_mods(0xdb, 0x1b, 0x0008))
        .is_empty());
    assert_eq!(
        translator.idle(),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Esc,
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Vt { bytes: vec![0x1b] },
        }]
    );
}

#[test]
fn vti_scan_code_zero_escape_key_record_flushes_after_idle() {
    let mut translator = WindowsInputTranslator::default();
    assert!(translator
        .translate(key_vk_with_scan_unicode(0x1b, 0, '\0', 0))
        .is_empty());
    assert_eq!(
        translator.idle(),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Esc,
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Vt { bytes: vec![0x1b] },
        }]
    );
}

#[test]
fn vti_physical_escape_key_record_keeps_native_ownership_after_idle() {
    for record in [
        WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0x1b,
            virtual_scan_code: 0x01,
            unicode: 0x1b,
            control_key_state: 0,
        },
        WindowsKeyRecord {
            key_down: true,
            repeat_count: 1,
            virtual_key_code: 0x1b,
            virtual_scan_code: 0x02,
            unicode: 0,
            control_key_state: 0,
        },
    ] {
        let mut translator = WindowsInputTranslator::default();
        assert!(translator
            .translate(WindowsInputRecord::Key(record))
            .is_empty());
        assert_eq!(
            translator.idle(),
            vec![crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Esc,
                modifiers: 0,
                kind: crate::protocol::wire::ClientKeyKind::Press,
                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::WindowsConsole { record },
            }]
        );
    }
}

#[test]
fn vti_grouped_escape_down_and_up_keep_native_ownership_record() {
    use crate::protocol::wire::ClientKeyKind::{Press, Release};

    let events = translate_with_provenance([
        key_vk_with_repeat(0x1b, 3),
        key_vk_up_with_scan_unicode(0x1b, 1, '\0', 0),
    ]);

    assert_eq!(
        events
            .iter()
            .map(|event| match event {
                crate::protocol::wire::ClientInputEvent::Key {
                    code: crate::protocol::wire::ClientKeyCode::Esc,
                    kind,
                    repeat_count,
                    source: crate::protocol::wire::ClientKeySource::WindowsConsole { record },
                    ..
                } => (*kind, *repeat_count, record.key_down, record.repeat_count),
                event => panic!("unexpected grouped event: {event:?}"),
            })
            .collect::<Vec<_>>(),
        [(Press, 3, true, 3), (Release, 1, false, 1)]
    );
}

#[test]
fn vti_ctrl_break_record_keeps_semantic_path() {
    assert!(matches!(
        translate_with_provenance([key_vk(0x03, 0x0008)]).as_slice(),
        [crate::protocol::wire::ClientInputEvent::Key { .. }]
    ));
}

#[test]
fn vti_ime_virtual_keys_keep_semantic_path() {
    for vk in [0xe5, 0xe7] {
        assert!(matches!(
            translate_with_provenance([key_vk_with_unicode(vk, 'é', 0)]).as_slice(),
            [crate::protocol::wire::ClientInputEvent::Key {
                generated_text: Some(text),
                source: crate::protocol::wire::ClientKeySource::Synthesized,
                ..
            }] if text == "é"
        ));
    }
}

#[test]
fn vti_modified_escape_remains_semantic() {
    assert_eq!(
        translate([key_vk_with_utf16_mods(0x1b, 0, 0x0010)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Esc,
            modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_repeated_virtual_key_records_emit_repeats() {
    assert_eq!(
        translate([key_vk_with_repeat(0x08, 3)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Backspace,
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,
            repeat_count: 3,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_shift_enter_record_preserves_shift_modifier() {
    assert_eq!(
        translate([key_vk_with_unicode(0x0d, '\r', 0x0010)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Enter,
            modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_key_release_record_is_preserved() {
    assert_eq!(
        translate([key_vk_up_with_unicode(0x4a, 'j', 0x0008)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('j'),
            modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Release,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_synthetic_shift_enter_record_preserves_shift_modifier() {
    assert_eq!(
        translate([key_vk_with_unicode(0, '\r', 0x0010)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Enter,
            modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_repeated_synthetic_shift_enter_emits_repeats() {
    assert_eq!(
        translate([key_vk_with_unicode_repeat(0, '\r', 0x0010, 3)]),
        vec![
            crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::wire::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::Synthesized,
            },
            crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::wire::ClientKeyKind::Repeat,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::Synthesized,
            },
            crate::protocol::wire::ClientInputEvent::Key {
                code: crate::protocol::wire::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::wire::ClientKeyKind::Repeat,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::wire::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_special_key_does_not_emit_incidental_printable_text() {
    assert_eq!(
        translate_with_provenance([WindowsInputRecord::Key(WindowsKeyRecord {
            key_down: true,
            repeat_count: 0,
            virtual_key_code: 0x0d,
            virtual_scan_code: 0,
            unicode: b'a'.into(),
            control_key_state: 0,
        })]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Enter,
            modifiers: 0,
            kind: crate::protocol::wire::ClientKeyKind::Press,
            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_physical_escape_and_open_bracket_remain_separate_keys() {
    let escape = key_vk_with_scan_unicode(0x1b, 0x01, '\x1b', 0);
    let mut translator = WindowsInputTranslator::default();
    assert!(translator.translate(escape).is_empty());
    assert!(translator.translate(key_char('[')).is_empty());
    let events = translator.idle();
    assert!(
        matches!(
            events.as_slice(),
            [
                crate::protocol::wire::ClientInputEvent::Key {
                    code: crate::protocol::wire::ClientKeyCode::Esc,
                    source: crate::protocol::wire::ClientKeySource::WindowsConsole { .. },
                    ..
                },
                crate::protocol::wire::ClientInputEvent::Key {
                    code: crate::protocol::wire::ClientKeyCode::Char('['),
                    ..
                }
            ]
        ),
        "unexpected input events: {events:?}"
    );
}

#[test]
fn vti_modified_physical_escape_stays_semantic_before_sgr_tail() {
    let modified_escape = key_vk_with_scan_unicode(0x1b, 0x01, '\x1b', 0x0010);
    let mut translator = WindowsInputTranslator::default();
    let events = [modified_escape]
        .into_iter()
        .chain("[<0;20;10M".chars().map(key_char))
        .flat_map(|record| translator.translate(record))
        .collect::<Vec<_>>();
    assert!(matches!(
        events.first(),
        Some(crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Esc,
            modifiers,
            source: crate::protocol::wire::ClientKeySource::WindowsConsole { .. },
            ..
        }) if *modifiers == crossterm::event::KeyModifiers::SHIFT.bits()
    ));
}

#[test]
fn vti_real_ctrl_c_record_stays_semantic() {
    assert_eq!(
        translate([key_vk_with_unicode(0x43, '\u{3}', 0x0008)]),
        vec![crate::protocol::wire::ClientInputEvent::Key {
            code: crate::protocol::wire::ClientKeyCode::Char('c'),
            modifiers: crossterm::event::KeyModifiers::CONTROL.bits(),
            kind: crate::protocol::wire::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::wire::ClientKeySource::Synthesized,
        }]
    );
}
