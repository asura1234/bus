#[test]
fn composer_box_encloses_controls_and_grows_with_matching_column_divider() {
    let (mut ui, room, _) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        ui.compute_view(cols, rows);
        let mut buffer =
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        ui.render(&mut buffer);
        let editor = composer_rect(&ui);
        let top = editor.y - 3;
        let left = editor.x - 1;
        let right = editor.right();
        let bottom = editor.bottom() + 1;
        let divider = ui.view.sidebar.right() - 1;
        let color = buffer[(divider, 0)].fg;
        assert_eq!(buffer[(divider, 0)].symbol(), "│");
        assert_ne!(color, buffer[(editor.x, editor.y)].fg);
        for y in 0..rows {
            assert_eq!(buffer[(divider, y)].symbol(), "│");
            assert_eq!(buffer[(divider, y)].fg, color);
        }
        for (x, y, symbol) in [
            (left, top, "┌"),
            (right, top, "┐"),
            (left, bottom, "└"),
            (right, bottom, "┘"),
        ] {
            assert_eq!(buffer[(x, y)].symbol(), symbol);
            assert_eq!(buffer[(x, y)].fg, color);
        }
        for x in left + 1..right {
            for y in [top, bottom] {
                assert_eq!(buffer[(x, y)].symbol(), "─");
                assert_eq!(buffer[(x, y)].fg, color);
            }
        }
        for y in top + 1..bottom {
            for x in [left, right] {
                let symbol = if y == editor.y - 1 {
                    if x == left {
                        "├"
                    } else {
                        "┤"
                    }
                } else {
                    "│"
                };
                assert_eq!(buffer[(x, y)].symbol(), symbol);
                assert_eq!(buffer[(x, y)].fg, color);
            }
        }
        assert_eq!(buffer[(editor.x, top + 1)].symbol(), "+");
        assert_eq!(buffer[(editor.x + 3, top + 1)].symbol(), "@");
    }
}

#[test]
fn routine_footer_hints_are_hidden_in_rooms_and_forms() {
    let (mut ui, _, _) = fixture();
    for form in [false, true] {
        if form {
            ui.action(render::Action::NewRoom);
        }
        let screen = room_screen(&mut ui, 120, 40);
        for hint in ["Ctrl+Q quit", "F2 rename", "Enter send", "Tab fields"] {
            assert!(!screen.contains(hint), "persistent shortcut: {hint}");
        }
    }
}

#[test]
fn composer_toolbar_has_a_matching_horizontal_line_above_editable_text() {
    let (mut ui, room, _) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        ui.compute_view(cols, rows);
        let mut buffer =
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        ui.render(&mut buffer);
        let editor = composer_rect(&ui);
        let color = buffer[(ui.view.sidebar.right() - 1, 0)].fg;
        for x in editor.x..editor.right() {
            assert_eq!(buffer[(x, editor.y - 1)].symbol(), "─");
            assert_eq!(buffer[(x, editor.y - 1)].fg, color);
        }
        assert_eq!(buffer[(editor.x - 1, editor.y - 1)].symbol(), "├");
        assert_eq!(buffer[(editor.right(), editor.y - 1)].symbol(), "┤");
        assert_eq!(buffer[(editor.x, editor.y - 2)].symbol(), "+");
        assert_eq!(buffer[(editor.x + 3, editor.y - 2)].symbol(), "@");
        assert_eq!(buffer[(editor.x, editor.y)].symbol(), "d");
    }
}

