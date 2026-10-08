use super::*;

#[test]
fn process_pty_bytes_does_not_advertise_unsupported_glyph_protocol() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b_25a1;s\x1b\\", &tx);

    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_libghostty_query_responses_without_queuing_input() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[6n", &tx);

    assert_eq!(result.terminal_responses.len(), 1);
    assert!(String::from_utf8_lossy(&result.terminal_responses[0]).contains('R'));
    assert!(rx.try_recv().is_err());
}

#[test]
fn color_scheme_queries_and_live_updates_follow_terminal_mode() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
        .is_none());
    let query = pane.process_pty_bytes(pane_id, 0, b"\x1b[?996n", &tx);
    assert_eq!(
        query.terminal_responses,
        vec![Bytes::from_static(b"\x1b[?997;1n")]
    );

    pane.process_pty_bytes(pane_id, 0, b"\x1b[?2031h", &tx);
    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
        .is_none());
    assert_eq!(
        pane.apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light)),
        Some(Bytes::from_static(b"\x1b[?997;2n"))
    );

    assert!(pane.apply_host_terminal_appearance(None).is_none());
    let unknown_query = pane.process_pty_bytes(pane_id, 0, b"\x1b[?996n", &tx);
    assert!(unknown_query.terminal_responses.is_empty());
    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark))
        .is_none());

    pane.process_pty_bytes(pane_id, 0, b"\x1bc", &tx);
    assert!(pane
        .apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light))
        .is_none());
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_xtgettcap_truecolor_query_responses_without_queuing_input() {
    let (tx, mut rx) = mpsc::channel(8);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(
        pane_id,
        0,
        b"\x1bP+q5463;524742;73657472676266;73657472676262\x1b\\",
        &tx,
    );

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_xtgettcap_response("5463", None),
            expected_xtgettcap_response("524742", Some(b"8")),
            expected_xtgettcap_response("73657472676266", Some(b"\\E[38:2:%p1%d:%p2%d:%p3%dm")),
            expected_xtgettcap_response("73657472676262", Some(b"\\E[48:2:%p1%d:%p2%d:%p3%dm")),
        ]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_split_xtgettcap_query_response() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1bP+q4", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b"D73\x1b", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b"\\", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![expected_xtgettcap_response(
            "4D73",
            Some(b"\\E]52;%p1%s;%p2%s\\007")
        )]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_orders_device_attribute_reply_before_following_xtgettcap_reply() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b[c\x1bP+q5463\x1b\\", &tx);

    assert_eq!(result.terminal_responses.len(), 2);
    assert!(String::from_utf8_lossy(&result.terminal_responses[0]).contains('c'));
    assert_eq!(
        result.terminal_responses[1],
        expected_xtgettcap_response("5463", None)
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_orders_xtgettcap_reply_before_following_device_attribute_reply() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1bP+q5463\x1b\\\x1b[c", &tx);

    assert_eq!(result.terminal_responses.len(), 2);
    assert_eq!(
        result.terminal_responses[0],
        expected_xtgettcap_response("5463", None)
    );
    assert!(String::from_utf8_lossy(&result.terminal_responses[1]).contains('c'));
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_orders_xtgettcap_reply_before_following_default_color_reply() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1bP+q5463\x1b\\\x1b]11;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_xtgettcap_response("5463", None),
            Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x07"),
        ]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn host_theme_update_preserves_child_default_color_override() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;#112233\x07", &tx);
    assert!(result.terminal_responses.is_empty());

    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;?\x07", &tx);
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:1111/2222/3333\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn child_default_color_reset_restores_cached_host_color() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"\x1b]11;#112233\x07", &tx);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xaa,
            g: 0xbb,
            b: 0xcc,
        }),
        ..Default::default()
    });
    pane.process_pty_bytes(pane_id, 0, b"\x1b]111\x07", &tx);
    assert!(!pane.has_transient_default_color_override());

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;?\x07", &tx);
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:aaaa/bbbb/cccc\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_recovers_xtgettcap_after_osc_bel_terminator() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]0;title\x07\x1bP+q5463\x1b\\", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![expected_xtgettcap_response("5463", None)]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_ignores_unknown_and_unsupported_xtgettcap_queries() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1bP+q6E6F7065;4D7\x1b\\", &tx);

    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_underline_color_xtgettcap_query_responses() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(
        pane_id,
        0,
        b"\x1bP+q5375;536D756C78;536574756C63\x1b\\",
        &tx,
    );

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_xtgettcap_response("5375", None),
            expected_xtgettcap_response("536D756C78", Some(b"\\E[4:%p1%dm")),
            expected_xtgettcap_response(
                "536574756C63",
                Some(b"\\E[58:2::%p1%{65536}%/%d:%p1%{256}%/%{255}%&%d:%p1%{255}%&%d%;m")
            ),
        ]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_orders_default_color_reply_before_following_device_attribute_reply() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;?\x07\x1b[c", &tx);

    assert_eq!(result.terminal_responses.len(), 2);
    assert_eq!(
        result.terminal_responses[0],
        Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x07")
    );
    assert!(String::from_utf8_lossy(&result.terminal_responses[1]).contains('c'));
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_host_palette_color_without_queuing_input() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(
        crate::terminal_theme::TerminalTheme::default().with_palette_color(
            0,
            crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
        ),
    );

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;0;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]4;0;rgb:1111/2222/3333\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn opentui_256_palette_query_burst_uses_host_snapshot() {
    use std::fmt::Write as _;

    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let mut theme = crate::terminal_theme::TerminalTheme::default();
    let mut queries = String::new();
    for index in 0..=u8::MAX {
        theme = theme.with_palette_color(
            index,
            crate::terminal_theme::RgbColor {
                r: index,
                g: 0x22,
                b: 0x33,
            },
        );
        let _ = write!(queries, "\x1b]4;{index};?\x07");
    }
    pane.apply_host_terminal_theme(theme);

    let result = pane.process_pty_bytes(pane_id, 0, queries.as_bytes(), &tx);

    assert_eq!(result.terminal_responses.len(), 256);
    assert_eq!(
        result.terminal_responses[0],
        Bytes::from_static(b"\x1b]4;0;rgb:0000/2222/3333\x07")
    );
    assert_eq!(
        result.terminal_responses[255],
        Bytes::from_static(b"\x1b]4;255;rgb:ffff/2222/3333\x07")
    );
}

