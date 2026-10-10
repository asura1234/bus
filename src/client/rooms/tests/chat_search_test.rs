use super::*;

/// `count` completed exchanges in `room`: prompt `prompt-NN` (with "Needle"
/// in the ones listed) and reply `answer-NN`.
fn history_with(ui: &mut BusUi, room: RoomId, agent: AgentId, count: usize, needles: &[usize]) {
    let mut snapshot = (*ui.snapshot).clone();
    let mut requests = Vec::new();
    for i in 0..count {
        snapshot.state.set_draft_recipients(room, [agent]).unwrap();
        let text = if needles.contains(&i) {
            format!("prompt-{i:02} with a Needle inside")
        } else {
            format!("prompt-{i:02}")
        };
        snapshot.state.set_draft_text(room, &text).unwrap();
        requests.push(
            snapshot
                .state
                .submit_draft(room, 1000 + i as u64 * 1000)
                .unwrap()[0],
        );
    }
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (i, id) in requests.iter().enumerate() {
        let record = &mut json["requests"][id.0.to_string()];
        record["phase"] = "completed".into();
        record["completed_at_ms"] = (1500 + i as u64 * 1000).into();
        record["pending_final"] = serde_json::json!({
            "callback_id": format!("final-{i}"), "text": format!("answer-{i:02}"),
            "received_at_ms": 1500 + i as u64 * 1000,
            "provider_session_id": "session", "provider_turn_id": format!("turn-{i}")
        });
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

fn type_text(ui: &mut BusUi, text: &str) {
    for c in text.chars() {
        key(ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

/// The history row of the selected match, and the rows on screen.
fn selected_row(ui: &BusUi) -> usize {
    let current = ui.chat_search.as_ref().unwrap().current;
    ui.view.search_matches[current].0
}

/// The search panel's query row, rendered at 100x30: the label, the query
/// in its field, and the counter, joined by single spaces.
fn panel_row(ui: &mut BusUi) -> String {
    let screen: Vec<char> = room_screen(ui, 100, 30).chars().collect();
    let y = usize::from(ui.view.search_field.y) + 1;
    screen[y * 100..(y + 1) * 100]
        .iter()
        .collect::<String>()
        .split('│')
        .skip(2)
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn on_screen(ui: &BusUi, row: usize) -> bool {
    (ui.main_scroll..ui.main_scroll + usize::from(ui.view.history.height)).contains(&row)
}

#[test]
fn ctrl_f_searches_the_whole_history_and_wraps_both_ways() {
    let (mut ui, room, agent) = fixture();
    history_with(&mut ui, room, agent, 40, &[2, 25, 38]);
    ui.open_room(room);
    room_screen(&mut ui, 100, 30);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert!(ui.form.is_none(), "Ctrl+F no longer opens the file picker");
    // Case-insensitive, over rows far outside the viewport.
    type_text(&mut ui, "nEEDLE");
    let screen = room_screen(&mut ui, 100, 30);
    assert_eq!(ui.view.search_matches.len(), 3);
    let rows: Vec<usize> = ui.view.search_matches.iter().map(|(row, _)| *row).collect();
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?}");
    // A new query starts at the newest match, in view.
    assert!(panel_row(&mut ui).starts_with("Find nEEDLE"), "{screen}");
    assert!(
        panel_row(&mut ui).ends_with("3/3"),
        "{}",
        panel_row(&mut ui)
    );
    assert_eq!(selected_row(&ui), rows[2]);
    assert!(on_screen(&ui, rows[2]));
    assert!(
        !on_screen(&ui, rows[0]),
        "the oldest match starts off screen"
    );

    // Enter goes to the next older match, up the history.
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(panel_row(&mut ui).ends_with("2/3"));
    assert!(on_screen(&ui, rows[1]));
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert!(panel_row(&mut ui).ends_with("1/3"));
    assert!(on_screen(&ui, rows[0]), "scrolled up to the oldest match");
    // Past the oldest it wraps to the newest.
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(panel_row(&mut ui).ends_with("3/3"));
    assert!(on_screen(&ui, rows[2]));
    // Shift+Enter and Down go newer, wrapping from the newest to the oldest.
    key(&mut ui, KeyCode::Enter, KeyModifiers::SHIFT);
    assert!(panel_row(&mut ui).ends_with("1/3"));
    assert!(on_screen(&ui, rows[0]));
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert!(panel_row(&mut ui).ends_with("2/3"));
    // Typing went to the search, never into the draft.
    assert_eq!(ui.locals[&room].text.text, "");
}

#[test]
fn the_selected_match_is_highlighted_brighter_than_the_others() {
    let (mut ui, room, agent) = fixture();
    history_with(&mut ui, room, agent, 3, &[0, 1]);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    type_text(&mut ui, "needle");
    ui.compute_view(100, 60);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 60));
    ui.render(&mut buffer);
    let backgrounds: Vec<_> = ui
        .view
        .search_matches
        .clone()
        .into_iter()
        .map(|(row, range)| {
            let line = &ui.history.cached()[row];
            let column = unicode_width::UnicodeWidthStr::width(&line.text[..range.start]) as u16;
            let y = ui.view.history_text.y + (row - ui.main_scroll) as u16;
            buffer[(ui.view.history_text.x + column, y)].bg
        })
        .collect();
    assert_eq!(
        backgrounds,
        [
            ratatui::style::Color::Rgb(150, 125, 60),
            ratatui::style::Color::Rgb(240, 190, 70)
        ]
    );
}

#[test]
fn esc_closes_the_search_and_returns_focus_where_it_was() {
    for notes in [false, true] {
        let (mut ui, room, agent) = fixture();
        history_with(&mut ui, room, agent, 2, &[1]);
        ui.open_room(room);
        if notes {
            key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
        }
        assert_eq!(ui.notes_focus, notes);
        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        assert!(!ui.notes_focus, "the search bar takes focus");
        type_text(&mut ui, "needle");
        room_screen(&mut ui, 100, 30);
        assert_eq!(
            ui.view.cursor.as_ref().map(|cursor| cursor.y),
            Some(ui.view.search_field.y + 1),
            "the cursor is in the search panel"
        );
        key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
        assert!(ui.chat_search.is_none());
        assert_eq!(ui.notes_focus, notes, "focus is back where it was");
        let screen = room_screen(&mut ui, 100, 30);
        assert!(!screen.contains("Find "), "{screen}");
        // Keys reach the composer or notes again.
        type_text(&mut ui, "x");
        let local = &ui.locals[&room];
        assert_eq!(
            if notes {
                &local.notes.text
            } else {
                &local.text.text
            },
            "x"
        );
    }
}

#[test]
fn author_names_match_and_master_is_searchable() {
    let (mut ui, room, agent) = fixture();
    history_with(&mut ui, room, agent, 2, &[]);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    type_text(&mut ui, "author");
    room_screen(&mut ui, 100, 30);
    assert_eq!(
        ui.view.search_matches.len(),
        4,
        "each exchange names the agent as recipient and on its reply"
    );

    let mut snapshot = (*ui.snapshot).clone();
    let master = snapshot.state.ensure_master_room();
    let orchestrator = snapshot
        .state
        .create_agent(master, "orch", Provider::ClaudeCode, "/p".into(), None)
        .unwrap();
    snapshot
        .state
        .post_to_human(
            master,
            orchestrator,
            "Gate passed on PR 5.".into(),
            Vec::new(),
            9,
        )
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    ui.open_room(master);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    type_text(&mut ui, "gate passed");
    let row = panel_row(&mut ui);
    assert!(
        row.starts_with("Find gate passed") && row.ends_with("1/1"),
        "{row}"
    );
}

#[test]
fn ctrl_o_opens_the_file_picker_and_empty_or_missing_queries_say_so() {
    let (mut ui, room, agent) = fixture();
    history_with(&mut ui, room, agent, 1, &[]);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert!(matches!(ui.form, Some(forms::Form::Files(_))));
    ui.form = None;
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert_eq!(panel_row(&mut ui), "Find");
    type_text(&mut ui, "zzz");
    let row = panel_row(&mut ui);
    assert!(
        row.starts_with("Find zzz") && row.ends_with("no matches"),
        "{row}"
    );
    // Next and previous do nothing without matches.
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(ui.chat_search.as_ref().unwrap().query, "zz");
}

#[test]
fn the_panel_sits_directly_above_the_history_and_gives_its_rows_back() {
    for master in [false, true] {
        let (mut ui, room, agent) = fixture();
        history_with(&mut ui, room, agent, 3, &[1]);
        let room = if master {
            let mut snapshot = (*ui.snapshot).clone();
            let master = snapshot.state.ensure_master_room();
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
            master
        } else {
            room
        };
        ui.open_room(room);
        room_screen(&mut ui, 100, 30);
        let (top, height) = (ui.view.history_text.y, ui.view.history.height);
        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        let screen: Vec<char> = room_screen(&mut ui, 100, 30).chars().collect();
        let panel = ui.view.search_box;
        // Where the history began, across the chat pane like the notes box.
        assert_eq!(panel.y, top, "master: {master}");
        assert_eq!(
            (panel.x, panel.width),
            (ui.view.history.x + 1, ui.view.history.width - 2)
        );
        assert_eq!(ui.view.history_text.y, panel.bottom());
        assert_eq!(ui.view.history.height, height - panel.height);
        let hint: String = screen[usize::from(panel.y + 4) * 100..usize::from(panel.y + 5) * 100]
            .iter()
            .collect();
        assert!(
            hint.contains("Enter/↑ older · Shift+Enter/↓ newer · Esc close"),
            "{hint}"
        );
        key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
        room_screen(&mut ui, 100, 30);
        assert_eq!(ui.view.search_box.height, 0);
        assert_eq!(
            (ui.view.history_text.y, ui.view.history.height),
            (top, height)
        );
    }
}

#[test]
fn the_query_has_its_own_field_after_a_dim_label() {
    let (mut ui, room, agent) = fixture();
    history_with(&mut ui, room, agent, 2, &[1]);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    type_text(&mut ui, "needle");
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let (panel, field) = (ui.view.search_box, ui.view.search_field);
    // The field is a bordered box inside the panel, right of the label.
    assert_eq!((field.y, field.height), (panel.y + 1, 3));
    assert!(field.x > panel.x + 1 && field.right() < panel.right() - 1);
    assert_eq!(buffer[(field.x, field.y)].symbol(), "┌");
    let label: String = (panel.x + 1..field.x)
        .map(|x| buffer[(x, field.y + 1)].symbol())
        .collect();
    assert_eq!(label.trim(), "Find");
    let hint_grey = buffer[(panel.x + 1, panel.y + 4)].fg;
    assert_eq!(buffer[(panel.x + 1, field.y + 1)].fg, hint_grey);
    let query = buffer[(field.x + 1, field.y + 1)].clone();
    assert_eq!(query.symbol(), "n");
    assert_ne!(query.fg, hint_grey, "only the query is bright");
}

#[test]
fn cancelling_recall_after_switching_rooms_preserves_the_destination_draft() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot.state.create_room("second").unwrap();
    snapshot
        .state
        .set_draft_text(second, "second unsent draft")
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(first);
    let local = ui.locals.get_mut(&first).unwrap();
    local.text.insert("first unsent draft");
    local.recall.push("first older prompt".into());
    key(&mut ui, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&first].text.text, "first older prompt");

    ui.open_room(second);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);

    assert_eq!(ui.locals[&second].text.text, "second unsent draft");
    assert!(!ui.pending.iter().any(|pending| matches!(
        &pending.command,
        BusCommand::SetDraftText(room, text) if *room == second && text == "first unsent draft"
    )));
}
