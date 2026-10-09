use super::*;

#[test]
fn pane_input_set_changes_only_the_target_pane() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let target = app.state.workspaces[0].tabs[0].root_pane;
    let other = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);

    let response = app.handle_pane_input_set(
        "req".into(),
        PaneInputSetParams {
            pane_id: public_pane_id,
            right_click: crate::protocol::api::schema::PaneRightClickTarget::Pane,
        },
    );

    let response: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert!(matches!(response.result, ResponseResult::Ok {}));
    assert!(
        app.state.workspaces[0]
            .pane_state(target)
            .unwrap()
            .right_click_passthrough
    );
    assert!(
        !app.state.workspaces[0]
            .pane_state(other)
            .unwrap()
            .right_click_passthrough
    );
}

#[tokio::test]
async fn api_pane_send_keys_accepts_control_navigation_chords() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(4);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
            pane_id,
            keys: vec![
                "ctrl+h".into(),
                "ctrl+j".into(),
                "ctrl+k".into(),
                "ctrl+l".into(),
            ],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "req");
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x08]));
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0a]));
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0b]));
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0c]));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_keys_encodes_shift_tab_as_backtab() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
            pane_id,
            keys: vec!["shift+tab".into()],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from_static(b"\x1b[Z"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_get_exposes_scroll_metrics() {
    let (mut app, public_pane_id, pane_id) = app_with_scrollback_runtime();
    let runtime = app
        .state
        .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.scroll_up(3);

    let response = app.handle_pane_get(
        "req".into(),
        PaneTarget {
            pane_id: public_pane_id,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneInfo { pane } = success.result else {
        panic!("expected pane info response");
    };
    let scroll = pane.scroll.expect("scroll metrics");
    assert_eq!(scroll.offset_from_bottom, 3);
    assert!(scroll.max_offset_from_bottom >= scroll.offset_from_bottom);
    assert_eq!(scroll.viewport_rows, 5);
}

#[tokio::test]
async fn api_pane_scroll_sets_and_clamps_endpoint_owned_history() {
    let (mut app, public_pane_id, pane_id) = app_with_scrollback_runtime();
    let runtime = app
        .state
        .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    let max_offset = runtime
        .scroll_metrics()
        .expect("scroll metrics")
        .max_offset_from_bottom;

    let response = app.handle_pane_scroll(
        "req".into(),
        PaneScrollParams {
            pane_id: public_pane_id,
            offset_from_bottom: u64::MAX,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneInfo { pane } = success.result else {
        panic!("expected pane info response");
    };
    assert_eq!(
        pane.scroll.expect("scroll metrics").offset_from_bottom,
        max_offset as u64
    );
}

#[tokio::test]
async fn api_pane_read_reports_when_older_rows_are_omitted() {
    let (mut app, public_pane_id, _pane_id) = app_with_scrollback_runtime();

    let response = app.handle_pane_read(
        "req".into(),
        PaneReadParams {
            pane_id: public_pane_id,
            source: crate::protocol::api::schema::ReadSource::Recent,
            lines: Some(2),
            format: crate::protocol::api::schema::ReadFormat::Text,
            strip_ansi: true,
            intent: crate::protocol::api::schema::ReadIntent::Interactive,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneRead { read } = success.result else {
        panic!("expected pane read response");
    };
    assert!(read.text.contains("line 19"));
    assert!(read.truncated);
}

#[tokio::test]
async fn alternate_screen_harvest_reports_range_facts_for_captured_rows() {
    use std::time::{Duration, Instant};

    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0]
        .terminal_id(pane_id)
        .unwrap()
        .clone();
    let initial_bytes = b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H16\r\n17\r\n18\r\n19\r\n20";
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20,
            5,
            0,
            initial_bytes,
            8,
        );
    app.state.insert_test_runtime(pane_id, runtime);
    let fallback = app.handle_pane_read(
        "harvest".into(),
        PaneReadParams {
            pane_id: public_pane_id,
            source: crate::protocol::api::schema::ReadSource::Recent,
            lines: Some(8),
            format: crate::protocol::api::schema::ReadFormat::Text,
            strip_ansi: true,
            intent: crate::protocol::api::schema::ReadIntent::Interactive,
        },
    );
    let passive: SuccessResponse = serde_json::from_str(&fallback).unwrap();
    let ResponseResult::PaneRead { read } = passive.result else {
        panic!("expected passive pane read");
    };
    let runtime = app.lookup_runtime_sender(0, pane_id).unwrap();
    let (_, initial, content_seq) = runtime.screen_text_snapshot_with_seq().unwrap();
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    let pending = crate::server::terminals::scrollback_read::PendingAltScreenRead::start(
        terminal_id,
        "harvest".into(),
        respond_to,
        fallback,
        read,
        8,
        false,
        initial,
        content_seq,
        started,
    );
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(10))
        .unwrap();
    input_rx.try_recv().expect("bottom wheel probe");
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(130))
        .unwrap();
    input_rx.try_recv().expect("upward wheel batch");

    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H13\r\n14\r\n15\r\n16\r\n17");
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(131))
        .unwrap();
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(141))
        .unwrap();
    input_rx.try_recv().expect("restore wheel batch");
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H16\r\n17\r\n18\r\n19\r\n20");
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(142))
        .unwrap();
    assert!(pending
        .poll(Some(runtime), started + Duration::from_millis(152))
        .is_none());

    let success: SuccessResponse = serde_json::from_str(&response_rx.try_recv().unwrap()).unwrap();
    let ResponseResult::PaneRead { read } = success.result else {
        panic!("expected harvested pane read");
    };
    assert_eq!(read.text, "13\n14\n15\n16\n17\n18\n19\n20\n");
    assert!(read.truncated);
    assert_eq!((read.returned_lines, read.exhausted), (8, Some(false)));
    assert!(read.available_lines.is_none_or(|available| available >= 8));
}

