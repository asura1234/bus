use super::*;

#[test]
fn vti_bracketed_paste_records_emit_single_paste() {
    let records = "\x1b[200~alpha\rbravo\rcharlie\x1b[201~"
        .chars()
        .map(key_char);

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alpha\rbravo\rcharlie".into(),
        }]
    );
}

#[test]
fn vti_remote_bracketed_paste_decodes_reported_enter_records() {
    let records = concat!(
        "\x1b[200~ - line one",
        "\x1b[13;28;13;1;0;1_\x1b[13;28;13;0;0;1_",
        "  - line two",
        "\x1b[13;28;13;1;0;1_\x1b[13;28;13;0;0;1_",
        "  - line three\x1b[201~",
    )
    .chars()
    .map(key_char);

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: " - line one\r  - line two\r  - line three".into(),
        }]
    );
}

#[test]
fn vti_remote_paste_keeps_incomplete_enter_report_pairs_opaque() {
    let records = concat!(
        "\x1b[200~before",
        "\x1b[13;28;13;1;0;1_middle\x1b[13;28;13;0;0;1_",
        "after\x1b[201~",
    )
    .chars()
    .map(key_char);

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: concat!(
                "before",
                "\x1b[13;28;13;1;0;1_middle\x1b[13;28;13;0;0;1_",
                "after",
            )
            .into(),
        }]
    );
}

#[test]
fn vti_incomplete_bracketed_paste_waits_for_terminator() {
    let mut translator = WindowsInputTranslator::default();
    for record in "\x1b[200~alpha\r".chars().map(key_char) {
        assert!(translator.translate(record).is_empty());
    }

    let mut events = Vec::new();
    for record in "bravo\x1b[201~".chars().map(key_char) {
        events.extend(translator.translate(record));
    }
    assert_eq!(
        events,
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alpha\rbravo".into(),
        }]
    );
}

#[test]
fn vti_physical_escape_flushes_older_raw_escape_first() {
    let physical_escape = key_vk_with_scan_unicode(0x1b, 0x01, '\x1b', 0);
    let mut translator = WindowsInputTranslator::default();
    assert!(translator
        .translate(key_vk_with_scan_unicode(0x1b, 0, '\0', 0))
        .is_empty());
    assert!(matches!(
        translator.translate(physical_escape).as_slice(),
        [crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Esc,
            source: crate::protocol::ClientKeySource::Vt { .. },
            ..
        }]
    ));
    assert!(matches!(
        translator.idle().as_slice(),
        [crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Esc,
            source: crate::protocol::ClientKeySource::WindowsConsole { .. },
            ..
        }]
    ));
}

#[test]
fn vti_enter_outside_paste_becomes_enter_key() {
    assert_eq!(
        translate([key_char('\r')]),
        vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Enter,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_synthetic_shift_enter_inside_paste_stays_in_paste_payload() {
    let mut records: Vec<_> = "\x1b[200~alpha".chars().map(key_char).collect();
    records.push(key_vk_with_unicode(0, '\r', 0x0010));
    records.extend("bravo\x1b[201~".chars().map(key_char));

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alpha\rbravo".into(),
        }]
    );
}

#[test]
fn vti_vk_return_inside_paste_stays_in_paste_payload() {
    let mut records: Vec<_> = "\x1b[200~alpha".chars().map(key_char).collect();
    records.push(key_vk_with_unicode(0x0d, '\r', 0));
    records.extend("bravo\x1b[201~".chars().map(key_char));

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alpha\rbravo".into(),
        }]
    );
}

#[test]
fn vti_vk_return_release_inside_paste_is_suppressed() {
    let mut records: Vec<_> = "\x1b[200~alpha".chars().map(key_char).collect();
    records.push(key_vk_up_with_unicode(0x0d, '\r', 0));
    records.extend("bravo\x1b[201~".chars().map(key_char));

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Paste {
            text: "alphabravo".into(),
        }]
    );
}

#[test]
fn vti_ctrl_bracket_escape_starts_mouse_sequence() {
    let records = [key_vk_with_utf16_mods(0xdb, 0x1b, 0x0008)]
        .into_iter()
        .chain("[<35;48;26M".chars().map(key_char));

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Mouse {
            kind: crate::protocol::ClientMouseKind::Moved,
            column: 47,
            row: 25,
            modifiers: 0,
        }]
    );
}

#[test]
fn vti_scan_code_zero_escape_starts_mouse_sequence() {
    let records = [key_vk_with_scan_unicode(0x1b, 0, '\0', 0)]
        .into_iter()
        .chain("[<35;48;26M".chars().map(key_char));

    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Mouse {
            kind: crate::protocol::ClientMouseKind::Moved,
            column: 47,
            row: 25,
            modifiers: 0,
        }]
    );
}

