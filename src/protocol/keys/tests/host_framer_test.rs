use super::*;
use crossterm::event::{KeyCode, KeyEventKind};

fn assert_raw_key(event: RawInputEvent, code: KeyCode, modifiers: KeyModifiers) {
    let RawInputEvent::Key(key) = event else {
        panic!("expected key");
    };
    assert_eq!(key.code, code);
    assert_eq!(key.modifiers, modifiers);
}

fn decode_hex(hex: &str) -> Vec<u8> {
    let hex = hex.trim();
    assert_eq!(hex.len() % 2, 0, "hex string must have even length");
    (0..hex.len())
        .step_by(2)
        .map(|idx| u8::from_str_radix(&hex[idx..idx + 2], 16).unwrap())
        .collect()
}

fn parse_fixture_key_code(value: &str) -> KeyCode {
    match value {
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "esc" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "insert" => KeyCode::Insert,
        "delete" => KeyCode::Delete,
        value if value.starts_with("char:") => {
            KeyCode::Char(value.trim_start_matches("char:").chars().next().unwrap())
        }
        other => panic!("unsupported fixture key code: {other}"),
    }
}

fn parse_fixture_modifiers(value: &str) -> KeyModifiers {
    if value == "-" || value.is_empty() {
        return KeyModifiers::empty();
    }

    let mut modifiers = KeyModifiers::empty();
    for part in value.split('+') {
        match part {
            "shift" => modifiers |= KeyModifiers::SHIFT,
            "alt" => modifiers |= KeyModifiers::ALT,
            "control" => modifiers |= KeyModifiers::CONTROL,
            "super" => modifiers |= KeyModifiers::SUPER,
            "hyper" => modifiers |= KeyModifiers::HYPER,
            "meta" => modifiers |= KeyModifiers::META,
            other => panic!("unsupported fixture modifier: {other}"),
        }
    }
    modifiers
}

fn assert_fixture_extracts_whole_events(corpus: &str, macos_layout: bool) {
    for line in corpus.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut columns: Vec<_> = line.split('\t').collect();
        if columns.len() == 5 {
            columns.push("");
        }

        if macos_layout {
            if columns.len() == 6 {
                columns.push("");
            }
            assert_eq!(
                columns.len(),
                7,
                "macOS fixture row must have 7 columns: {line}"
            );
            if columns[2].is_empty() {
                continue;
            }
            let bytes = decode_hex(columns[2]);
            let (event, consumed) = extract_one_event(&bytes).unwrap();
            assert_eq!(
                consumed,
                bytes.len(),
                "fixture should extract a whole event: {line}"
            );
            assert_raw_key(
                event,
                parse_fixture_key_code(columns[3]),
                parse_fixture_modifiers(columns[4]),
            );
        } else {
            let (bytes_hex, code, modifiers) = match columns.len() {
                6 => {
                    if columns[1].chars().all(|ch| ch.is_ascii_hexdigit()) {
                        (columns[1], columns[2], columns[3])
                    } else {
                        (columns[2], columns[3], columns[4])
                    }
                }
                7 => (columns[2], columns[3], columns[4]),
                _ => panic!("fixture row must have 6 or 7 columns: {line}"),
            };
            assert!(
                bytes_hex.chars().all(|ch| ch.is_ascii_hexdigit()),
                "non-hex fixture bytes: {bytes_hex} in {line}"
            );
            let bytes = decode_hex(bytes_hex);
            let (event, consumed) = extract_one_event(&bytes).unwrap();
            assert_eq!(
                consumed,
                bytes.len(),
                "fixture should extract a whole event: {line}"
            );
            assert_raw_key(
                event,
                parse_fixture_key_code(code),
                parse_fixture_modifiers(modifiers),
            );
        }
    }
}

#[path = "host_events_test.rs"]
mod events;

#[path = "host_mouse_test.rs"]
mod mouse;

#[path = "host_replies_test.rs"]
mod replies;

#[test]
fn raw_framer_waits_for_application_keypad_sequence_final_byte() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1bO").is_empty());
    let events = framer.push(b"q");

    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('1'),
        KeyModifiers::empty(),
    );
}

#[test]
fn chunked_legacy_arrow_waits_for_completion() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"[A");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Up,
        KeyModifiers::empty(),
    );
}

#[test]
fn lone_escape_is_buffered_until_timeout_flush() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.flush_timeout();
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Esc,
        KeyModifiers::empty(),
    );
}

