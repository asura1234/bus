use super::*;

#[test]
fn ctrl_click_routes_link_activation_through_endpoint_then_client_host() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("pane frame");
    let pane = state.hits.panes[0].clone();
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x + 2,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::CONTROL,
    };
    let activate = state.handle_raw_events(vec![RawInputEvent::Mouse(down)]);
    let [ClientShellAction::Endpoint { request, .. }] = &activate.actions[..] else {
        panic!("expected link activation request");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneLinkActivate(params)
            if params.pane_id == "pane_1" && params.viewport_row == 1 && params.col == 2
    ));

    let up = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        ..down
    };
    let held = state.handle_raw_events(vec![RawInputEvent::Mouse(up)]);
    assert!(held.requests.is_empty() && held.actions.is_empty());
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneLinkActivated {
            url: Some("https://example.test".to_owned()),
            handled: false,
        }),
    );
    assert!(matches!(
        &actions[..],
        [ClientShellAction::OpenSafeWebUrl(url)] if url == "https://example.test"
    ));
    assert!(!state.url_click_consumes_until_up);
}

#[test]
fn ctrl_click_without_a_link_replays_the_original_gesture() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("pane frame");
    let pane = state.hits.panes[0].clone();
    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x + 2,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::CONTROL,
    };
    let activate = state.handle_raw_events(vec![RawInputEvent::Mouse(down)]);
    let request_id = match &activate.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("expected link activation request"),
    };
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(crate::api::schema::ResponseResult::PaneLinkActivated {
            url: None,
            handled: false,
        }),
    );
    assert!(matches!(
        &actions[..],
        [ClientShellAction::ReplayMouse(events)] if events == &vec![down]
    ));
    let replay = match actions.into_iter().next().expect("replay action") {
        ClientShellAction::ReplayMouse(events) => state.replay_mouse_events(events),
        _ => unreachable!(),
    };
    assert!(matches!(
        &replay.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(request.method, crate::api::schema::Method::PaneFocus(_))
    ));
    assert!(state.selection.is_some());
}

#[test]
fn pane_split_drag_uses_projected_handle_and_stable_tab_path() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.splits.push(PaneSurfaceSplit {
        direction: PaneSurfaceSplitDirection::Horizontal,
        pos: 40,
        area: SurfaceRect {
            x: 0,
            y: 0,
            width: 80,
            height: 19,
        },
        hit_rect: SurfaceRect {
            x: 40,
            y: 0,
            width: 1,
            height: 19,
        },
        path: vec![false, true],
    });
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("split pane surface");
    let split = state.hits.pane_splits[0].clone();

    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: split.hit_rect.x,
        row: split.hit_rect.y + 2,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        state.chrome_drag,
        Some(ClientChromeDrag::PaneSplit { .. })
    ));
    let mut replacement = snapshot();
    replacement.revision = 2;
    replacement
        .tab_bar_right
        .push(crate::protocol::ClientShellTabStatusSegment {
            text: "updated".into(),
            accent: false,
        });
    let mut replacement_surface = surface();
    replacement_surface.projection_revision = 2;
    replacement_surface.splits.push(PaneSurfaceSplit {
        direction: PaneSurfaceSplitDirection::Horizontal,
        pos: 40,
        area: SurfaceRect {
            x: 0,
            y: 0,
            width: 80,
            height: 19,
        },
        hit_rect: SurfaceRect {
            x: 40,
            y: 0,
            width: 1,
            height: 19,
        },
        path: vec![false, true],
    });
    state.set_snapshot(Box::new(replacement));
    state.set_pane_surface(replacement_surface);
    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: split.area.x + 48,
        row: split.hit_rect.y + 2,
        modifiers: KeyModifiers::empty(),
    })]);
    let [ClientShellAction::Endpoint { request, .. }] = &drag.actions[..] else {
        panic!("pane split drag should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::LayoutSetSplitRatio(params)
            if params.tab_id.as_deref() == Some("tab_1")
                && params.path == vec![false, true]
                && (params.ratio - 0.6).abs() < f32::EPSILON
    ));
    let release =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: split.area.x + 48,
            row: split.hit_rect.y + 2,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(release.actions.is_empty());
    assert!(state.chrome_drag.is_none());
}

