use super::*;

#[test]
fn api_pane_swap_direction_no_neighbor_returns_unchanged_layout() {
    let mut app = app_with_one_workspace();
    let source = app.state.workspaces[0].tabs[0].root_pane;
    app.state.workspaces[0].tabs[0].layout.focus_pane(source);
    crate::ui::compute_view_with_runtime_registry(
        &mut app.state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );
    let source_public = app.public_pane_id(0, source).unwrap();

    let response = app.handle_pane_swap(
        "req".into(),
        PaneSwapParams {
            pane_id: Some(source_public.clone()),
            direction: Some(PaneDirection::Left),
            ..PaneSwapParams::default()
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneSwap { swap } = success.result else {
        panic!("expected pane swap response");
    };
    assert!(!swap.changed);
    assert_eq!(swap.reason, Some(PaneSwapReason::NoNeighbor));
    assert_eq!(swap.source_pane_id, source_public);
    assert_eq!(swap.target_pane_id, None);
    assert_eq!(swap.layout.panes.len(), 1);
    assert!(app.event_hub.events_after(0).is_empty());
}

#[test]
fn api_pane_swap_explicit_missing_target_returns_not_found_noop() {
    let mut app = app_with_one_workspace();
    let source = app.state.workspaces[0].tabs[0].root_pane;
    let source_public = app.public_pane_id(0, source).unwrap();

    let response = app.handle_pane_swap(
        "req".into(),
        PaneSwapParams {
            source_pane_id: Some(source_public.clone()),
            target_pane_id: Some("missing-pane".into()),
            ..PaneSwapParams::default()
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneSwap { swap } = success.result else {
        panic!("expected pane swap response");
    };
    assert!(!swap.changed);
    assert_eq!(swap.reason, Some(PaneSwapReason::NotFound));
    assert_eq!(swap.source_pane_id, source_public);
    assert_eq!(swap.target_pane_id, Some("missing-pane".into()));
    assert_eq!(swap.layout.panes.len(), 1);
}

#[test]
fn api_pane_swap_explicit_missing_source_returns_not_found_noop() {
    let mut app = app_with_one_workspace();
    let target = app.state.workspaces[0].tabs[0].root_pane;
    let target_public = app.public_pane_id(0, target).unwrap();

    let response = app.handle_pane_swap(
        "req".into(),
        PaneSwapParams {
            source_pane_id: Some("missing-pane".into()),
            target_pane_id: Some(target_public.clone()),
            ..PaneSwapParams::default()
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneSwap { swap } = success.result else {
        panic!("expected pane swap response");
    };
    assert!(!swap.changed);
    assert_eq!(swap.reason, Some(PaneSwapReason::NotFound));
    assert_eq!(swap.source_pane_id, "missing-pane");
    assert_eq!(swap.target_pane_id, Some(target_public));
    assert_eq!(swap.layout.panes.len(), 1);
}

#[test]
fn api_pane_swap_explicit_cross_workspace_preserves_target_id() {
    let mut app = app_with_one_workspace();
    app.state.workspaces.push(Workspace::test_new("other"));
    let source = app.state.workspaces[0].tabs[0].root_pane;
    let target = app.state.workspaces[1].tabs[0].root_pane;
    let source_public = app.public_pane_id(0, source).unwrap();
    let target_public = app.public_pane_id(1, target).unwrap();

    let response = app.handle_pane_swap(
        "req".into(),
        PaneSwapParams {
            source_pane_id: Some(source_public.clone()),
            target_pane_id: Some(target_public.clone()),
            ..PaneSwapParams::default()
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneSwap { swap } = success.result else {
        panic!("expected pane swap response");
    };
    assert!(!swap.changed);
    assert_eq!(swap.reason, Some(PaneSwapReason::CrossTab));
    assert_eq!(swap.source_pane_id, source_public);
    assert_eq!(swap.target_pane_id, Some(target_public));
    assert_eq!(swap.layout.workspace_id, app.public_workspace_id(0));
}

#[test]
fn api_pane_zoom_single_pane_returns_noop() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let root_public = app.public_pane_id(0, root).unwrap();

    let response = app.handle_pane_zoom(
        "req".into(),
        PaneZoomParams {
            pane_id: Some(root_public.clone()),
            mode: PaneZoomMode::Toggle,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(!zoom.changed);
    assert!(!zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert_eq!(zoom.reason, Some(PaneZoomReason::SinglePane));
    assert_eq!(zoom.pane_id, root_public);
    assert!(!zoom.zoomed);
    assert!(!app.state.workspaces[0].tabs[0].zoomed);
}

#[test]
fn api_pane_zoom_on_and_off_are_idempotent() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let _right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    let root_public = app.public_pane_id(0, root).unwrap();

    let response = app.handle_pane_zoom(
        "req".into(),
        PaneZoomParams {
            pane_id: Some(root_public.clone()),
            mode: PaneZoomMode::On,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(zoom.changed);
    assert!(zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert!(zoom.zoomed);

    let response = app.handle_pane_zoom(
        "req".into(),
        PaneZoomParams {
            pane_id: Some(root_public.clone()),
            mode: PaneZoomMode::On,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(!zoom.changed);
    assert!(!zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert_eq!(zoom.reason, Some(PaneZoomReason::AlreadyZoomed));
    assert!(zoom.zoomed);

    let response = app.handle_pane_zoom(
        "req".into(),
        PaneZoomParams {
            pane_id: Some(root_public),
            mode: PaneZoomMode::Off,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(zoom.changed);
    assert!(zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert!(!zoom.zoomed);

    let response = app.handle_pane_zoom(
        "req".into(),
        PaneZoomParams {
            pane_id: None,
            mode: PaneZoomMode::Off,
        },
    );
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(!zoom.changed);
    assert!(!zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert_eq!(zoom.reason, Some(PaneZoomReason::AlreadyUnzoomed));
    assert!(!zoom.zoomed);
}

#[test]
fn api_pane_zoom_params_serialize_modes() {
    let request = crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneZoom(PaneZoomParams {
            pane_id: Some("issue-1".into()),
            mode: PaneZoomMode::On,
        }),
    };

    let encoded = serde_json::to_string(&request).unwrap();
    assert!(encoded.contains("\"method\":\"pane.zoom\""));
    assert!(encoded.contains("\"mode\":\"on\""));

    let decoded: crate::protocol::api::schema::Request = serde_json::from_str(&encoded).unwrap();
    let crate::protocol::api::schema::Method::PaneZoom(params) = decoded.method else {
        panic!("expected pane zoom request");
    };
    assert_eq!(params.pane_id, Some("issue-1".into()));
    assert_eq!(params.mode, PaneZoomMode::On);
}

#[test]
fn api_pane_layout_returns_public_ids_rects_and_splits() {
    let mut app = app_with_one_workspace();
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    crate::ui::compute_view_with_runtime_registry(
        &mut app.state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );
    let root_public = app.public_pane_id(0, root).unwrap();
    let right_public = app.public_pane_id(0, right).unwrap();

    let response = app.handle_pane_layout(
        "req".into(),
        crate::protocol::api::schema::PaneLayoutParams {
            pane_id: Some(root_public.clone()),
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneLayout { layout } = success.result else {
        panic!("expected pane layout response");
    };
    assert_eq!(layout.focused_pane_id, root_public);
    assert!(layout.panes.iter().any(|pane| pane.pane_id == root_public));
    assert!(layout.panes.iter().any(|pane| pane.pane_id == right_public));
    assert_eq!(layout.splits.len(), 1);
    assert_eq!(
        layout.splits[0].direction,
        crate::protocol::api::schema::SplitDirection::Right
    );
}