#[test]
fn escape_followed_by_arrow_before_flush_does_not_emit_escape() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"[B");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Down,
        KeyModifiers::empty(),
    );
}

#[test]
fn legacy_doubled_escape_alt_arrow_remains_one_event() {
    let mut framer = RawInputFramer::default();

    let events = framer.push(b"\x1b\x1b[A");

    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Up,
        KeyModifiers::ALT,
    );
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn escape_followed_by_alt_char_before_flush_becomes_alt_key() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    let events = framer.push(b"b");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('b'),
        KeyModifiers::ALT,
    );
}

#[test]
fn chunked_kitty_sequence_waits_for_completion() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[49:33;2:").is_empty());
    let events = framer.push(b"1u");
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('1'),
        KeyModifiers::SHIFT,
    );
}

#[test]
fn bracketed_paste_routes_split_osc_replies_without_parsing_literal_escape_text() {
    let payload = b"first\n\n\x1b]4;7;rgb:1111/2222/3333\x1b\\second\x1b]10;rgb:aaaa/bbbb/cccc\x07\n\nthird\x1b]11;rgb:0000/1111/2222\x1b\\\x1b[A\x1b]0;literal title\x07\n\x1b[201~z";
    for split in 0..payload.len() - 1 {
        let mut framer = RawInputFramer::for_host_input();
        assert!(framer.push(b"\x1b[200~").is_empty());
        let mut events = framer.push(&payload[..split]);
        events.extend(framer.flush_timeout());
        events.extend(framer.push(&payload[split..]));
        events.extend(framer.flush_timeout());
        let pastes: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RawInputEvent::Paste(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            pastes,
            ["first\n\nsecond\n\nthird\x1b[A\x1b]0;literal title\x07\n"],
            "split {split}"
        );
        assert_eq!(events.len(), 5, "split {split}");
        assert!(
            matches!(&events[0], RawInputEvent::HostPaletteColors { colors } if colors == &vec![(7, RgbColor { r: 0x11, g: 0x22, b: 0x33 })])
        );
        assert!(matches!(
            events[1],
            RawInputEvent::HostDefaultColor {
                kind: DefaultColorKind::Foreground,
                ..
            }
        ));
        assert!(matches!(
            events[2],
            RawInputEvent::HostDefaultColor {
                kind: DefaultColorKind::Background,
                ..
            }
        ));
        let RawInputEvent::Key(key) = &events[4] else {
            panic!("expected trailing key");
        };
        assert_eq!(key.code, KeyCode::Char('z'));
    }
}

#[test]
fn chunked_bracketed_paste_waits_for_terminator() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[200~hello").is_empty());
    let events = framer.push(b"\x1b[201~");
    assert_eq!(events.len(), 1);
    let RawInputEvent::Paste(text) = &events[0] else {
        panic!("expected paste");
    };
    assert_eq!(text, "hello");
}

#[test]
fn incomplete_bracketed_paste_is_not_flushed_on_timeout() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[200~hello\nworld").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(b"\x1b[201~");
    assert_eq!(events.len(), 1);
    let RawInputEvent::Paste(text) = &events[0] else {
        panic!("expected paste");
    };
    assert_eq!(text, "hello\nworld");
}

#[test]
fn complete_utf8_char_before_incomplete_char_is_drained() {
    let mut framer = RawInputByteFramer::default();
    let mut input = "你".as_bytes().to_vec();
    input.push("好".as_bytes()[0]);

    assert_eq!(framer.push(&input), vec!["你".as_bytes().to_vec()]);
    assert_eq!(framer.push(&[]), Vec::<Vec<u8>>::new());
}

#[test]
fn incomplete_utf8_prefix_is_not_flushed_on_timeout() {
    let mut framer = RawInputByteFramer::default();
    let prefix = &"好".as_bytes()[..1];

    assert!(framer.push(prefix).is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(&"好".as_bytes()[1..]),
        vec!["好".as_bytes().to_vec()]
    );
}

#[test]
fn invalid_utf8_lead_byte_is_flushed_instead_of_buffered_forever() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(&[0xC0]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(!framer.has_pending_input());
}

#[test]
fn complete_utf8_char_before_incomplete_char_survives_timeout_and_next_chunk() {
    let mut framer = RawInputByteFramer::default();
    let mut input = "你".as_bytes().to_vec();
    input.push("好".as_bytes()[0]);

    assert_eq!(framer.push(&input), vec!["你".as_bytes().to_vec()]);
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(&"好".as_bytes()[1..]),
        vec!["好".as_bytes().to_vec()]
    );
}

