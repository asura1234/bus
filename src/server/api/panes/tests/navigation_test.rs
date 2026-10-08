use super::*;

#[test]
fn api_pane_current_prefers_caller_pane_id() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.ensure_test_terminals();
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    let root_public = app.public_pane_id(0, root).unwrap();
    let right_public = app.public_pane_id(0, right).unwrap();

    let response = app.handle_pane_current(
        "req".into(),
        crate::protocol::api::schema::PaneCurrentParams {
            caller_pane_id: Some(right_public.clone()),
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneCurrent { pane } = success.result else {
        panic!("expected pane current response");
    };
    assert_eq!(pane.pane_id, right_public);
    assert!(!pane.focused);
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));
    assert_ne!(pane.pane_id, root_public);
}

#[test]
fn api_pane_current_falls_back_to_focused_pane() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    let root_public = app.public_pane_id(0, root).unwrap();

    let response = app.handle_pane_current(
        "req".into(),
        crate::protocol::api::schema::PaneCurrentParams::default(),
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneCurrent { pane } = success.result else {
        panic!("expected pane current response");
    };
    assert_eq!(pane.pane_id, root_public);
    assert!(pane.focused);
}

#[test]
fn api_pane_current_dispatches_through_socket_request() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let root_public = app.public_pane_id(0, root).unwrap();

    let response = app.handle_api_request(crate::protocol::api::schema::Request {
        id: "req".into(),
        method: crate::protocol::api::schema::Method::PaneCurrent(
            crate::protocol::api::schema::PaneCurrentParams::default(),
        ),
    });

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneCurrent { pane } = success.result else {
        panic!("expected pane current response");
    };
    assert_eq!(pane.pane_id, root_public);
}

#[test]
fn api_pane_current_reports_invalid_caller_pane_id() {
    let mut app = app_with_one_workspace();

    let response = app.handle_pane_current(
        "req".into(),
        crate::protocol::api::schema::PaneCurrentParams {
            caller_pane_id: Some("missing".into()),
        },
    );

    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "pane_not_found");
}

#[test]
fn api_pane_current_reports_no_active_pane() {
    let mut app = app_with_one_workspace();
    app.state.active = None;

    let response = app.handle_pane_current(
        "req".into(),
        crate::protocol::api::schema::PaneCurrentParams::default(),
    );

    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "pane_not_found");
}

#[test]
fn api_pane_swap_explicit_source_and_target_preserves_focus_and_returns_layout() {
    let mut app = app_with_one_workspace();
    let source = app.state.workspaces[0].tabs[0].root_pane;
    let target = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces[0].tabs[0].layout.focus_pane(source);
    crate::ui::compute_view_with_runtime_registry(
        &mut app.state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );
    let source_public = app.public_pane_id(0, source).unwrap();
    let target_public = app.public_pane_id(0, target).unwrap();

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
    assert!(swap.changed);
    assert_eq!(swap.reason, None);
    assert_eq!(swap.source_pane_id, source_public);
    assert_eq!(swap.target_pane_id, Some(target_public));
    assert_eq!(swap.focused_pane_id, swap.source_pane_id);
    assert_eq!(swap.layout.focused_pane_id, swap.source_pane_id);
    assert_eq!(swap.layout.panes.len(), 2);
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(source));
}

#[test]
fn api_pane_zoom_current_toggles_zoom() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let _right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    let root_public = app.public_pane_id(0, root).unwrap();

    let response = app.handle_pane_zoom("req".into(), PaneZoomParams::default());

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(zoom.changed);
    assert!(zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert_eq!(zoom.reason, None);
    assert_eq!(zoom.pane_id, root_public);
    assert_eq!(zoom.focused_pane_id, zoom.pane_id);
    assert!(zoom.zoomed);
    assert!(zoom.layout.zoomed);
    assert!(matches!(
        &app.event_hub.events_after(0).last().expect("layout event").1.data,
        EventData::LayoutUpdated { layout }
            if layout.tab_id == app.public_tab_id(0, 0).unwrap() && layout.zoomed
    ));

    let response = app.handle_pane_zoom("req".into(), PaneZoomParams::default());
    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(zoom.changed);
    assert!(zoom.zoom_changed);
    assert!(!zoom.focus_changed);
    assert!(!zoom.zoomed);
    assert!(!zoom.layout.zoomed);
    assert!(matches!(
        &app.event_hub.events_after(0).last().expect("layout event").1.data,
        EventData::LayoutUpdated { layout }
            if layout.tab_id == app.public_tab_id(0, 0).unwrap() && !layout.zoomed
    ));
}