/// The composer's bottom border is the window's last row, so no row exists
/// below it for any notice, search or queue status.
fn assert_composer_on_last_row(ui: &mut BusUi, cols: u16, rows: u16, case: &str) -> String {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    let editor = composer_rect(ui);
    assert_eq!(
        editor.bottom() + 1,
        rows - 1,
        "{case}: row under the composer"
    );
    assert_eq!(buffer[(editor.x - 1, rows - 1)].symbol(), "└", "{case}");
    assert_eq!(buffer[(editor.right(), rows - 1)].symbol(), "┘", "{case}");
    for x in editor.x..editor.right() {
        assert_eq!(
            buffer[(x, rows - 1)].symbol(),
            "─",
            "{case}: text under the composer"
        );
    }
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

#[test]
fn nothing_is_drawn_under_the_composer() {
    let (mut ui, room, agent) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        assert_composer_on_last_row(&mut ui, cols, rows, "idle");

        ui.error = Some("Save failed".into());
        let screen = assert_composer_on_last_row(&mut ui, cols, rows, "save error");
        assert!(
            !screen.contains("Save failed"),
            "errors never title the composer:\n{screen}"
        );
        ui.error = None;

        assert_composer_on_last_row(&mut ui, cols, rows, "pending send");
    }

    // An agent notice goes to the client log, not the screen.
    let mut snapshot = (*ui.snapshot).clone();
    let notice = "Provider turn ended without a captured reply; request closed.";
    snapshot
        .state
        .set_agent_error(agent, Some(notice.into()))
        .unwrap();
    snapshot.revision += 1;
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| ui.receive_snapshot(Arc::new(snapshot)));
    assert!(
        capture.text().contains("bus.agent.notice"),
        "{}",
        capture.text()
    );
    let screen = assert_composer_on_last_row(&mut ui, 100, 30, "agent notice");
    assert!(!screen.contains("captured reply"), "{screen}");

    // Queued requests are counted nowhere under the composer.
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    for text in ["one", "two", "three"] {
        snapshot.state.set_draft_text(room, text).unwrap();
        snapshot.state.submit_draft_queued(room, 1).unwrap();
    }
    assert!(!snapshot.state.queued_requests(agent).is_empty());
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let screen = assert_composer_on_last_row(&mut ui, 100, 30, "queued requests");
    assert!(!screen.contains("queued requests"), "{screen}");

    // The Ctrl+R recall search titles the composer instead.
    key(&mut ui, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(ui.history_search.is_some());
    let screen = assert_composer_on_last_row(&mut ui, 100, 30, "recall search");
    assert!(screen.contains("search   1/"), "{screen}");
}