#[test]
fn child_palette_override_survives_host_refresh_until_reset() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(
        crate::terminal_theme::TerminalTheme::default().with_palette_color(
            7,
            crate::terminal_theme::RgbColor {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
        ),
    );
    pane.process_pty_bytes(pane_id, 0, b"\x1b]4;7;rgb:aa/bb/cc\x1b\\", &tx);

    pane.apply_host_terminal_theme(
        crate::terminal_theme::TerminalTheme::default().with_palette_color(
            7,
            crate::terminal_theme::RgbColor {
                r: 0x44,
                g: 0x55,
                b: 0x66,
            },
        ),
    );
    let overridden = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;7;?\x1b\\", &tx);
    assert_eq!(
        overridden.terminal_responses,
        vec![Bytes::from_static(b"\x1b]4;7;rgb:aaaa/bbbb/cccc\x1b\\")]
    );

    pane.process_pty_bytes(pane_id, 0, b"\x1b]104;7\x1b\\", &tx);
    let reset = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;7;?\x1b\\", &tx);
    assert_eq!(
        reset.terminal_responses,
        vec![Bytes::from_static(b"\x1b]4;7;rgb:4444/5555/6666\x1b\\")]
    );
}

#[test]
fn process_pty_bytes_returns_split_palette_color_query_response() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let color = current_palette_color(&pane, 255);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;25", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b"5;?\x1b", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b"\\", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![expected_osc_rgb_response("4;255", color, "\x1b\\")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_ignores_malformed_and_preserves_multi_palette_queries() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(
            pane_id,
            0,
            b"\x1b]4;;?\x07\x1b]4;-1;?\x07\x1b]4;256;?\x07\x1b]4;0;?;1;?\x07\x1b]4;0;rgb:1111/2222/3333\x07",
            &tx,
        );

    assert_eq!(result.terminal_responses.len(), 1);
    assert!(result.terminal_responses[0].starts_with(b"\x1b]4;0;rgb:"));
    assert_eq!(
        result.terminal_responses[0]
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        2
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_orders_palette_reply_before_following_terminal_replies() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let color = current_palette_color(&pane, 0);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;0;?\x07\x1b]11;?\x07\x1b[c", &tx);

    assert_eq!(result.terminal_responses.len(), 3);
    assert_eq!(
        result.terminal_responses[0],
        expected_osc_rgb_response("4;0", color, "\x07")
    );
    assert_eq!(
        result.terminal_responses[1],
        Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x07")
    );
    assert!(String::from_utf8_lossy(&result.terminal_responses[2]).contains('c'));
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_default_color_query_responses_without_queuing_input() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x00,
            g: 0x2b,
            b: 0x36,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:0000/2b2b/3636\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_answers_default_color_queries_without_a_host_theme() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let colors = current_default_colors(&pane);

    // Bus has not learned its own host terminal's colors, which is the
    // normal case under a detached multiplexer. The pane still has to
    // answer, with the colors it paints, or a child that waits on the
    // report never draws.
    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![
            expected_osc_rgb_response("10", colors.foreground, "\x1b\\"),
            expected_osc_rgb_response("11", colors.background, "\x1b\\"),
        ]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_mirrors_query_terminator_in_default_color_reply() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let colors = current_default_colors(&pane);

    let bel = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;?\x07", &tx);
    assert_eq!(
        bel.terminal_responses,
        vec![expected_osc_rgb_response("11", colors.background, "\x07")]
    );

    let st = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;?\x1b\\", &tx);
    assert_eq!(
        st.terminal_responses,
        vec![expected_osc_rgb_response("11", colors.background, "\x1b\\")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_answers_every_tui_startup_capability_query() {
    let (tx, mut rx) = mpsc::channel(8);
    let terminal = crate::ghostty::Terminal::new(120, 30, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    let colors = current_default_colors(&pane);

    // The startup preamble a Codex-style TUI writes: mode sets that need no
    // answer, then cursor position, both default colors, the Kitty keyboard
    // flags and primary device attributes, which all do.
    let result = pane.process_pty_bytes(
            pane_id,
            0,
            b"\x1b[?2004h\x1b[>4;0m\x1b[>5u\x1b[?1004h\x1b[6n\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b[?u\x1b[c",
            &tx,
        );

    let replies: Vec<String> = result
        .terminal_responses
        .iter()
        .map(|response| String::from_utf8_lossy(response).into_owned())
        .collect();
    assert_eq!(replies.len(), 5, "unanswered startup query: {replies:?}");
    assert!(replies[0].ends_with('R'), "no cursor position report");
    assert_eq!(
        result.terminal_responses[1],
        expected_osc_rgb_response("10", colors.foreground, "\x1b\\")
    );
    assert_eq!(
        result.terminal_responses[2],
        expected_osc_rgb_response("11", colors.background, "\x1b\\")
    );
    assert!(replies[3].ends_with('u'), "no Kitty keyboard flags report");
    assert!(replies[4].ends_with('c'), "no device attributes report");
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_preserves_untracked_multi_color_query_responses() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let palette = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;0;?;1;?\x1b\\", &tx);
    let palette_response = palette.terminal_responses.concat();
    assert!(palette_response.starts_with(b"\x1b]4;0;rgb:"));
    assert_eq!(
        palette_response
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        2
    );

    let defaults = pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?;?;?\x1b\\", &tx);
    let default_response = defaults.terminal_responses.concat();
    assert!(
        default_response.starts_with(b"\x1b]10;rgb:"),
        "unexpected default-color report: {:?}",
        String::from_utf8_lossy(&default_response)
    );
    assert_eq!(
        default_response
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        3
    );
    let core = pane.core.lock().unwrap();
    assert!(!core.child_default_foreground_changed);
    assert!(!core.child_default_background_changed);
    drop(core);
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_preserves_earlier_aggregate_palette_reply() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]4;0;?;1;?\x1b\\\x1b]4;0;?\x1b\\", &tx);

    assert_eq!(result.terminal_responses.len(), 2);
    assert_eq!(
        result.terminal_responses[0]
            .windows(4)
            .filter(|window| *window == b"rgb:")
            .count(),
        2
    );
    assert!(result.terminal_responses[1].starts_with(b"\x1b]4;0;rgb:"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_preserves_libghostty_reply_for_child_color_override() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"\x1b]10;rgb:11/22/33\x07", &tx);
    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?\x1b\\", &tx);

    assert_eq!(result.terminal_responses.len(), 1);
    assert!(result.terminal_responses[0].starts_with(b"\x1b]10;rgb:1111/2222/3333"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_tracks_later_multi_value_color_set() {
    let (tx, _rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);

    pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?;rgb:44/55/66\x1b\\", &tx);

    let core = pane.core.lock().unwrap();
    assert!(!core.child_default_foreground_changed);
    assert!(core.child_default_background_changed);
}

#[test]
fn process_pty_bytes_returns_cursor_color_query_response_from_foreground_fallback() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: None,
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:6565/7b7b/8383\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_cursor_color_query_response_from_child_foreground() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: None,
        ..Default::default()
    });

    pane.process_pty_bytes(pane_id, 0, b"\x1b]10;rgb:11/22/33\x07", &tx);
    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_explicit_cursor_color_query_response() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: None,
        ..Default::default()
    });

    pane.process_pty_bytes(pane_id, 0, b"\x1b]12;rgb:11/22/33\x07", &tx);
    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:1111/2222/3333\x07")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_default_color_query_responses_in_order() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x65,
            g: 0x7b,
            b: 0x83,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let result =
        pane.process_pty_bytes(pane_id, 0, b"\x1b]10;?\x07\x1b]11;?\x07\x1b]12;?\x07", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![
            Bytes::from_static(b"\x1b]10;rgb:6565/7b7b/8383\x07"),
            Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07"),
            Bytes::from_static(b"\x1b]12;rgb:6565/7b7b/8383\x07"),
        ]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_split_default_color_query_response() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b";?\x1b", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b"\\", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_returns_split_cursor_color_query_response() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        background: None,
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]12", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b";?\x1b", &tx);
    assert!(result.terminal_responses.is_empty());
    assert!(rx.try_recv().is_err());
    let result = pane.process_pty_bytes(pane_id, 0, b"\\", &tx);

    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]12;rgb:fdfd/f6f6/e3e3\x1b\\")]
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn process_pty_bytes_tracks_default_color_set_and_reset_before_replying() {
    let (tx, mut rx) = mpsc::channel(4);
    let terminal = crate::ghostty::Terminal::new(20, 5, 0).unwrap();
    let pane = GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap();
    let pane_id = PaneId::from_raw(1);
    pane.apply_host_terminal_theme(crate::terminal_theme::TerminalTheme {
        foreground: None,
        background: Some(crate::terminal_theme::RgbColor {
            r: 0xfd,
            g: 0xf6,
            b: 0xe3,
        }),
        ..Default::default()
    });

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]11;rgb:11/22/33\x07\x1b]11;?\x07", &tx);
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:1111/2222/3333\x07")]
    );
    assert!(rx.try_recv().is_err());

    let result = pane.process_pty_bytes(pane_id, 0, b"\x1b]111\x07\x1b]11;?\x07", &tx);
    assert_eq!(
        result.terminal_responses,
        vec![Bytes::from_static(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x07")]
    );
    assert!(rx.try_recv().is_err());
}