#[test]
fn api_pane_zoom_idempotent_mode_reports_focus_change() {
    let mut app = app_with_one_workspace();
    app.state.active = Some(0);
    app.state.selected = 0;
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    app.state.workspaces[0].tabs[0].zoomed = true;
    let right_public = app.public_pane_id(0, right).unwrap();

    let response = app.handle_pane_zoom(
        "req".into(),
        PaneZoomParams {
            pane_id: Some(right_public),
            mode: PaneZoomMode::On,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneZoom { zoom } = success.result else {
        panic!("expected pane zoom response");
    };
    assert!(zoom.changed);
    assert!(!zoom.zoom_changed);
    assert!(zoom.focus_changed);
    assert_eq!(zoom.reason, Some(PaneZoomReason::AlreadyZoomed));
    assert!(zoom.zoomed);
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
    assert!(matches!(
        &app.event_hub.events_after(0).last().expect("layout event").1.data,
        EventData::LayoutUpdated { layout }
            if layout.focused_pane_id == app.public_pane_id(0, right).unwrap()
    ));
}

#[test]
fn api_pane_resize_changes_target_ratio_without_changing_focus() {
    let mut app = app_with_one_workspace();
    let root = app.state.workspaces[0].tabs[0].root_pane;
    let right = app.state.workspaces[0].test_split(ratatui::layout::Direction::Horizontal);
    app.state.workspaces[0].tabs[0].layout.focus_pane(right);
    crate::ui::compute_view_with_runtime_registry(
        &mut app.state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );
    let root_public = app.public_pane_id(0, root).unwrap();
    let right_public = app.public_pane_id(0, right).unwrap();

    let response = app.handle_pane_resize(
        "req".into(),
        crate::protocol::api::schema::PaneResizeParams {
            pane_id: Some(root_public.clone()),
            direction: PaneDirection::Right,
            amount: Some(0.1),
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneResize { resize } = success.result else {
        panic!("expected pane resize response");
    };
    assert!(resize.changed);
    assert_eq!(resize.reason, None);
    assert_eq!(resize.pane_id, root_public);
    assert_eq!(resize.focused_pane_id, right_public);
    assert_eq!(resize.layout.focused_pane_id, right_public);
    assert!((resize.layout.splits[0].ratio - 0.6).abs() < f32::EPSILON);
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
    assert!(matches!(
        &app.event_hub.events_after(0).last().expect("layout event").1.data,
        EventData::LayoutUpdated { layout }
            if layout.tab_id == app.public_tab_id(0, 0).unwrap()
                && (layout.splits[0].ratio - 0.6).abs() < f32::EPSILON
    ));
}

#[test]
fn api_pane_focus_direction_focuses_neighbor() {
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

    let response = app.handle_pane_focus_direction(
        "req".into(),
        crate::protocol::api::schema::PaneFocusDirectionParams {
            pane_id: Some(root_public.clone()),
            direction: PaneDirection::Right,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneFocusDirection { focus } = success.result else {
        panic!("expected pane focus direction response");
    };
    assert!(focus.changed);
    assert_eq!(focus.reason, None);
    assert_eq!(focus.source_pane_id, root_public);
    assert_eq!(focus.focused_pane_id, Some(right_public.clone()));
    assert_eq!(focus.layout.focused_pane_id, right_public);
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(right));
}

#[test]
fn api_pane_focus_focuses_direct_target_across_tabs_and_workspaces() {
    let mut app = app_with_one_workspace();
    app.state.workspaces.push(Workspace::test_new("other"));
    let target_tab_idx = app.state.workspaces[1].test_add_tab(Some("target"));
    app.state.workspaces[1].switch_tab(target_tab_idx);
    let target_pane = app.state.workspaces[1].tabs[target_tab_idx].root_pane;
    app.state.ensure_test_terminals();
    let target_public = app.public_pane_id(1, target_pane).unwrap();
    app.state.switch_workspace(0);
    assert_eq!(app.state.active, Some(0));

    let response = app.handle_pane_focus(
        "req".into(),
        crate::protocol::api::schema::PaneTarget {
            pane_id: target_public.clone(),
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneInfo { pane } = success.result else {
        panic!("expected pane info response");
    };
    assert_eq!(pane.pane_id, target_public);
    assert_eq!(app.state.active, Some(1));
    assert_eq!(app.state.workspaces[1].active_tab, target_tab_idx);
    assert_eq!(app.state.workspaces[1].focused_pane_id(), Some(target_pane));
    assert_eq!(app.state.mode, Mode::Terminal);
}

#[test]
fn api_pane_focus_rejects_invalid_pane_id() {
    let mut app = app_with_one_workspace();

    let response = app.handle_pane_focus(
        "req".into(),
        crate::protocol::api::schema::PaneTarget {
            pane_id: "pane_missing".into(),
        },
    );

    let error: ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(error.error.code, "pane_not_found");
}

#[test]
fn api_pane_focus_direction_no_neighbor_is_noop() {
    let mut app = app_with_one_workspace();
    let root = app.state.workspaces[0].tabs[0].root_pane;
    app.state.workspaces[0].tabs[0].layout.focus_pane(root);
    crate::ui::compute_view_with_runtime_registry(
        &mut app.state,
        &crate::terminal::TerminalRuntimeRegistry::new(),
        ratatui::layout::Rect::new(0, 0, 100, 20),
    );
    let root_public = app.public_pane_id(0, root).unwrap();

    let response = app.handle_pane_focus_direction(
        "req".into(),
        crate::protocol::api::schema::PaneFocusDirectionParams {
            pane_id: Some(root_public.clone()),
            direction: PaneDirection::Left,
        },
    );

    let success: SuccessResponse = serde_json::from_str(&response).unwrap();
    let ResponseResult::PaneFocusDirection { focus } = success.result else {
        panic!("expected pane focus direction response");
    };
    assert!(!focus.changed);
    assert_eq!(focus.reason, Some(PaneFocusDirectionReason::NoNeighbor));
    assert_eq!(focus.source_pane_id, root_public.clone());
    assert_eq!(focus.focused_pane_id, Some(root_public));
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));
}