#[test]
fn history_scroll_moves_one_row_per_event() {
    use crossterm::event::MouseEventKind::ScrollUp;
    let (mut ui, room, agent) = fixture();
    let history = (0..80)
        .map(|i| format!("history-{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, &history).unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.submit_draft(room, 10).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    room_screen(&mut ui, 100, 30);
    let bottom = ui.main_scroll;
    assert!(bottom > 1);

    mouse(&mut ui, ScrollUp, 40, 12);

    assert_eq!(ui.main_scroll, bottom - 1);
}

#[test]
fn history_scrolls_and_clamps_without_moving_or_editing_the_composer() {
    use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
    let (mut ui, room, agent) = fixture();
    let history = (0..80)
        .map(|i| format!("history-{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, &history).unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.submit_draft(room, 10).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("unsent draft");
    assert!(room_screen(&mut ui, 100, 30).contains("history-79"));
    let composer = composer_rect(&ui);
    for _ in 0..80 {
        mouse(&mut ui, ScrollDown, 40, 12);
        ui.compute_view(100, 30);
    }
    let bottom = room_screen(&mut ui, 100, 30);
    assert!(
        bottom.contains("history-79"),
        "last history line must remain visible after repeated scrolling"
    );
    assert!(!bottom.contains("history-00"));
    assert_eq!(composer_rect(&ui), composer);
    assert_eq!(ui.locals[&room].text.text, "unsent draft");
    assert_eq!(ui.locals[&room].composer_scroll, None);
    assert_eq!(ui.sidebar_scroll, 0);
    for _ in 0..80 {
        mouse(&mut ui, ScrollUp, 40, 12);
        ui.compute_view(100, 30);
    }
    assert!(room_screen(&mut ui, 100, 30).contains("history-00"));
    mouse(&mut ui, ScrollDown, 40, 1); // The room header is not history.
    assert_eq!(ui.main_scroll, 0);
}

#[test]
fn composer_full_height_picker_covers_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"underlying-draft-suffix-should-not-bleed-through\n".repeat(80));
    ui.compute_view(100, 30);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    ui.compute_view(100, 30);
    let hit = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::Recipient(None))
        .unwrap();
    let rect = hit.rect;
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let label: String = (rect.x..rect.right())
        .map(|x| buffer[(x, rect.y)].symbol())
        .collect();
    assert_eq!(label.trim(), "[ ] All");
}

#[test]
fn direct_text_render_filters_terminal_controls_without_changing_source() {
    let (mut ui, room, _) = fixture();
    let dangerous = "x\u{1b}]52;c;ZXZpbA==\u{7}y";
    ui.locals.get_mut(&room).unwrap().text.insert(dangerous);
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    assert!(buffer
        .content
        .iter()
        .all(|cell| !cell.symbol().chars().any(char::is_control)));
    assert_eq!(ui.locals[&room].text.text, dangerous);
}

#[test]
fn bus_presentation_1_and_15_agents_stays_bounded_to_visible_rows() {
    for count in [1, 15] {
        let (mut ui, room, _) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        for index in 1..count {
            snapshot
                .state
                .create_agent(
                    room,
                    &format!("agent{index}"),
                    Provider::ClaudeCode,
                    "/does/not/exist".into(),
                    None,
                )
                .unwrap();
        }
        let recipients: Vec<_> = snapshot.state.agents().map(|agent| agent.id).collect();
        snapshot
            .state
            .set_draft_recipients(room, recipients)
            .unwrap();
        snapshot
            .state
            .set_draft_text(
                room,
                "Exercise visible recipient names in the history header",
            )
            .unwrap();
        snapshot.state.submit_draft(room, 1).unwrap();
        ui.receive_snapshot(Arc::new(snapshot));
        let started = std::time::Instant::now();
        for _ in 0..200 {
            ui.compute_view(100, 30);
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
            ui.render(&mut buffer);
            std::hint::black_box(buffer);
        }
        println!(
            "Bus fixed 100x30, agents={count}, 200 compute+render frames: {:?}",
            started.elapsed()
        );
        // The whole-column viewport now uses the former empty footer rows,
        // but hit targets must still be bounded by visible content, not agents.
        // Name, status and delete can share a row; targets stay viewport-bounded.
        assert!(ui.view.hits.len() <= 3 * 30);
        assert!(ui
            .view
            .hits
            .iter()
            .all(|hit| hit.rect.bottom() <= 30 && hit.rect.right() <= 100));
        assert_eq!(ui.snapshot.state.agents().count(), count);
    }
}

#[test]
fn switching_bus_screens_requests_one_full_repaint() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    ui.full_repaint = false;
    ui.compute_view(100, 30);
    assert!(!ui.full_repaint, "redrawing the same screen diffs as usual");
    ui.action(render::Action::Settings);
    ui.compute_view(100, 30);
    assert!(std::mem::take(&mut ui.full_repaint), "opening a form");
    ui.compute_view(100, 30);
    assert!(!ui.full_repaint, "once per switch");
    ui.action(render::Action::Cancel);
    ui.compute_view(100, 30);
    assert!(std::mem::take(&mut ui.full_repaint), "closing a form");
    ui.open_room(other);
    ui.compute_view(100, 30);
    assert!(std::mem::take(&mut ui.full_repaint), "switching rooms");
    ui.open_room(room);
    ui.terminal = Some(agent);
    ui.compute_view(100, 30);
    assert!(
        std::mem::take(&mut ui.full_repaint),
        "opening an agent terminal"
    );
}

