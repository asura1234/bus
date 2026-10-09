use super::*;

#[tokio::test]
async fn api_pane_selection_read_uses_endpoint_terminal_text() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 1000, b"hello world"),
    );

    let runtime = app
        .state
        .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
        .unwrap();
    let revision = runtime.content_seq();
    runtime.test_process_pty_bytes(b"\r\nagent is still working");
    assert_ne!(runtime.content_seq(), revision);
    let mut params = PaneSelectionReadParams {
        pane_id: public_pane_id.clone(),
        anchor: crate::protocol::api::schema::PaneTextPoint { row: 0, col: 0 },
        cursor: crate::protocol::api::schema::PaneTextPoint { row: 0, col: 4 },
        content_revision: Some(revision),
    };
    assert_eq!(
        app.pane_selection_text(&params).unwrap_err().0,
        "stale_content"
    );
    params.content_revision = None;
    let response = app.handle_pane_selection_read("req".into(), params);

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        success.result,
        ResponseResult::PaneSelection {
            pane_id: public_pane_id,
            text: "hello".into(),
        }
    );
}

#[tokio::test]
async fn api_copy_motion_uses_endpoint_terminal_word_semantics() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 1000, b"hello world"),
    );

    let response = app.handle_pane_copy_motion(
        "req".into(),
        PaneCopyMotionParams {
            pane_id: public_pane_id.clone(),
            cursor: crate::protocol::api::schema::PaneTextPoint { row: 0, col: 0 },
            motion: PaneCopyMotion::NextWordStart,
            content_revision: None,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        success.result,
        ResponseResult::PaneCopyMotion {
            pane_id: public_pane_id,
            cursor: crate::protocol::api::schema::PaneTextPoint { row: 0, col: 6 },
            content_revision: 0,
        }
    );
}

#[tokio::test]
async fn api_paragraph_motion_preserves_the_copy_cursor_column() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            20,
            5,
            1000,
            b"one\r\n\r\nthree",
        ),
    );
    let response = app.handle_pane_copy_motion(
        "req".into(),
        PaneCopyMotionParams {
            pane_id: public_pane_id.clone(),
            cursor: PaneTextPoint { row: 0, col: 2 },
            motion: PaneCopyMotion::NextParagraph,
            content_revision: None,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        success.result,
        ResponseResult::PaneCopyMotion {
            pane_id: public_pane_id,
            cursor: PaneTextPoint { row: 1, col: 2 },
            content_revision: 0,
        }
    );
}

#[tokio::test]
async fn api_copy_search_uses_endpoint_terminal_matches_and_wraps() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            20,
            5,
            1000,
            b"alpha beta alpha",
        ),
    );

    let content_revision = app
        .state
        .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
        .expect("runtime")
        .content_seq();
    let response = app.handle_pane_copy_search(
        "req".into(),
        PaneCopySearchParams {
            pane_id: public_pane_id.clone(),
            query: "alpha".into(),
            direction: PaneCopySearchDirection::Forward,
            cursor: PaneTextPoint { row: 0, col: 0 },
            content_revision,
            previous: None,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneCopySearch {
        pane_id,
        matches,
        current,
        total,
        current_global,
        ..
    } = success.result
    else {
        panic!("expected copy search response");
    };
    assert_eq!(pane_id, public_pane_id);
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].start, PaneTextPoint { row: 0, col: 0 });
    assert_eq!(matches[1].start, PaneTextPoint { row: 0, col: 11 });
    assert_eq!(current, Some(1));
    assert_eq!(current_global, Some(1));
    assert_eq!(total, 2);
}

#[tokio::test]
async fn api_copy_search_bounds_returned_matches_but_keeps_exact_total() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    let text = "a ".repeat(1500);
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            200,
            20,
            4000,
            text.as_bytes(),
        ),
    );
    let content_revision = app
        .state
        .runtime_for_pane_in_workspace(&app.terminal_runtimes, 0, pane_id)
        .expect("runtime")
        .content_seq();

    let response = app.handle_pane_copy_search(
        "req".into(),
        PaneCopySearchParams {
            pane_id: public_pane_id,
            query: "a".into(),
            direction: PaneCopySearchDirection::Forward,
            cursor: PaneTextPoint { row: 0, col: 0 },
            content_revision,
            previous: None,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneCopySearch { matches, total, .. } = success.result else {
        panic!("expected copy search response");
    };
    assert_eq!(total, 1500);
    assert_eq!(matches.len(), 1024);
}

#[tokio::test]
async fn api_copy_search_rejects_stale_content_revision() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 1000, b"alpha beta"),
    );
    let response = app.handle_pane_copy_search(
        "req".into(),
        PaneCopySearchParams {
            pane_id: public_pane_id,
            query: "alpha".into(),
            direction: PaneCopySearchDirection::Forward,
            cursor: PaneTextPoint { row: 0, col: 0 },
            content_revision: 2,
            previous: None,
        },
    );
    assert!(response.contains("stale_content"));
}

#[tokio::test]
async fn api_line_end_motion_uses_terminal_cells_for_grapheme_clusters() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            20,
            5,
            1000,
            "👨\u{200d}👩\u{200d}👧 x".as_bytes(),
        ),
    );
    assert_eq!(
        app.pane_selection_text(&PaneSelectionReadParams {
            pane_id: public_pane_id.clone(),
            anchor: PaneTextPoint { row: 0, col: 3 },
            cursor: PaneTextPoint { row: 0, col: 3 },
            content_revision: None,
        })
        .unwrap(),
        "x",
        "the family grapheme occupies two terminal cells"
    );

    let response = app.handle_pane_copy_motion(
        "line-end".into(),
        PaneCopyMotionParams {
            pane_id: public_pane_id,
            cursor: PaneTextPoint { row: 0, col: 0 },
            motion: PaneCopyMotion::LineEnd,
            content_revision: None,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneCopyMotion { cursor, .. } = success.result else {
        panic!("expected copy motion response");
    };
    assert_eq!(cursor, PaneTextPoint { row: 0, col: 3 });
}

#[tokio::test]
async fn api_line_end_motion_keeps_ascii_wide_combining_and_blank_rows() {
    let (mut app, public_pane_id) = app_with_test_workspace();
    let pane_id = app.state.workspaces[0].tabs[0].root_pane;
    app.state.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
            20,
            5,
            1000,
            "abc\r\na界\r\nae\u{301}\r\n\r\nx".as_bytes(),
        ),
    );

    for (row, expected) in [(0, 2), (1, 1), (2, 1), (3, 0)] {
        let response = app.handle_pane_copy_motion(
            "line-end".into(),
            PaneCopyMotionParams {
                pane_id: public_pane_id.clone(),
                cursor: PaneTextPoint { row, col: 0 },
                motion: PaneCopyMotion::LineEnd,
                content_revision: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::PaneCopyMotion { cursor, .. } = success.result else {
            panic!("expected copy motion response");
        };
        assert_eq!(cursor, PaneTextPoint { row, col: expected }, "row {row}");
    }
}
