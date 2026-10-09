use super::*;

#[test]
fn client_mouse_selection_highlights_and_copies_through_endpoint_extraction() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();

    let down = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x,
        row: pane.inner_rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &down.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::protocol::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
            )
    ));
    assert!(state
        .selection
        .as_ref()
        .is_some_and(|selection| !selection.is_visible()));

    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: pane.inner_rect.x + 2,
        row: pane.inner_rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(drag.repaint);
    assert!(state
        .selection
        .as_ref()
        .is_some_and(crate::utils::text::selection::Selection::is_visible));
    let selected = state.compose(106, 20).expect("selected frame");
    let selected_cell =
        &selected.cells[usize::from(pane.inner_rect.y) * 106 + usize::from(pane.inner_rect.x)];
    assert_ne!(
        selected_cell.bg,
        crate::protocol::wire::color_to_u32(ratatui::style::Color::Reset)
    );

    let release =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: pane.inner_rect.x + 2,
            row: pane.inner_rect.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(state.selection.is_none());
    let [ClientShellAction::Endpoint { request, .. }] = &release.actions[..] else {
        panic!("selection release should request endpoint extraction");
    };
    let request_id = request.id.clone();
    assert!(matches!(
        &request.method,
        crate::protocol::api::schema::Method::PaneSelectionRead(params)
            if params.pane_id == "pane_1"
                && params.anchor == crate::protocol::api::schema::PaneTextPoint { row: 0, col: 0 }
                && params.cursor == crate::protocol::api::schema::PaneTextPoint { row: 0, col: 2 }
    ));

    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(
            crate::protocol::api::schema::ResponseResult::PaneSelection {
                pane_id: "pane_1".into(),
                text: "LIV".into(),
            },
        ),
    );
    assert!(repaint);
    assert!(matches!(
        &actions[..],
        [ClientShellAction::ClipboardWrite(bytes)] if bytes == b"LIV"
    ));
    assert_eq!(
        state
            .copy_feedback
            .as_ref()
            .map(|feedback| feedback.message.as_str()),
        Some("copied to clipboard")
    );
}

#[test]
fn clipboard_feedback_is_client_local_and_respects_config() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let now = std::time::Instant::now();
    assert!(state.show_copy_feedback(now));
    assert_eq!(
        state
            .copy_feedback
            .as_ref()
            .map(|feedback| feedback.message.as_str()),
        Some("copied to clipboard")
    );
    assert_eq!(
        state.copy_feedback_deadline,
        Some(now + std::time::Duration::from_secs(2))
    );

    state.config.clipboard_toast_enabled = false;
    state.copy_feedback = None;
    state.copy_feedback_deadline = None;
    assert!(!state.show_copy_feedback(now));
    assert!(state.copy_feedback.is_none());
    assert!(state.copy_feedback_deadline.is_none());
}