fn harvested_read_from_eight_row_history(
    requested_lines: u32,
) -> crate::protocol::api::schema::PaneReadResult {
    use std::time::{Duration, Instant};

    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0]
        .terminal_id(pane_id)
        .unwrap()
        .clone();
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20,
            5,
            0,
            b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[2J\x1b[H16\r\n17\r\n18\r\n19\r\n20",
            8,
        );
    app.state.insert_test_runtime(pane_id, runtime);
    let fallback = app.handle_pane_read(
        "range-boundary".into(),
        PaneReadParams {
            pane_id: public_pane_id,
            source: crate::protocol::api::schema::ReadSource::Recent,
            lines: Some(requested_lines),
            format: crate::protocol::api::schema::ReadFormat::Text,
            strip_ansi: true,
            intent: crate::protocol::api::schema::ReadIntent::Interactive,
        },
    );
    let passive: SuccessResponse = serde_json::from_str(&fallback).unwrap();
    let ResponseResult::PaneRead { read } = passive.result else {
        panic!("expected passive pane read");
    };
    let runtime = app.lookup_runtime_sender(0, pane_id).unwrap();
    let (_, initial, content_seq) = runtime.screen_text_snapshot_with_seq().unwrap();
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    let pending = crate::server::terminals::scrollback_read::PendingAltScreenRead::start(
        terminal_id,
        "range-boundary".into(),
        respond_to,
        fallback,
        read,
        requested_lines as usize,
        false,
        initial.clone(),
        content_seq,
        started,
    );
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(10))
        .unwrap();
    input_rx.try_recv().expect("bottom wheel probe");
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(130))
        .unwrap();
    input_rx.try_recv().expect("upward wheel batch");
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H13\r\n14\r\n15\r\n16\r\n17");
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(131))
        .unwrap();
    let pending = pending
        .poll(Some(runtime), started + Duration::from_millis(141))
        .unwrap();
    input_rx.try_recv().expect("next wheel batch");
    let (pending, restore_at) = if requested_lines > 8 {
        // The next upward wheel cannot move past the eight-row history's top.
        let pending = pending
            .poll(Some(runtime), started + Duration::from_millis(261))
            .unwrap();
        input_rx
            .try_recv()
            .expect("restore wheel batch after reaching top");
        (pending, 261)
    } else {
        (pending, 141)
    };
    runtime.test_process_pty_bytes(b"\x1b[2J\x1b[H16\r\n17\r\n18\r\n19\r\n20");
    let pending = pending
        .poll(
            Some(runtime),
            started + Duration::from_millis(restore_at + 1),
        )
        .unwrap();
    assert!(pending
        .poll(
            Some(runtime),
            started + Duration::from_millis(restore_at + 11)
        )
        .is_none());
    assert_eq!(runtime.screen_text_snapshot().unwrap().1, initial);
    assert!(input_rx.try_recv().is_err());
    let response: SuccessResponse = serde_json::from_str(&response_rx.try_recv().unwrap()).unwrap();
    let ResponseResult::PaneRead { read } = response.result else {
        panic!("expected harvested pane read");
    };
    read
}

#[tokio::test]
async fn alternate_screen_harvest_reports_exact_available_rows_after_reaching_top() {
    let read = harvested_read_from_eight_row_history(12);
    assert_eq!(read.text, "13\n14\n15\n16\n17\n18\n19\n20\n");
    assert_eq!(read.requested_lines, Some(12));
    assert_eq!(read.returned_lines, 8);
    assert_eq!(read.available_lines, Some(8));
    assert_eq!(read.exhausted, Some(true));
    assert!(!read.truncated);
}