#[test]
fn alt_utf8_char_drains_as_one_event_before_following_input() {
    let events = parse_raw_input_bytes_sync("\x1béx".as_bytes());
    assert_eq!(events.len(), 2);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('é'),
        KeyModifiers::ALT,
    );
}

#[test]
fn chunked_alt_utf8_waits_for_continuation_byte_after_escape() {
    let mut framer = RawInputFramer::default();
    let bytes = "\x1bé".as_bytes();

    assert!(framer.push(&bytes[..2]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(&bytes[2..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('é'),
        KeyModifiers::ALT,
    );
}

#[test]
fn chunked_utf8_waits_for_continuation_byte() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(&"é".as_bytes()[..1]).is_empty());
    let events = framer.push(&"é".as_bytes()[1..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('é'),
        KeyModifiers::empty(),
    );
}

#[test]
fn chunked_cjk_utf8_waits_for_all_continuation_bytes() {
    let mut framer = RawInputFramer::default();
    let bytes = "好".as_bytes();

    assert!(framer.push(&bytes[..1]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(&bytes[1..2]).is_empty());
    assert!(framer.flush_timeout().is_empty());
    let events = framer.push(&bytes[2..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('好'),
        KeyModifiers::empty(),
    );
}

#[test]
fn chunked_four_byte_utf8_waits_for_all_continuation_bytes() {
    let mut framer = RawInputFramer::default();
    let bytes = "🙂".as_bytes();

    for split in 1..bytes.len() {
        assert!(framer.push(&bytes[split - 1..split]).is_empty());
        assert!(framer.flush_timeout().is_empty());
    }

    let events = framer.push(&bytes[bytes.len() - 1..]);
    assert_eq!(events.len(), 1);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('🙂'),
        KeyModifiers::empty(),
    );
}

#[test]
fn long_multilingual_voice_like_burst_drains_without_truncation() {
    let text = "你好，今天我们测试一段比较长的语音输入。こんにちは。안녕하세요.🙂".repeat(128);
    assert!(
        text.len() > 4096,
        "test input should exceed the client read buffer"
    );
    let mut framer = RawInputByteFramer::default();

    let chunks = framer.push(text.as_bytes());
    let rebuilt: Vec<u8> = chunks.into_iter().flatten().collect();

    assert!(!framer.has_pending_input());
    assert_eq!(rebuilt, text.as_bytes());
}

#[test]
fn long_multilingual_burst_survives_one_byte_chunks_and_timeouts() {
    let text = "中文かなカナ한글🙂，。".repeat(64);
    let mut framer = RawInputByteFramer::default();
    let mut rebuilt = Vec::new();

    for byte in text.as_bytes() {
        rebuilt.extend(
            framer
                .push(std::slice::from_ref(byte))
                .into_iter()
                .flatten(),
        );
        if framer.has_pending_input() {
            assert!(framer.flush_timeout().is_empty());
        }
    }

    rebuilt.extend(framer.flush_timeout().into_iter().flatten());
    assert!(!framer.has_pending_input());
    assert_eq!(rebuilt, text.as_bytes());
}

#[test]
fn raw_input_framer_reassembles_split_default_background_response() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    let events = framer.push(b"11;#123456\x07");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            color: RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56,
            }
        }
    ));
}

#[test]
fn raw_input_byte_framer_keeps_discarding_tail_across_timeout() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"1").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"1;#123456\x07").is_empty());
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
}

#[test]
fn raw_input_byte_framer_releases_discard_on_implausible_tail() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"a").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"b"), vec![b"b".to_vec()]);
}

#[test]
fn opted_in_byte_framer_rearms_after_outer_focus_gained() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"[?997;2n"),
        vec![GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.to_vec()]
    );
}

#[test]
fn disabled_focus_query_does_not_rearm_byte_framer() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn focus_query_policy_does_not_delay_plain_escape_without_focus() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_appearance_query_on_focus();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn focus_query_without_reply_holds_escape_for_only_one_flush() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn gives_up_holding_lone_escape_after_one_idle_flush() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();

    assert!(framer.push(b"\x1b").is_empty());
    // First idle flush holds the escape.
    assert!(framer.flush_timeout().is_empty());
    // No continuation arrived; the second idle flush releases it as Escape.
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}