#[test]
fn client_double_click_selects_and_copies_endpoint_row_word() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    let click = || {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: pane.inner_rect.x + 1,
            row: pane.inner_rect.y,
            modifiers: KeyModifiers::empty(),
        })
    };
    let release = || {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: pane.inner_rect.x + 1,
            row: pane.inner_rect.y,
            modifiers: KeyModifiers::empty(),
        })
    };

    state.handle_raw_events(vec![click()]);
    state.handle_raw_events(vec![release()]);
    let second = state.handle_raw_events(vec![click()]);
    let ClientShellAction::Endpoint { request, .. } = second
        .actions
        .iter()
        .find(|action| {
            matches!(
                action,
                ClientShellAction::Endpoint { request, .. }
                    if matches!(request.method, crate::api::schema::Method::PaneSelectionRead(_))
            )
        })
        .expect("word-row read")
    else {
        unreachable!()
    };
    let word_request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneSelectionRead(params)
            if params.anchor == crate::api::schema::PaneTextPoint { row: 0, col: 0 }
                && params.cursor == crate::api::schema::PaneTextPoint { row: 0, col: 3 }
    ));

    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &word_request_id,
        Ok(crate::api::schema::ResponseResult::PaneSelection {
            pane_id: "pane_1".into(),
            text: "LIVE".into(),
        }),
    );
    assert!(repaint);
    assert!(state
        .selection
        .as_ref()
        .is_some_and(crate::selection::Selection::is_finalized));
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("auto-copy should read the selected word");
    };
    let copy_request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneSelectionRead(params)
            if params.anchor.col == 0 && params.cursor.col == 3
    ));
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &copy_request_id,
        Ok(crate::api::schema::ResponseResult::PaneSelection {
            pane_id: "pane_1".into(),
            text: "LIVE".into(),
        }),
    );
    assert!(matches!(
        &actions[..],
        [ClientShellAction::ClipboardWrite(bytes)] if bytes == b"LIVE"
    ));
}

#[test]
fn pane_content_updates_preserve_active_selection_only_when_selected_cells_stay_stable() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let surface_at = |surface_revision, content_revision, alternate_screen_active| {
        let mut pane_surface = surface();
        pane_surface.surface_revision = surface_revision;
        pane_surface.panes[0].content_revision = content_revision;
        pane_surface.panes[0].scroll = Some(crate::protocol::PaneSurfaceScrollMetrics {
            offset_from_bottom: 0,
            max_offset_from_bottom: 11,
            viewport_rows: 2,
        });
        pane_surface.panes[0].alternate_screen_active = alternate_screen_active;
        pane_surface
    };
    state.set_pane_surface(surface_at(1, 0, true));
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    let mouse = |kind, column, row| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        })
    };

    state.handle_raw_events(vec![mouse(
        MouseEventKind::Down(MouseButton::Left),
        pane.inner_rect.x,
        pane.inner_rect.y + 1,
    )]);
    let mut updated_surface = surface_at(2, 2, true);
    updated_surface.frame.cells[0].symbol = "W".into();
    state.set_pane_surface(updated_surface);
    state.compose(106, 20).expect("updated frame");

    let drag = state.handle_raw_events(vec![mouse(
        MouseEventKind::Drag(MouseButton::Left),
        pane.inner_rect.x + 1,
        pane.inner_rect.y + 1,
    )]);

    assert!(drag.repaint);
    let selection = state.selection.as_ref().expect("visible selection");
    assert!(selection.is_visible());
    assert_eq!(selection.ordered_cells(), ((12, 0), (12, 1)));

    state.selection = Some(crate::selection::Selection::absolute_anchor(
        "pane_1".to_owned(),
        (12, 0),
    ));
    let mut replaced_surface = surface_at(3, 4, true);
    replaced_surface.frame.cells[4].symbol = "X".into();
    state.set_pane_surface(replaced_surface);
    assert!(state.selection.is_none());

    for (surface_revision, content_revision, width, alternate_screen_active) in
        [(4, 6, 4, false), (5, 8, 3, false), (6, 9, 3, false)]
    {
        state.selection = Some(crate::selection::Selection::absolute_anchor(
            "pane_1".to_owned(),
            (12, 0),
        ));
        let mut changed_surface =
            surface_at(surface_revision, content_revision, alternate_screen_active);
        changed_surface.panes[0].inner_rect.width = width;
        changed_surface.panes[0].alternate_screen_active = alternate_screen_active;
        state.set_pane_surface(changed_surface);
        assert!(state.selection.is_none());
    }
}