#[tokio::test]
async fn alternate_screen_harvest_limits_returned_count_when_wheel_batch_overshoots() {
    let read = harvested_read_from_eight_row_history(6);
    assert_eq!(read.text, "15\n16\n17\n18\n19\n20\n");
    assert_eq!(read.requested_lines, Some(6));
    assert_eq!(read.returned_lines, 6);
    assert_eq!(read.available_lines, Some(8));
    assert_eq!(read.exhausted, Some(false));
    assert!(read.truncated);
}

#[tokio::test]
async fn api_pane_read_honors_recent_line_requests_above_one_thousand() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let runtime =
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(80, 3, 10_000_000, &[]);
    for index in 0..1500 {
        runtime.test_process_pty_bytes(format!("{index:06}\r\n").as_bytes());
    }
    app.state.insert_test_runtime(pane_id, runtime);

    let response = app.handle_pane_read(
        "req".into(),
        PaneReadParams {
            pane_id: public_pane_id,
            source: crate::protocol::api::schema::ReadSource::Recent,
            lines: Some(5000),
            format: crate::protocol::api::schema::ReadFormat::Text,
            strip_ansi: true,
            intent: crate::protocol::api::schema::ReadIntent::Interactive,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneRead { read } = success.result else {
        panic!("expected pane read response");
    };
    let returned = read
        .text
        .split_inclusive('\n')
        .filter(|line| !line.is_empty())
        .count();
    assert!(returned > 1000, "got {returned} rows");
    assert!(read.text.contains("000000"));
    assert!(!read.truncated);
}

#[tokio::test]
async fn api_pane_send_keys_preserves_legacy_control_c_aliases() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(3);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
            pane_id,
            keys: vec!["C-c".into(), "c-c".into(), "ctrl+c".into()],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "req");
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x03]));
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x03]));
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x03]));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_keys_accepts_literal_plus() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
            pane_id,
            keys: vec!["+".into()],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "req");
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from_static(b"+"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_keys_sends_shifted_punctuation_as_text_in_kitty_mode() {
    let (mut app, pane_id) = app_with_test_workspace();
    let internal_pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let (runtime, mut rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            80,
            24,
            0,
            b"\x1b[>7u",
            1,
        );
    app.state.insert_test_runtime(internal_pane_id, runtime);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
            pane_id,
            keys: vec!["shift+?".into()],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "req");
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from_static(b"?"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_input_brackets_text_and_enter_atomically() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);
    let internal_pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.lookup_runtime_sender(0, internal_pane_id)
        .unwrap()
        .test_process_pty_bytes(b"\x1b[?2004h");

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendInput(PaneSendInputParams {
            pane_id,
            text: "A != B".into(),
            keys: vec!["Enter".into()],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(
        rx.try_recv().unwrap(),
        bytes::Bytes::from_static(b"\x1b[200~A != B\x1b[201~\r")
    );
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_input_keys_accept_key_combo_chords() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(1);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendInput(PaneSendInputParams {
            pane_id,
            text: String::new(),
            keys: vec!["ctrl+j".into()],
        }),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(success.id, "req");
    assert_eq!(success.result, ResponseResult::Ok {});
    assert_eq!(rx.try_recv().unwrap(), bytes::Bytes::from(vec![0x0a]));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_keys_rejects_invalid_keys_before_writing() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(2);

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendKeys(PaneSendKeysParams {
            pane_id,
            keys: vec!["ctrl+h".into(), "not-a-key".into()],
        }),
    });

    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "invalid_key");
    assert_eq!(error.error.message, "unsupported key not-a-key");
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn api_pane_send_input_rejects_prefix_bindings_before_writing_text_or_keys() {
    let (mut app, pane_id, mut rx) = app_with_send_key_runtime(4);
    let raw_key = " prefix+h ".to_string();

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneSendInput(PaneSendInputParams {
            pane_id,
            text: "hello".into(),
            keys: vec!["ctrl+h".into(), raw_key.clone()],
        }),
    });

    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "invalid_key");
    assert_eq!(error.error.message, format!("unsupported key {raw_key}"));
    assert!(rx.try_recv().is_err());
}

#[test]
fn api_pane_focus_marks_already_focused_done_pane_seen() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.outer_terminal_focus = Some(false);

    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
        .attached_terminal_id
        .clone();
    app.state.terminals.get_mut(&terminal_id).unwrap().state = crate::agents::AgentState::Idle;
    app.state.workspaces[0].tabs[0]
        .panes
        .get_mut(&pane_id)
        .unwrap()
        .seen = false;
    app.state.workspaces[0].tabs[0].layout.focus_pane(pane_id);

    let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
    let response = app.handle_pane_focus(
        "req".into(),
        PaneTarget {
            pane_id: public_pane_id,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneInfo { pane } = success.result else {
        panic!("expected pane info response");
    };
    assert_eq!(
        pane.agent_status,
        crate::protocol::api::schema::AgentStatus::Idle
    );
}
