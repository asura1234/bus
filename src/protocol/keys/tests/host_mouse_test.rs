use super::*;

#[test]
fn parses_sgr_mouse() {
    let (RawInputEvent::Mouse(mouse), consumed) = extract_one_event(b"\x1b[<0;20;10M").unwrap()
    else {
        panic!("expected mouse");
    };
    assert_eq!(consumed, 11);
    assert_eq!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
    assert_eq!(mouse.column, 19);
    assert_eq!(mouse.row, 9);
    assert_eq!(mouse.modifiers, KeyModifiers::empty());
}

#[test]
fn parses_default_mouse_encoding() {
    let mut framer = RawInputFramer::default();
    let events = framer.push(b"\x1b[MCN1");
    let [RawInputEvent::Mouse(mouse)] = events.as_slice() else {
        panic!("expected one mouse event");
    };
    assert_eq!(mouse.kind, MouseEventKind::Moved);
    assert_eq!((mouse.column, mouse.row), (45, 16));
    assert_eq!(mouse.modifiers, KeyModifiers::empty());
}

#[test]
fn rejected_default_mouse_frame_preserves_trailing_input() {
    let mut framer = RawInputFramer::default();
    let events = framer.push(b"\x1b[M\x82AAx");

    assert_eq!(events.len(), 2);
    assert!(matches!(events[0], RawInputEvent::Unsupported));
    assert_raw_key(
        events.into_iter().nth(1).unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn parses_extended_button_drag_as_mouse_motion() {
    for input in [
        b"\x1b[<160;20;10M".as_slice(),
        b"\x1b[<161;20;10M".as_slice(),
    ] {
        let (RawInputEvent::Mouse(mouse), _) = extract_one_event(input).unwrap() else {
            panic!("expected mouse");
        };
        assert_eq!(mouse.kind, MouseEventKind::Moved);
        assert_eq!((mouse.column, mouse.row), (19, 9));
    }
}

#[test]
fn parses_sgr_mouse_observable_modifiers() {
    let cases = [
        (b"\x1b[<8;20;10M".as_slice(), KeyModifiers::ALT),
        (b"\x1b[<16;20;10M".as_slice(), KeyModifiers::CONTROL),
        (
            b"\x1b[<24;20;10M".as_slice(),
            KeyModifiers::ALT | KeyModifiers::CONTROL,
        ),
    ];

    for (input, expected) in cases {
        let (RawInputEvent::Mouse(mouse), _) = extract_one_event(input).unwrap() else {
            panic!("expected mouse");
        };
        assert_eq!(mouse.modifiers, expected);
        assert!(!mouse.modifiers.contains(KeyModifiers::SUPER));
    }
}

#[test]
fn escape_followed_by_sgr_mouse_before_flush_does_not_emit_text() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"[<65;43;26M");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 42,
            row: 25,
            ..
        })
    ));
}

#[test]
fn lone_escape_then_complete_sgr_mouse_report_emits_both_events() {
    for report in [b"\x1b[<35;10;20M".as_slice(), b"\x1b[<35;10;20m".as_slice()] {
        let mut framer = RawInputFramer::default();

        assert!(framer.push(b"\x1b").is_empty());
        let events = framer.push(report);

        assert_eq!(events.len(), 2);
        let mut events = events.into_iter();
        assert_raw_key(events.next().unwrap(), KeyCode::Esc, KeyModifiers::empty());
        assert!(matches!(
            events.next().unwrap(),
            RawInputEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Moved,
                column: 9,
                row: 19,
                ..
            })
        ));
        assert!(framer.flush_timeout().is_empty());
    }
}

#[test]
fn lone_escape_then_default_mouse_report_emits_both_events() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"\x1b[MCN1");

    assert_eq!(events.len(), 2);
    let mut events = events.into_iter();
    assert_raw_key(events.next().unwrap(), KeyCode::Esc, KeyModifiers::empty());
    assert!(matches!(
        events.next().unwrap(),
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 45,
            row: 16,
            ..
        })
    ));
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn sgr_mouse_sequence_split_after_button_prefix_is_reassembled_before_timeout() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[<3").is_empty());
    let events = framer.push(b"5;58;30M");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 57,
            row: 29,
            ..
        })
    ));
}

#[test]
fn timed_out_split_sgr_mouse_tail_is_discarded_and_following_input_is_preserved() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[<3").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(b"5;58;30Mx");

    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn timed_out_sgr_mouse_discard_state_clears_at_quiescence() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b[<3").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"M"), vec![b"M".to_vec()]);
}

#[test]
fn sgr_mouse_tail_after_lone_escape_timeout_is_discarded() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let timeout_events = framer.flush_timeout();
    assert_eq!(timeout_events.len(), 1);
    assert_raw_key(
        timeout_events.into_iter().next().unwrap(),
        KeyCode::Esc,
        KeyModifiers::empty(),
    );

    assert!(framer.push(b"[<65;43;26M").is_empty());
}

#[test]
fn input_after_discarded_complete_sgr_mouse_tail_is_preserved() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout().len(), 1);
    let events = framer.push(b"[<65;43;26Mx");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn invalid_orphaned_sgr_mouse_tail_after_escape_timeout_is_preserved() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout().len(), 1);

    let events = framer.push(b"[<x");

    assert_eq!(events.len(), 3);
    assert_raw_key(
        events.into_iter().last().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}

#[test]
fn double_split_sgr_mouse_tail_after_lone_escape_timeout_is_discarded() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout().len(), 1);

    assert!(framer.push(b"[<65;4").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(b"3;26Mx");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('x'),
        KeyModifiers::empty(),
    );
}