#[test]
fn layout_takes_the_sidebar_width_from_settings_and_clamps_to_a_third() {
    use ratatui::layout::Rect;
    assert_eq!(layout(100, 30, 28).sidebar, Rect::new(0, 0, 28, 30));
    assert_eq!(layout(100, 30, 28).pane_surface, Rect::new(28, 0, 72, 30));
    // A wide setting on a narrow terminal still leaves the chat two thirds.
    assert_eq!(layout(90, 30, 40).sidebar.width, 30);
    assert_eq!(layout(200, 30, 60).sidebar.width, 60);
    assert_eq!(layout(200, 30, 60).pane_surface, Rect::new(60, 0, 140, 30));
    assert_eq!(layout(2, 30, 60).sidebar.width, 1);
}

#[test]
fn sidebar_width_setting_applies_at_once_to_hits_scroll_and_pane_surface() {
    let (mut ui, _, _) = fixture();
    ui.compute_view(120, 40);
    assert_eq!(ui.view.sidebar.width, 28);
    let plus = |ui: &BusUi| {
        ui.view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::NewAgent)
            .map(|hit| hit.rect.x)
    };
    assert_eq!(plus(&ui), Some(25));
    ui.receive_event(BusEvent::SettingsChanged(
        crate::messaging::prefs::settings::BusSettings {
            sidebar_width: 40,
            ..Default::default()
        },
    ));
    ui.compute_view(120, 40);
    assert_eq!(ui.view.sidebar.width, 40);
    assert_eq!(ui.view.sidebar_body.width, 40);
    assert_eq!(plus(&ui), Some(37));
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 120, 40));
    ui.render(&mut buffer);
    for y in 0..40 {
        assert_eq!(buffer[(39, y)].symbol(), "│", "divider row {y}");
    }
    // Mouse hit-testing follows: a click right of the old edge still lands
    // in the sidebar.
    assert!(ui.view.sidebar.right() == 40);
    // A narrow terminal clamps the saved width at render time.
    ui.compute_view(90, 40);
    assert_eq!(ui.view.sidebar.width, 30);
    // The agent pane surface starts where the wider sidebar ends.
    let shell = shell_with(ui);
    assert_eq!(shell.layout(120, 30).pane_surface.x, 40);
    assert_eq!(shell.surface_size(120, 30).cols, 80);
}

#[test]
fn work_room_history_matches_master_layout() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    for (cols, rows) in [(120, 40), (80, 24), (60, 12)] {
        ui.open_room(master);
        ui.compute_view(cols, rows);
        let expected = (ui.view.history, ui.view.history_text, composer_rect(&ui));
        assert_eq!(expected.0.y, 4);
        ui.open_room(work);
        ui.compute_view(cols, rows);
        assert_eq!(
            (ui.view.history, ui.view.history_text, composer_rect(&ui)),
            expected,
            "work history gets every row available to MASTER at {cols}x{rows}"
        );
        assert_eq!(ui.view.history.bottom(), composer_rect(&ui).y - 3);
    }
}

#[test]
fn work_room_ctrl_f_uses_master_top_area() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    for (cols, rows) in [(120, 40), (80, 24)] {
        ui.open_room(master);
        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        ui.compute_view(cols, rows);
        let expected = (
            ui.view.search_box,
            ui.view.search_field,
            ui.view.history,
            composer_rect(&ui),
        );
        assert_eq!(expected.0.y, 4);
        assert_eq!(expected.0.height, 6);
        assert_eq!(expected.2.y, expected.0.bottom());
        ui.open_room(work);
        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        ui.compute_view(cols, rows);
        assert_eq!(
            (
                ui.view.search_box,
                ui.view.search_field,
                ui.view.history,
                composer_rect(&ui)
            ),
            expected,
            "work search uses MASTER's full top area at {cols}x{rows}"
        );
        assert!(ui.view.history.height > 0);
    }
}
