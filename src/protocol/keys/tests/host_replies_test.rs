use super::*;

#[test]
fn parses_host_default_color_response_with_st() {
    let (RawInputEvent::HostDefaultColor { kind, color }, consumed) =
        extract_one_event(b"\x1b]10;rgb:cccc/dddd/eeee\x1b\\").unwrap()
    else {
        panic!("expected host color response");
    };
    assert_eq!(consumed, 25);
    assert_eq!(kind, DefaultColorKind::Foreground);
    assert_eq!(
        color,
        RgbColor {
            r: 0xcc,
            g: 0xdd,
            b: 0xee
        }
    );
}

#[test]
fn parses_host_default_color_response_with_bel() {
    let (RawInputEvent::HostDefaultColor { kind, color }, consumed) =
        extract_one_event(b"\x1b]11;#112233\x07").unwrap()
    else {
        panic!("expected host color response");
    };
    assert_eq!(consumed, 13);
    assert_eq!(kind, DefaultColorKind::Background);
    assert_eq!(
        color,
        RgbColor {
            r: 0x11,
            g: 0x22,
            b: 0x33
        }
    );
}

#[test]
fn parses_host_palette_color_response() {
    let (RawInputEvent::HostPaletteColors { colors }, consumed) =
        extract_one_event(b"\x1b]4;7;rgb:1111/2222/3333\x1b\\").unwrap()
    else {
        panic!("expected host palette response");
    };
    assert_eq!(consumed, 26);
    assert_eq!(
        colors,
        vec![(
            7,
            RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            }
        )]
    );
}

#[test]
fn parses_ghostty_color_scheme_reports() {
    for bytes in [
        GHOSTTY_COLOR_SCHEME_DARK_REPORT,
        GHOSTTY_COLOR_SCHEME_LIGHT_REPORT,
    ] {
        let events = parse_raw_input_bytes_sync(bytes);
        assert_eq!(events.len(), 1, "bytes: {bytes:?}");
        assert!(matches!(
            events[0],
            RawInputEvent::HostColorSchemeChanged(HostAppearance::Dark | HostAppearance::Light)
        ));
    }
}

#[test]
fn ghostty_color_scheme_report_parser_is_exact() {
    for bytes in [
        b"\x1b[?997;0n".as_slice(),
        b"\x1b[?997;3n".as_slice(),
        b"\x1b[?998;1n".as_slice(),
    ] {
        let events = parse_raw_input_bytes_sync(bytes);
        assert_eq!(events.len(), 1, "bytes: {bytes:?}");
        assert!(matches!(events[0], RawInputEvent::Unsupported));
    }
}

#[test]
fn raw_input_framer_reassembles_split_color_scheme_report() {
    let mut framer = RawInputFramer::default();

    assert!(framer.push(b"\x1b[?997;").is_empty());
    let events = framer.push(b"1n");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostColorSchemeChanged(HostAppearance::Dark)
    ));
}

#[test]
fn parses_host_cell_size_report() {
    let events = parse_raw_input_bytes_sync(b"\x1b[6;21;10t");

    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        RawInputEvent::HostCellSizeReport {
            width_px: 10,
            height_px: 21,
        }
    ));
}

#[test]
fn host_cell_size_report_parser_is_exact() {
    for bytes in [
        // Zero dimensions carry no usable cell size.
        b"\x1b[6;0;10t".as_slice(),
        b"\x1b[6;21;0t".as_slice(),
        // Missing or extra parameters.
        b"\x1b[6;21t".as_slice(),
        b"\x1b[6;21;10;3t".as_slice(),
        // Other XTWINOPS reports must not be mistaken for a cell size.
        b"\x1b[4;1610;777t".as_slice(),
        b"\x1b[8;37;161t".as_slice(),
        // Non-numeric parameters.
        b"\x1b[6;21;1-t".as_slice(),
    ] {
        assert!(
            parse_host_cell_size_report(bytes).is_none(),
            "bytes: {bytes:?}"
        );
    }
}

#[test]
fn split_color_scheme_timeout_does_not_swallow_legacy_alt_bracket() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b[").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b[".to_vec()]);
}

#[test]
fn raw_input_byte_framer_discards_timed_out_split_color_scheme_report_tail() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b[?997;").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"1n").is_empty());
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
    assert!(framer.flush_timeout().is_empty());
}