#[test]
fn retained_mouse_selection_survives_output_and_copies_without_terminal_input() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.copy_on_select = false;
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    for event in [
        crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: pane.inner_rect.x,
            row: pane.inner_rect.y,
            modifiers: KeyModifiers::empty(),
        },
        crossterm::event::MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: pane.inner_rect.x + 2,
            row: pane.inner_rect.y,
            modifiers: KeyModifiers::empty(),
        },
        crossterm::event::MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: pane.inner_rect.x + 2,
            row: pane.inner_rect.y,
            modifiers: KeyModifiers::empty(),
        },
    ] {
        state.handle_raw_events(vec![RawInputEvent::Mouse(event)]);
        // Output can arrive between drag and release, including an in-flight revision.
        let mut updated = state.pane_surface.clone().expect("pane surface");
        updated.panes[0].content_revision += 1;
        updated.frame.cells[0].symbol = "x".into();
        state.set_pane_surface(updated);
    }
    assert!(state
        .selection
        .as_ref()
        .is_some_and(crate::utils::text::selection::Selection::is_finalized));

    // A patch that redraws selected text must retain the same live terminal range.
    let mut updated = state.pane_surface.clone().expect("pane surface");
    updated.panes[0].content_revision += 1;
    let mut cell = updated.frame.cells[0].clone();
    cell.symbol = "y".into();
    assert!(matches!(
        state.apply_pane_surface_patch(crate::protocol::wire::PaneSurfacePatch {
            boot_id: updated.boot_id,
            projection_revision: updated.projection_revision,
            base_surface_revision: updated.surface_revision,
            surface_revision: updated.surface_revision + 1,
            panes: updated.panes,
            rows: vec![crate::protocol::wire::PaneSurfacePatchRow {
                x: 0,
                y: 0,
                cells: vec![cell]
            }],
            cursor: updated.frame.cursor,
        }),
        crate::client::compositor::patch::ClientPaneSurfacePatchOutcome::Applied(_)
    ));
    assert!(state
        .selection
        .as_ref()
        .is_some_and(crate::utils::text::selection::Selection::is_finalized));

    let highlighted = state.compose(106, 20).expect("highlighted frame");
    let cell_index = usize::from(pane.inner_rect.y) * 106 + usize::from(pane.inner_rect.x);
    let selected_cell = highlighted.cells[cell_index].clone();
    let selection = state.selection.take();
    let unselected = state.compose(106, 20).expect("unselected frame");
    assert_ne!(selected_cell.bg, unselected.cells[cell_index].bg);
    state.selection = selection;

    let copy = state.handle_raw_events(vec![RawInputEvent::Key(
        crate::protocol::keys::TerminalKey::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    )]);
    assert!(state.selection.is_none());
    assert!(matches!(
        &copy.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(request.method, crate::protocol::api::schema::Method::PaneSelectionRead(
                crate::protocol::api::schema::PaneSelectionReadParams { content_revision: None, .. }
            ))
    ));
    assert!(copy.requests.is_empty());
    let request_id = match &copy.actions[0] {
        ClientShellAction::Endpoint { request, .. } => request.id.clone(),
        _ => unreachable!(),
    };
    let (_, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(
            crate::protocol::api::schema::ResponseResult::PaneSelection {
                pane_id: "pane_1".into(),
                text: "yIV".into(),
            },
        ),
    );
    assert!(matches!(&actions[..], [ClientShellAction::ClipboardWrite(bytes)] if bytes == b"yIV"));
}

#[test]
fn selection_edge_drag_requests_scroll_and_timer_continues_it() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::wire::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 20,
        viewport_rows: 2,
    });
    // Leave a row above the pane so the drag can leave its top edge.
    pane_surface.panes[0].rect.y = 1;
    pane_surface.panes[0].inner_rect.y = 1;
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::empty(),
    })]);
    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: pane.inner_rect.x,
        row: pane.inner_rect.y.saturating_sub(1),
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(matches!(
        &drag.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::protocol::api::schema::Method::PaneScroll(params)
                    if params.offset_from_bottom == 3
            )
    ));
    let drag_request_id = match &drag.actions[0] {
        ClientShellAction::Endpoint { request, .. } => request.id.clone(),
        _ => unreachable!(),
    };
    let now = std::time::Instant::now();
    state.selection_autoscroll_deadline = Some(now);
    let tick = state.tick_selection_autoscroll(now);
    assert!(tick.actions.is_empty());
    let (_, next_scroll) =
        state.handle_endpoint_result("boot-1", &drag_request_id, Ok(pane_scroll_result(3, 20, 3)));
    assert!(matches!(
        &next_scroll[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::protocol::api::schema::Method::PaneScroll(params)
                    if params.offset_from_bottom == 4
            )
    ));
}

#[test]
fn retained_selection_copy_suppresses_key_repeats() {
    let mut config = Config::default();
    config.ui.copy_on_select = false;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut selection = crate::utils::text::selection::Selection::absolute_range(
        "pane_1".to_owned(),
        (0, 0),
        (0, 1),
    );
    assert!(selection.finish());
    state.selection = Some(selection);

    let key = crate::protocol::keys::TerminalKey::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let press = state.handle_raw_events(vec![RawInputEvent::Key(key.clone())]);
    assert!(press.actions.iter().any(|action| matches!(
        action,
        ClientShellAction::Endpoint { request, .. }
            if matches!(request.method, crate::protocol::api::schema::Method::PaneSelectionRead(_))
    )));
    let repeat = state.handle_raw_events(vec![RawInputEvent::Key(
        key.clone()
            .with_kind(crossterm::event::KeyEventKind::Repeat),
    )]);
    assert!(repeat.actions.is_empty());
    assert!(repeat.requests.is_empty());
    let release = state.handle_raw_events(vec![RawInputEvent::Key(
        key.with_kind(crossterm::event::KeyEventKind::Release),
    )]);
    assert!(release.actions.is_empty());
    assert!(release.requests.is_empty());
}