#[test]
fn vti_physical_escape_prefix_still_parses_sgr_mouse_reports() {
    let escape = key_vk_with_scan_unicode(0x1b, 0x01, '\x1b', 0);
    let records = [escape]
        .into_iter()
        .chain("[<5;36;21M".chars().map(key_char))
        .chain([escape])
        .chain("[<5;76;28M".chars().map(key_char));

    assert_eq!(
        translate(records),
        vec![
            crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Down(
                    crate::protocol::ClientMouseButton::Middle,
                ),
                column: 35,
                row: 20,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
            },
            crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Down(
                    crate::protocol::ClientMouseButton::Middle,
                ),
                column: 75,
                row: 27,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
            },
        ]
    );
}

#[test]
fn vti_escape_arrow_sequence_becomes_arrow_key() {
    let records = "\x1b[A".chars().map(key_char);
    assert_eq!(
        translate(records),
        vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Up,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::ClientKeySource::Synthesized,
        }]
    );
}

#[test]
fn vti_lone_escape_flushes_only_after_idle() {
    let mut translator = WindowsInputTranslator::default();
    assert!(translator.translate(key_char('\x1b')).is_empty());
    assert_eq!(
        translator.idle(),
        vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Esc,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,

            repeat_count: 1,
            generated_text: None,
            source: crate::protocol::ClientKeySource::Vt { bytes: vec![0x1b] },
        }]
    );
}

#[test]
fn vti_synthetic_shift_enter_after_lone_escape_stays_semantic() {
    let mut translator = WindowsInputTranslator::default();
    assert!(translator.translate(key_char('\x1b')).is_empty());
    assert_eq!(
        translator.translate(key_vk_with_unicode(0, '\r', 0x0010)),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Esc,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Vt { bytes: vec![0x1b] },
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Enter,
                modifiers: crossterm::event::KeyModifiers::SHIFT.bits(),
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_semantic_event_flushes_pending_raw_first() {
    let mut translator = WindowsInputTranslator::default();
    assert!(translator.translate(key_char('\x1b')).is_empty());
    assert_eq!(
        semantic_only(translator.translate(key_vk(0x26, 0))),
        vec![
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Esc,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
            crate::protocol::ClientInputEvent::Key {
                code: crate::protocol::ClientKeyCode::Up,
                modifiers: 0,
                kind: crate::protocol::ClientKeyKind::Press,

                repeat_count: 1,
                generated_text: None,
                source: crate::protocol::ClientKeySource::Synthesized,
            },
        ]
    );
}

#[test]
fn vti_mouse_press_release_records_are_preserved() {
    let events = translate([
        WindowsInputRecord::Mouse(WindowsMouseRecord {
            x: 3,
            y: 4,
            button_state: 0x0001,
            control_key_state: 0,
            event_flags: 0,
        }),
        WindowsInputRecord::Mouse(WindowsMouseRecord {
            x: 3,
            y: 4,
            button_state: 0,
            control_key_state: 0,
            event_flags: 0,
        }),
    ]);

    assert_eq!(
        events,
        vec![
            crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Down(
                    crate::protocol::ClientMouseButton::Left,
                ),
                column: 3,
                row: 4,
                modifiers: 0,
            },
            crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Up(
                    crate::protocol::ClientMouseButton::Left,
                ),
                column: 3,
                row: 4,
                modifiers: 0,
            },
        ]
    );
}

#[test]
fn vti_horizontal_wheel_records_match_crossterm_direction() {
    let events = translate([
        WindowsInputRecord::Mouse(WindowsMouseRecord {
            x: 3,
            y: 4,
            button_state: 0xffff_0000,
            control_key_state: 0,
            event_flags: 0x0008,
        }),
        WindowsInputRecord::Mouse(WindowsMouseRecord {
            x: 3,
            y: 4,
            button_state: 0x0001_0000,
            control_key_state: 0,
            event_flags: 0x0008,
        }),
    ]);

    assert_eq!(
        events,
        vec![
            crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::ScrollLeft,
                column: 3,
                row: 4,
                modifiers: 0,
            },
            crate::protocol::ClientInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::ScrollRight,
                column: 3,
                row: 4,
                modifiers: 0,
            },
        ]
    );
}

#[test]
fn vti_focus_records_are_preserved() {
    assert_eq!(
        translate([
            WindowsInputRecord::Focus(true),
            WindowsInputRecord::Focus(false)
        ]),
        vec![
            crate::protocol::ClientInputEvent::FocusGained,
            crate::protocol::ClientInputEvent::FocusLost,
        ]
    );
}