#[cfg(not(target_os = "macos"))]
#[test]
fn non_macos_host_input_splits_lone_escape_from_arrow() {
    let mut framer = RawInputByteFramer::for_host_input();

    assert_eq!(
        framer.push(b"\x1b\x1b[D"),
        vec![b"\x1b".to_vec(), b"\x1b[D".to_vec()]
    );
}

#[test]
fn macos_host_input_policy_preserves_legacy_doubled_escape_alt_arrow() {
    let mut framer = RawInputByteFramer::with_host_input_policy(true);

    assert_eq!(framer.push(b"\x1b\x1b[D"), vec![b"\x1b\x1b[D".to_vec()]);
}

#[test]
fn raw_input_byte_framer_discards_split_control_string_after_timeout() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b]").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"11;#123456\x07").is_empty());
    assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
}

#[test]
fn parse_raw_input_bytes_sync_does_not_parse_incomplete_strings_as_alt_keys() {
    for bytes in [
        b"\x1b]".as_slice(),
        b"\x1bP".as_slice(),
        b"\x1b_".as_slice(),
        b"\x1b^".as_slice(),
        b"\x1bX".as_slice(),
    ] {
        let events = parse_raw_input_bytes_sync(bytes);

        assert!(events.is_empty(), "parsed {bytes:?} as {events:?}");
    }
}

#[test]
fn non_osc_control_strings_ignore_bel_and_complete_at_st() {
    let bytes = b"\x1bPabc\x07def\x1b\\x";

    let (event, consumed) = extract_one_event(bytes).unwrap();

    assert!(matches!(event, RawInputEvent::Unsupported));
    assert_eq!(consumed, b"\x1bPabc\x07def\x1b\\".len());
}

#[test]
fn non_osc_default_color_text_remains_key_input() {
    let events = parse_raw_input_bytes_sync(b"11;rgb:2828/2a2a/3636\x07");

    assert_eq!(events.len(), 22);
    assert_raw_key(
        events.into_iter().next().unwrap(),
        KeyCode::Char('1'),
        KeyModifiers::empty(),
    );
}

#[test]
fn byte_framer_does_not_hold_non_osc_default_color_text() {
    let mut framer = RawInputByteFramer::default();

    let chunks = framer.push(b"11;rgb:2828");

    assert_eq!(chunks.len(), 11);
    assert!(framer.flush_timeout().is_empty());
}

#[test]
fn holds_lone_escape_and_stitches_split_host_color_reply() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();

    // The reply is split right at its ESC introducer.
    assert!(framer.push(b"\x1b").is_empty());
    // The idle flush must not release the ESC as an Escape key while a host
    // color reply is still outstanding.
    assert!(framer.flush_timeout().is_empty());

    // The rest of the OSC 11 reply arrives and stitches back together
    // instead of leaking its payload into the focused pane.
    let chunks = framer.push(b"]11;rgb:2424/2727/3a3a\x1b\\");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            ..
        }
    ));
}

#[test]
fn holds_lone_escape_and_stitches_split_host_cell_size_reply() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    // The XTWINOPS reply is split right at its ESC introducer.
    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());

    let chunks = framer.push(b"[6;21;10t");
    assert_eq!(chunks, vec![b"\x1b[6;21;10t".to_vec()]);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostCellSizeReport {
            width_px: 10,
            height_px: 21,
        }
    ));
}

#[test]
fn timed_out_host_cell_size_reply_fragments_do_not_leak() {
    for (prefix, tail) in [
        (b"\x1b[6".as_slice(), b";21;10t".as_slice()),
        (b"\x1b[6;".as_slice(), b"21;10t".as_slice()),
        (b"\x1b[6;21;".as_slice(), b"10t".as_slice()),
    ] {
        let mut framer = RawInputByteFramer::default();
        framer.host_cell_size_query_sent();

        assert!(framer.push(prefix).is_empty(), "prefix: {prefix:?}");
        assert!(framer.flush_timeout().is_empty(), "prefix: {prefix:?}");
        assert!(framer.push(tail).is_empty(), "tail: {tail:?}");
        assert_eq!(framer.push(b"a"), vec![b"a".to_vec()]);
    }
}

#[test]
fn split_host_cell_size_reply_after_csi_intro_gets_one_more_flush() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert!(framer.push(b"\x1b[").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.push(b"6;21;10t"), vec![b"\x1b[6;21;10t".to_vec()]);

    let mut alt_bracket = RawInputByteFramer::default();
    alt_bracket.host_cell_size_query_sent();
    assert!(alt_bracket.push(b"\x1b[").is_empty());
    assert!(alt_bracket.flush_timeout().is_empty());
    assert_eq!(alt_bracket.flush_timeout(), vec![b"\x1b[".to_vec()]);
}

