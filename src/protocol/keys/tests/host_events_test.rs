use super::*;

#[test]
fn parses_kitty_shift_letter_release() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[108:76;2:3u").unwrap()
    else {
        panic!("expected key");
    };
    assert_eq!(consumed, 13);
    assert_eq!(key.code, KeyCode::Char('l'));
    assert_eq!(key.modifiers, KeyModifiers::SHIFT);
    assert_eq!(key.kind, KeyEventKind::Release);
    assert_eq!(key.shifted_codepoint, Some('L' as u32));
}

#[test]
fn parses_bracketed_paste() {
    let (RawInputEvent::Paste(text), consumed) =
        extract_one_event(b"\x1b[200~hello\x1b[201~rest").unwrap()
    else {
        panic!("expected paste");
    };
    assert_eq!(text, "hello");
    assert_eq!(consumed, 17);
}

#[test]
fn parses_legacy_up_arrow() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[A").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 3);
    assert_eq!(key.code, KeyCode::Up);
}

#[test]
fn parses_outer_focus_events() {
    let (event, consumed) = extract_one_event(b"\x1b[I").unwrap();
    assert_eq!(consumed, 3);
    assert!(matches!(event, RawInputEvent::OuterFocusGained));

    let (event, consumed) = extract_one_event(b"\x1b[O").unwrap();
    assert_eq!(consumed, 3);
    assert!(matches!(event, RawInputEvent::OuterFocusLost));
}

#[test]
fn parses_xterm_alt_up_arrow() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[1;3A").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 6);
    assert_eq!(key.code, KeyCode::Up);
    assert_eq!(key.modifiers, KeyModifiers::ALT);
}

#[test]
fn parses_legacy_alt_backspace() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b\x7f").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 2);
    assert_eq!(key.code, KeyCode::Backspace);
    assert_eq!(key.modifiers, KeyModifiers::ALT);
}

#[test]
fn parses_kitty_alt_backspace() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[127;3u").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 8);
    assert_eq!(key.code, KeyCode::Backspace);
    assert_eq!(key.modifiers, KeyModifiers::ALT);
}

#[test]
fn parses_enhanced_pageup_press() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[5;1:1~").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 8);
    assert_eq!(key.code, KeyCode::PageUp);
    assert_eq!(key.modifiers, KeyModifiers::empty());
    assert_eq!(key.kind, KeyEventKind::Press);
}

#[test]
fn parses_enhanced_pagedown_release() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x1b[6;1:3~").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 8);
    assert_eq!(key.code, KeyCode::PageDown);
    assert_eq!(key.modifiers, KeyModifiers::empty());
    assert_eq!(key.kind, KeyEventKind::Release);
}

#[test]
fn raw_input_family_matrix_is_covered() {
    let cases: &[(&[u8], KeyCode, KeyModifiers)] = &[
        (b"\x02", KeyCode::Char('b'), KeyModifiers::CONTROL),
        (b"\r", KeyCode::Enter, KeyModifiers::empty()),
        (b"\t", KeyCode::Tab, KeyModifiers::empty()),
        (b"\x7f", KeyCode::Backspace, KeyModifiers::empty()),
        (b"\x1b[A", KeyCode::Up, KeyModifiers::empty()),
        (b"\x1b[1;3A", KeyCode::Up, KeyModifiers::ALT),
        (b"\x1b\x7f", KeyCode::Backspace, KeyModifiers::ALT),
        (b"\x1b[127;3u", KeyCode::Backspace, KeyModifiers::ALT),
        (b"\x1b[57420;1u", KeyCode::Down, KeyModifiers::empty()),
        (b"\x1b[57423;1u", KeyCode::Home, KeyModifiers::empty()),
        (b"\x1bOq", KeyCode::Char('1'), KeyModifiers::empty()),
        (b"\x1b[14~", KeyCode::F(4), KeyModifiers::empty()),
        (b"\x1b[49:33;2:1u", KeyCode::Char('1'), KeyModifiers::SHIFT),
    ];

    for (bytes, code, modifiers) in cases {
        let (event, consumed) = extract_one_event(bytes).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_raw_key(event, *code, *modifiers);
    }
}

#[test]
fn unsupported_ss3_sequence_stays_unsupported() {
    let (event, consumed) = extract_one_event(b"\x1bOz").unwrap();

    assert_eq!(consumed, 3);
    assert!(matches!(event, RawInputEvent::Unsupported));
}

#[test]
fn modified_rxvt_f_key_alias_stays_unsupported() {
    let (event, consumed) = extract_one_event(b"\x1b[14;3~").unwrap();

    assert_eq!(consumed, 7);
    assert!(matches!(event, RawInputEvent::Unsupported));
}

#[test]
fn parses_raw_ctrl_b() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\x02").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 1);
    assert_eq!(key.code, KeyCode::Char('b'));
    assert_eq!(key.modifiers, KeyModifiers::CONTROL);
}

#[test]
fn parses_raw_lf_as_ctrl_j() {
    let (RawInputEvent::Key(key), consumed) = extract_one_event(b"\n").unwrap() else {
        panic!("expected key");
    };
    assert_eq!(consumed, 1);
    assert_eq!(key.code, KeyCode::Char('j'));
    assert_eq!(key.modifiers, KeyModifiers::CONTROL);
}

#[test]
fn raw_input_corpus_fixture_extracts_whole_events() {
    let corpus = include_str!("../../../../tests/fixtures/keyboard_protocol_corpus.tsv");
    assert_fixture_extracts_whole_events(corpus, false);
}

#[test]
fn raw_input_macos_terminal_variants_fixture_extracts_whole_events() {
    let corpus = include_str!("../../../../tests/fixtures/macos_terminal_variants.tsv");
    assert_fixture_extracts_whole_events(corpus, true);
}

#[test]
fn raw_input_linux_terminal_variants_fixture_extracts_whole_events() {
    let corpus = include_str!("../../../../tests/fixtures/linux_terminal_variants.tsv");
    assert_fixture_extracts_whole_events(corpus, false);
}

#[test]
fn parses_ghostty_default_background_response() {
    let events = parse_raw_input_bytes_sync(b"\x1b]11;rgb:2828/2a2a/3636\x07");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            color: RgbColor {
                r: 0x28,
                g: 0x2a,
                b: 0x36
            }
        }
    ));
}

#[test]
fn shipped_logging_filters_suppress_host_input_payloads() {
    use crate::utils::logging::test_capture::Capture;

    let input = b"\x1b[200~synthetic-secret-payload\x1b[201~";
    for filter in [crate::utils::logging::DEV_FILTER, "bus=info"] {
        let capture = Capture::default();
        capture.run_filtered(filter, || {
            let events = RawInputFramer::default().push(input);
            assert!(matches!(events.as_slice(), [RawInputEvent::Paste(_)]));
            tracing::info!(target: "bus::logging_test", "input parsed");
        });

        let text = capture.text();
        assert!(text.contains("input parsed"));
        assert!(!text.contains("synthetic-secret-payload"));
        assert!(!text.contains("raw input event parsed"));
    }
}