#[test]
fn word_selection_result_survives_focus_snapshot_lag() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");
    let hit = state.hits.panes[0].clone();
    let mut request = ClientShellInput::default();
    state.request_word_selection(&hit, 0, 1, &mut request);
    let request_id = match &request.actions[0] {
        ClientShellAction::Endpoint { request, .. } => request.id.clone(),
        _ => unreachable!(),
    };
    let mut lagging = snapshot();
    lagging.focused_pane_id = None;
    lagging.panes[0].focused = false;
    state.set_snapshot(Box::new(lagging));
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Ok(
            crate::protocol::api::schema::ResponseResult::PaneSelection {
                pane_id: "pane_1".into(),
                text: "hello world".into(),
            },
        ),
    );
    assert!(repaint);
    assert!(state
        .selection
        .as_ref()
        .is_some_and(crate::utils::text::selection::Selection::is_visible));
}

#[test]
fn selection_wheel_accumulates_scroll_steps_before_endpoint_acknowledgement() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.mouse_scroll_lines = 3;
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::wire::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 20,
        viewport_rows: 2,
    });
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    let pointer = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x + 1,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::empty(),
    };
    state.handle_raw_events(vec![RawInputEvent::Mouse(pointer)]);
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        ..pointer
    };
    let first = state.handle_raw_events(vec![RawInputEvent::Mouse(wheel)]);
    let request_id = match &first.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("expected initial selection scroll"),
    };
    state.handle_raw_events(vec![RawInputEvent::Mouse(wheel)]);
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &request_id, Ok(pane_scroll_result(3, 20, 2)));
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected accumulated queued selection scroll");
    };
    assert!(
        matches!(
            &request.method,
            crate::protocol::api::schema::Method::PaneScroll(params)
                if params.offset_from_bottom == 6
        ),
        "two wheel steps should scroll six rows, got {:?}",
        request.method
    );
}

#[test]
fn selection_wheel_during_edge_autoscroll_continues_from_the_wheel_position() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.config.mouse_scroll_lines = 3;
    state.set_snapshot(Box::new(snapshot()));
    let mut pane_surface = surface();
    pane_surface.panes[0].scroll = Some(crate::protocol::wire::PaneSurfaceScrollMetrics {
        offset_from_bottom: 0,
        max_offset_from_bottom: 20,
        viewport_rows: 2,
    });
    // Leave a row above the pane so the drag can leave its top edge.
    pane_surface.panes[0].rect.y = 1;
    pane_surface.panes[0].inner_rect.y = 1;
    state.set_pane_surface(pane_surface);
    state.compose(106, 20).expect("composed frame");
    let pane = state.hits.panes[0].clone();
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: pane.inner_rect.x,
        row: pane.inner_rect.y + 1,
        modifiers: KeyModifiers::empty(),
    })]);
    let above = MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: pane.inner_rect.x,
        row: pane.inner_rect.y.saturating_sub(1),
        modifiers: KeyModifiers::empty(),
    };
    let drag = state.handle_raw_events(vec![RawInputEvent::Mouse(above)]);
    let request_id = match &drag.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        _ => panic!("expected edge autoscroll request"),
    };
    state.handle_raw_events(vec![RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        ..above
    })]);
    let now = std::time::Instant::now();
    state.selection_autoscroll_deadline = Some(now);
    state.tick_selection_autoscroll(now);
    let (_, actions) =
        state.handle_endpoint_result("boot-1", &request_id, Ok(pane_scroll_result(3, 20, 2)));
    let [ClientShellAction::Endpoint { request, .. }] = &actions[..] else {
        panic!("expected queued selection scroll");
    };
    assert!(
        matches!(
            &request.method,
            crate::protocol::api::schema::Method::PaneScroll(params)
                if params.offset_from_bottom == 7
        ),
        "autoscroll should continue one row past the wheel step, got {:?}",
        request.method
    );
}