#[test]
fn malformed_host_reply_tail_preserves_following_input() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert!(framer.push(b"\x1b[6;21").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b";10xabc"),
        vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
    );
}

#[test]
fn host_reply_tail_discard_is_bounded_across_pushes() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert!(framer.push(b"\x1b[6;21").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(&[b'1'; 64]).is_empty());
    assert_eq!(
        framer.push(&[b'2'; 67]),
        vec![b"2".to_vec(), b"2".to_vec(), b"2".to_vec()]
    );
}

#[test]
fn stops_holding_lone_escape_after_host_cell_size_reply_completes() {
    let mut framer = RawInputByteFramer::default();
    framer.host_cell_size_query_sent();

    assert_eq!(
        framer.push(b"\x1b[6;21;10t"),
        vec![b"\x1b[6;21;10t".to_vec()]
    );

    // Window closed: a later lone Escape flushes immediately.
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn default_byte_framer_does_not_rearm_after_color_scheme_report() {
    let mut framer = RawInputByteFramer::default();

    assert_eq!(
        framer.push(GHOSTTY_COLOR_SCHEME_DARK_REPORT),
        vec![GHOSTTY_COLOR_SCHEME_DARK_REPORT.to_vec()]
    );
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn opt_in_does_not_delay_plain_escape_without_color_scheme_report() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn opted_in_byte_framer_reassembles_appearance_reply_split_after_csi() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b[").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"?997;2n"),
        vec![GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.to_vec()]
    );
}

#[test]
fn opted_in_byte_framer_reassembles_delayed_appearance_reply() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b[?997;").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"2n"),
        vec![GHOSTTY_COLOR_SCHEME_LIGHT_REPORT.to_vec()]
    );
}

#[test]
fn timed_out_appearance_reply_preserves_pending_color_reply_window() {
    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    framer.enable_host_appearance_query_on_focus();

    assert_eq!(framer.push(b"\x1b[I"), vec![b"\x1b[I".to_vec()]);
    assert!(framer.push(b"\x1b[?997;").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert!(framer.push(b"2n").is_empty());

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(
        framer.push(b"]10;rgb:aaaa/bbbb/cccc\x1b\\"),
        vec![b"\x1b]10;rgb:aaaa/bbbb/cccc\x1b\\".to_vec()]
    );
}

#[test]
fn opted_in_byte_framer_rearms_after_color_scheme_report() {
    let mut framer = RawInputByteFramer::default();
    framer.enable_host_color_scheme_change_tracking();

    assert_eq!(
        framer.push(GHOSTTY_COLOR_SCHEME_DARK_REPORT),
        vec![GHOSTTY_COLOR_SCHEME_DARK_REPORT.to_vec()]
    );

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let chunks = framer.push(b"]10;#abcdef\x07");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Foreground,
            color: RgbColor {
                r: 0xab,
                g: 0xcd,
                b: 0xef
            }
        }
    ));

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    let chunks = framer.push(b"]11;#123456\x07");
    assert_eq!(chunks.len(), 1);
    let (event, _) = extract_one_event(&chunks[0]).unwrap();
    assert!(matches!(
        event,
        RawInputEvent::HostDefaultColor {
            kind: DefaultColorKind::Background,
            color: RgbColor {
                r: 0x12,
                g: 0x34,
                b: 0x56
            }
        }
    ));

    assert!(framer.push(b"\x1b").is_empty());
    assert!(framer.flush_timeout().is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn flushes_lone_escape_when_not_awaiting_host_color_reply() {
    let mut framer = RawInputByteFramer::default();

    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}

#[test]
fn stops_holding_lone_escape_after_host_color_reply_completes() {
    use std::fmt::Write as _;

    let mut framer = RawInputByteFramer::default();
    framer.host_color_query_sent();
    let mut replies =
        String::from("\x1b]10;rgb:6565/7b7b/8383\x1b\\\x1b]11;rgb:2424/2727/3a3a\x1b\\");
    for index in 0..=u8::MAX {
        let _ = write!(replies, "\x1b]4;{index};rgb:1111/2222/3333\x1b\\");
    }

    let chunks = framer.push(replies.as_bytes());
    assert_eq!(chunks.len(), 258);

    // Window closed: a later lone Escape flushes immediately.
    assert!(framer.push(b"\x1b").is_empty());
    assert_eq!(framer.flush_timeout(), vec![b"\x1b".to_vec()]);
}