#[test]
fn pane_mouse_input_keeps_stable_target_and_endpoint_encoding() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].mouse_reporting = true;
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();

    let click = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x + 2,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::ALT,
    })]);
    let [ClientMessage::ClientShellPaneInput { pane_id, events }] = &click.requests[..] else {
        panic!("pane application click should use targeted canonical input");
    };
    assert_eq!(pane_id, "pane_1");
    assert!(matches!(
        &events[..],
        [ClientPaneInputEvent::Mouse {
            kind: crate::protocol::ClientMouseKind::Down(
                crate::protocol::ClientMouseButton::Left
            ),
            position: ClientMousePosition::Cell { column: 2, row: 1 },
            modifiers,
            ..
        }] if *modifiers == KeyModifiers::ALT.bits()
    ));
    let moved = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Moved,
        column: 50,
        row: 10,
        modifiers: KeyModifiers::ALT,
    })]);
    assert!(moved.requests.is_empty());
    assert!(state.pane_mouse_gesture.is_some());
    state.hits.panes.clear();
    let release =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::ALT,
        })]);
    assert!(matches!(
        &release.requests[..],
        [ClientMessage::ClientShellPaneInput { pane_id, events }]
            if pane_id == "pane_1"
                && matches!(
                    &events[..],
                    [ClientPaneInputEvent::Mouse {
                        kind: crate::protocol::ClientMouseKind::Up(
                            crate::protocol::ClientMouseButton::Left
                        ),
                        ..
                    }]
                )
    ));
    assert!(state.pane_mouse_gesture.is_none());
}

#[test]
fn pane_pixel_mouse_preserves_pane_relative_pixel_coordinates() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].mouse_reporting = true;
    pane_surface.panes[0].sgr_pixel_mouse = true;
    pane_surface.panes[0].pixel_width = 39;
    pane_surface.panes[0].pixel_height = 38;
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    let geometry =
        crate::input::mouse::HostGeometry::new(106, 20, 1060, 400).expect("host geometry");
    let x = u32::from(pane.inner_rect.x) * 10 + 21;
    let y = u32::from(pane.inner_rect.y) * 20 + 21;
    let report = format!("\x1b[<0;{x};{y}M");

    let outcome = state.handle_pixel_mouse(report.as_bytes(), geometry);
    assert!(matches!(
        &outcome.requests[..],
        [ClientMessage::ClientShellPaneInput { pane_id, events }]
            if pane_id == "pane_1"
                && matches!(
                    &events[..],
                    [ClientPaneInputEvent::Mouse {
                        kind: crate::protocol::ClientMouseKind::Down(
                            crate::protocol::ClientMouseButton::Left
                        ),
                        position: ClientMousePosition::Pixels { x: 20, y: 20, .. },
                        ..
                    }]
                )
    ));

    let lost = state.handle_raw_events(vec![RawInputEvent::OuterFocusLost]);
    assert!(matches!(
        &lost.requests[..],
        [
            ClientMessage::ClientShellPaneInput { pane_id, events },
            ClientMessage::ClientShellFocus { focused: false }
        ] if pane_id == "pane_1" && matches!(
            &events[..],
            [ClientPaneInputEvent::Mouse {
                kind: crate::protocol::ClientMouseKind::Up(
                    crate::protocol::ClientMouseButton::Left
                ),
                position: ClientMousePosition::Pixels { x: 20, y: 20, .. },
                ..
            }]
        )
    ));
}
