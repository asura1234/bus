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

#[test]
fn room_notes_have_a_box_and_top_section_is_separated_from_history() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, "history body").unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.submit_draft(room, 10).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().notes.insert("Room notes");
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let color = buffer[(27, 0)].fg;
    // One line of notes gets a one-row box; history starts right below it.
    for (x, y, symbol) in [(29, 2, "┌"), (98, 2, "┐"), (29, 4, "└"), (98, 4, "┘")] {
        assert_eq!(buffer[(x, y)].symbol(), symbol);
        assert_eq!(buffer[(x, y)].fg, color);
    }
    for x in 30..98 {
        for y in [2, 4, 5] {
            assert_eq!(buffer[(x, y)].symbol(), "─");
            assert_eq!(buffer[(x, y)].fg, color);
        }
    }
    assert_eq!(buffer[(30, 3)].symbol(), "R");
    assert_eq!(buffer[(30, 6)].symbol(), "Y");
    assert_eq!(buffer[(30, 7)].symbol(), "h");
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('!'), KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].notes.text, "Room notes!");
    assert!(ui.send_intent.is_none());
}

#[test]
fn composer_sits_on_last_row_unless_a_visible_notice_needs_it() {
    let (mut ui, room, _) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        ui.toast = None;
        for notice in [false, true] {
            if notice {
                ui.show_toast("Save failed");
            }
            ui.compute_view(cols, rows);
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
            ui.render(&mut buffer);
            let editor = composer_rect(&ui);
            let border_y = rows - 1 - u16::from(notice);
            assert_eq!(editor.bottom() + 1, border_y, "no empty footer row");
            assert_eq!(buffer[(editor.x - 1, border_y)].symbol(), "└");
            assert_eq!(buffer[(editor.right(), border_y)].symbol(), "┘");
            for x in editor.x..editor.right() {
                assert_eq!(buffer[(x, border_y)].symbol(), "─");
            }
            if notice {
                let footer: String = (editor.x..editor.right())
                    .map(|x| buffer[(x, rows - 1)].symbol())
                    .collect();
                assert!(footer.starts_with("Save failed"));
            }
        }
        ui.toast.as_mut().unwrap().started_at -= std::time::Duration::from_secs(15);
        ui.compute_view(cols, rows);
        assert_eq!(composer_rect(&ui).bottom() + 1, rows - 1);
    }
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
    mouse(&mut ui, ScrollDown, 40, 3); // Notes are not the history viewport.
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
fn composer_yields_space_to_visible_notes_in_short_terminal() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"long draft\n".repeat(80));
    room_screen(&mut ui, 60, 12);
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("visible notes".into()),
        false,
        &mut Default::default(),
    );
    assert!(room_screen(&mut ui, 60, 12).contains("visible notes"));
    let cursor = ui.cursor().expect("visible notes caret");
    let notes = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Notes)
        .unwrap()
        .rect;
    assert!(notes.contains((cursor.x, cursor.y).into()));
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
fn room_notes_grow_to_a_quarter_of_the_window_and_history_takes_the_rest() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new(checklist(30));
    let rows = screen_rows(&mut ui, 100, 30);
    // 30 rows / 4 = 7 note rows inside the box's borders.
    assert_eq!(ui.view.notes.height, 7);
    assert_eq!(ui.view.notes_box, ratatui::layout::Rect::new(29, 2, 70, 9));
    assert_eq!(ui.view.history_text.y, 12, "history starts under the box");
    assert!(rows[1].contains("# "), "the room name keeps its row");
    assert!(rows[3].contains("item 01") && rows[9].contains("item 07"));
    assert!(rows[10].contains("↓ more"), "{:?}", rows[10]);
    assert!(!rows[2].contains("↑ more"));

    for (cols, height, notes) in [(100, 40, 10), (100, 24, 6), (60, 24, 6)] {
        screen_rows(&mut ui, cols, height);
        assert_eq!(ui.view.notes.height, notes, "{cols}x{height}");
        assert!(ui.view.notes_box.bottom() < ui.view.history_text.y);
        assert!(
            ui.view.history_text.height >= 1,
            "{cols}x{height}: history keeps a row"
        );
    }

    // Short notes shrink the box back; empty notes keep the F3 prompt row.
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new("two\nlines".into());
    screen_rows(&mut ui, 100, 30);
    assert_eq!(ui.view.notes.height, 2);
    assert_eq!(ui.view.history_text.y, 7);
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new(String::new());
    let rows = screen_rows(&mut ui, 100, 30);
    assert_eq!(ui.view.notes.height, 1);
    assert!(rows[3].contains("Add notes… (F3)"));
}

#[test]
fn room_notes_scroll_with_the_wheel_and_page_keys_and_keep_the_caret_visible() {
    use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new(checklist(30));
    screen_rows(&mut ui, 100, 30);
    let notes = ui.view.notes;

    // The wheel over the box scrolls only the notes.
    mouse(&mut ui, ScrollDown, notes.x + 2, notes.y + 1);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[3].contains("item 02"), "{:?}", rows[3]);
    assert!(rows[2].contains("↑ more") && rows[10].contains("↓ more"));
    assert_eq!(ui.main_scroll, ui.view.history_max_scroll);
    for _ in 0..40 {
        mouse(&mut ui, ScrollDown, notes.x + 2, notes.y + 1);
    }
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[9].contains("item 30") && !rows[10].contains("↓ more"));
    mouse(&mut ui, ScrollUp, notes.x + 2, notes.y + 1);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[9].contains("item 29"));

    // While editing, the caret starts in view, PgDn/PgUp page, and typing or
    // arrows bring the caret back into view.
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(
        rows[9].contains("item 30"),
        "F3 brings the end caret into view"
    );
    ui.locals.get_mut(&room).unwrap().notes.cursor = 0;
    screen_rows(&mut ui, 100, 30);
    let visible = |ui: &BusUi| {
        ui.cursor()
            .is_some_and(|cursor| ui.view.notes.contains((cursor.x, cursor.y).into()))
    };
    assert!(visible(&ui));
    key(&mut ui, KeyCode::PageDown, KeyModifiers::NONE);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(
        rows[3].contains("item 07"),
        "a page keeps one row of context"
    );
    assert!(!visible(&ui), "paging scrolls without moving the caret");
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[3].contains("item 01"));
    for _ in 0..20 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        screen_rows(&mut ui, 100, 30);
        assert!(visible(&ui), "arrow keys keep the caret visible");
    }
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    screen_rows(&mut ui, 100, 30);
    assert!(!visible(&ui));
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    screen_rows(&mut ui, 100, 30);
    assert!(visible(&ui), "typing scrolls back to the caret");
    assert!(ui.locals[&room].notes.text.contains("x- [ ] item 21"));
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
fn master_room_shows_no_notes_and_f3_never_focuses_them() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    ui.open_room(master);
    assert!(!room_screen(&mut ui, 100, 30).contains("Add notes"));
    assert_eq!(ui.view.notes_box.height, 0);
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(!ui.notes_focus, "MASTER has no notes to edit");

    ui.open_room(work);
    assert!(room_screen(&mut ui, 100, 30).contains("Add notes"));
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(ui.notes_focus, "work rooms keep their notes");
}
