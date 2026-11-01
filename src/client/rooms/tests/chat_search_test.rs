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
        record["completed_reply"] = serde_json::json!({"group":record["group"].as_u64().unwrap_or(record["id"].as_u64().unwrap()),"content":serde_json::json!({
            "callback_id": format!("final-{i}"), "text": format!("answer-{i:02}"),
            "received_at_ms": 1500 + i as u64 * 1000,
            "provider_session_id": "session", "provider_turn_id": format!("turn-{i}")
        })});
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
    assert!(panel_row(&mut ui).starts_with("Find: nEEDLE"), "{screen}");
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
        assert!(!screen.contains("Find: "), "{screen}");
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
        .post_to_developer(
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
        row.starts_with("Find: gate passed") && row.ends_with("1/1"),
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
    assert_eq!(panel_row(&mut ui), "Find:");
    type_text(&mut ui, "zzz");
    let row = panel_row(&mut ui);
    assert!(
        row.starts_with("Find: zzz") && row.ends_with("no matches"),
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
        let hint: String = screen[usize::from(panel.y + 6) * 100..usize::from(panel.y + 7) * 100]
            .iter()
            .collect();
        assert!(
            hint.contains("Tab")
                && hint.contains("Enter/Space/click toggle")
                && hint.contains("↑ older")
                && hint.contains("↓ newer")
                && hint.contains("Esc"),
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
    assert_eq!(label.trim(), "Find:");
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
    local
        .recall
        .push(Recalled::now("first older prompt".into()));
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

/// Deliberately identical text in mixed routes: rendered header text must not
/// determine whether a message is addressed to the developer.
fn mixed_find_history(ui: &mut BusUi, room: RoomId, agent: AgentId) {
    use crate::messaging::model::{Author, Draft};
    let mut snapshot = (*ui.snapshot).clone();
    let peer = snapshot
        .state
        .create_agent(
            room,
            "peer → You",
            Provider::ClaudeCode,
            "/peer".into(),
            None,
        )
        .unwrap();
    let mut replies = Vec::new();
    for (i, (author, text, reply)) in [
        (
            Author::Agent(agent),
            "Needle ordinary peer traffic",
            "ordinary peer answer",
        ),
        (
            Author::Agent(agent),
            "Needle @you choose a branch",
            "ordinary peer answer",
        ),
        (
            Author::Agent(agent),
            "[waiting-on-you] Needle approve access",
            "ordinary peer answer",
        ),
        (
            Author::Developer,
            "Needle @you outgoing prompt",
            "Needle developer answer",
        ),
        (
            Author::Agent(agent),
            "Needle peer question",
            "Needle @you reply mention",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let requests = snapshot
            .state
            .submit_message_from(
                room,
                Draft {
                    text: text.into(),
                    files: Vec::new(),
                    recipient_ids: [peer].into(),
                },
                author,
                1000 + i as u64 * 1000,
            )
            .unwrap();
        replies.push((requests[0], reply, 1500 + i as u64 * 1000));
    }
    snapshot
        .state
        .post_to_developer(
            room,
            agent,
            "Needle direct ask\nsecond line".into(),
            Vec::new(),
            6000,
        )
        .unwrap();
    snapshot
        .state
        .post_bus_notice(room, None, "Needle Bus notice".into(), 7000)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    complete_requests(ui, replies);
}

#[test]
fn you_only_selects_model_addressed_messages_and_intersects_text_matches() {
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    room_screen(&mut ui, 100, 60);
    assert!(
        ui.view.search_matches.is_empty(),
        "ordinary empty find is unchanged"
    );
    type_text(&mut ui, "needle");
    room_screen(&mut ui, 100, 60);
    assert_eq!(
        ui.view.search_matches.len(),
        9,
        "unfiltered matches include all routes"
    );
    let history = ui.history.cached().to_vec();
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    let screen = room_screen(&mut ui, 100, 60);
    assert!(screen.contains("[x] * → You"), "{screen}");
    assert_eq!(ui.view.search_matches.len(), 6);
    let texts: Vec<_> = ui
        .view
        .search_matches
        .iter()
        .map(|(row, _)| ui.history.cached()[*row].text.trim())
        .collect();
    assert_eq!(
        texts,
        [
            "Needle @you choose a branch",
            "[waiting-on-you] Needle approve access",
            "Needle developer answer",
            "Needle @you reply mention",
            "Needle direct ask",
            "Needle Bus notice"
        ]
    );
    assert_eq!(
        ui.history.cached(),
        history,
        "filtering leaves every row intact"
    );
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    room_screen(&mut ui, 100, 60);
    assert_eq!(ui.view.search_matches.len(), 9);
}

#[test]
fn empty_you_only_steps_once_per_message_newest_first_and_resets_on_close() {
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    room_screen(&mut ui, 100, 30);
    assert_eq!(
        ui.view.search_matches.len(),
        6,
        "one match per addressed message"
    );
    let rows: Vec<_> = ui.view.search_matches.iter().map(|(row, _)| *row).collect();
    assert_eq!(selected_row(&ui), rows[5]);
    for index in (0..5).rev() {
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
        room_screen(&mut ui, 100, 30);
        assert_eq!(selected_row(&ui), rows[index]);
        assert!(on_screen(&ui, rows[index]));
    }
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    room_screen(&mut ui, 100, 30);
    assert_eq!(selected_row(&ui), rows[5]);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    room_screen(&mut ui, 100, 30);
    assert_eq!(selected_row(&ui), rows[0]);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.chat_search.is_none());
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    assert!(!ui.chat_search.as_ref().unwrap().you_only);
    assert_eq!(panel_row(&mut ui), "Find:");
    assert!(ui.view.search_matches.is_empty());
}

#[test]
fn empty_you_only_highlights_the_selected_header_and_handles_no_matches() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(room, agent, "Please decide".into(), Vec::new(), 1)
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let row = selected_row(&ui);
    let y = ui.view.history_text.y + (row - ui.main_scroll) as u16;
    assert_eq!(
        buffer[(ui.view.history_text.x, y)].bg,
        ratatui::style::Color::Rgb(240, 190, 70)
    );
    key(&mut ui, KeyCode::BackTab, KeyModifiers::SHIFT);
    type_text(&mut ui, "absent");
    assert!(panel_row(&mut ui).ends_with("no matches"));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert!(ui.view.search_matches.is_empty());
    assert!(help::TEXT.contains("Tab/Shift+Tab focus text, * → You, You → *"));
}

#[test]
fn find_focus_cycles_both_ways_and_space_edits_only_the_focused_control() {
    use super::super::chat_search::SearchFocus::{FromYou, Query, ToYou};
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    for expected in [ToYou, FromYou, Query] {
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        let search = ui.chat_search.as_ref().unwrap();
        assert_eq!(search.focus, expected);
        assert!(!search.you_only && !search.from_you, "focus never toggles");
    }
    type_text(&mut ui, "needle ");
    assert_eq!(ui.chat_search.as_ref().unwrap().query, "needle ");
    for (code, modifiers, expected) in [
        (KeyCode::BackTab, KeyModifiers::SHIFT, FromYou),
        (KeyCode::Tab, KeyModifiers::SHIFT, ToYou),
        (KeyCode::BackTab, KeyModifiers::NONE, Query),
    ] {
        key(&mut ui, code, modifiers);
        assert_eq!(ui.chat_search.as_ref().unwrap().focus, expected);
    }
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    type_text(&mut ui, "ignored");
    key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(ui.chat_search.as_ref().unwrap().query, "needle ");
    assert!(ui.chat_search.as_ref().unwrap().you_only);
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(ui.chat_search.as_ref().unwrap().from_you);
    assert_eq!(ui.locals[&room].text.text, "");
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    let search = ui.chat_search.as_ref().unwrap();
    assert_eq!(search.focus, Query);
    assert!(search.query.is_empty() && !search.you_only && !search.from_you);
}

#[test]
fn outgoing_and_union_filters_use_authorship_in_work_rooms_and_master() {
    for master in [false, true] {
        let (mut ui, room, agent) = fixture();
        let (room, agent) = if master {
            let mut snapshot = (*ui.snapshot).clone();
            let master = snapshot.state.ensure_master_room();
            let orch = snapshot
                .state
                .create_agent(master, "orch", Provider::ClaudeCode, "/p".into(), None)
                .unwrap();
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
            (master, orch)
        } else {
            (room, agent)
        };
        mixed_find_history(&mut ui, room, agent);
        ui.open_room(room);
        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        key(&mut ui, KeyCode::BackTab, KeyModifiers::SHIFT);
        key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
        room_screen(&mut ui, 100, 60);
        assert_eq!(ui.view.search_matches.len(), 1, "one developer prompt");
        assert!(ui.history.cached()[selected_row(&ui)]
            .text
            .contains("You →"));
        key(&mut ui, KeyCode::BackTab, KeyModifiers::SHIFT);
        key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
        room_screen(&mut ui, 100, 60);
        assert_eq!(ui.view.search_matches.len(), 7, "union, not intersection");
        key(&mut ui, KeyCode::BackTab, KeyModifiers::SHIFT);
        type_text(&mut ui, "needle");
        room_screen(&mut ui, 100, 60);
        assert_eq!(ui.view.search_matches.len(), 7);
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
        room_screen(&mut ui, 100, 60);
        let texts: Vec<_> = ui
            .view
            .search_matches
            .iter()
            .map(|(row, _)| ui.history.cached()[*row].text.trim())
            .collect();
        assert_eq!(texts, ["Needle @you outgoing prompt"], "master: {master}");
    }
}

#[test]
fn arrow_navigation_works_from_every_control_with_empty_or_typed_text() {
    for query in ["", "needle"] {
        let (mut ui, room, agent) = fixture();
        history_with(&mut ui, room, agent, 5, &[0, 2, 4]);
        ui.open_room(room);
        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        type_text(&mut ui, query);
        key(&mut ui, KeyCode::BackTab, KeyModifiers::SHIFT);
        key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
        room_screen(&mut ui, 100, 60);
        let count = if query.is_empty() { 5 } else { 3 };
        assert_eq!(ui.view.search_matches.len(), count);
        for _ in 0..3 {
            for (code, modifiers, expected) in [
                (KeyCode::Up, KeyModifiers::NONE, count - 2),
                (KeyCode::Up, KeyModifiers::NONE, count - 3),
                (KeyCode::Down, KeyModifiers::NONE, count - 2),
                (KeyCode::Down, KeyModifiers::NONE, count - 1),
            ] {
                key(&mut ui, code, modifiers);
                room_screen(&mut ui, 100, 60);
                assert_eq!(ui.chat_search.as_ref().unwrap().current, expected);
                assert!(on_screen(&ui, selected_row(&ui)));
            }
            key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        }
    }
}

#[test]
fn focused_find_control_is_highlighted_and_cursor_follows_text_focus() {
    use super::super::chat_search::SearchFocus::{FromYou, Query, ToYou};
    let (mut ui, room, _) = fixture();
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    for focus in [Query, ToYou, FromYou, Query] {
        ui.compute_view(100, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let field = ui.view.search_field;
        for (expected, x, y) in [
            (Query, field.x + 1, field.y + 1),
            (ToYou, ui.view.search_box.x + 1, field.bottom()),
            (FromYou, ui.view.search_box.x + 1, field.bottom() + 1),
        ] {
            assert_eq!(
                buffer[(x, y)].bg,
                if focus == expected {
                    ratatui::style::Color::Rgb(46, 48, 58)
                } else {
                    ratatui::style::Color::Rgb(24, 24, 28)
                }
            );
        }
        assert_eq!(ui.view.cursor.is_some(), focus == Query);
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    }
}

#[test]
fn narrow_find_bar_keeps_the_typed_query_and_both_toggles_visible() {
    for cols in [50u16, 60, 66, 72, 100] {
        for query in ["needle", "absent"] {
            let (mut ui, room, agent) = fixture();
            history_with(&mut ui, room, agent, 3, &[1]);
            ui.open_room(room);
            key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
            type_text(&mut ui, query);
            ui.compute_view(cols, 30);
            let area = ratatui::layout::Rect::new(0, 0, cols, 30);
            let mut buffer = ratatui::buffer::Buffer::empty(area);
            ui.render(&mut buffer);
            let panel = ui.view.search_box;
            let field = ui.view.search_field;
            let text = |y: u16, xs: std::ops::Range<u16>| -> String {
                xs.map(|x| buffer[(x, y)].symbol().to_owned()).collect()
            };
            let field_row = text(field.y + 1, field.x..field.right());
            assert_eq!(
                text(field.bottom(), panel.x + 1..panel.x + 12),
                "[ ] * → You"
            );
            assert_eq!(
                text(field.bottom() + 1, panel.x + 1..panel.x + 12),
                "[ ] You → *"
            );
            let inside: Vec<String> = (panel.y..panel.bottom())
                .map(|y| text(y, panel.x..panel.right().min(cols)))
                .collect();
            let outside: Vec<String> = (panel.y..panel.bottom())
                .map(|y| text(y, panel.right().min(cols)..cols))
                .collect();
            let all = inside.join("\n");
            assert!(
                field_row.contains(query)
                    && all.contains("[ ] * → You")
                    && all.contains("[ ] You → *")
                    && all.contains(if query == "needle" {
                        "1/1"
                    } else {
                        "no matches"
                    })
                    && !all.contains("1/1You")
                    && outside.iter().all(|row| !row.contains("You")),
                "cols {cols}, panel {panel:?}, field {field:?}:\n{all}"
            );
        }
    }
}

#[test]
fn enter_on_each_find_toggle_refilters_and_selects_the_newest_match() {
    use super::super::chat_search::SearchFocus::{FromYou, ToYou};
    for query in ["", "needle"] {
        let (mut ui, room, agent) = fixture();
        mixed_find_history(&mut ui, room, agent);
        ui.open_room(room);
        ui.open_chat_search();
        type_text(&mut ui, query);
        room_screen(&mut ui, 100, 60);
        for (focus, count) in [(ToYou, 6), (FromYou, 7)] {
            key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
            key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
            room_screen(&mut ui, 100, 60);
            let search = ui.chat_search.as_ref().unwrap();
            assert_eq!(search.focus, focus);
            assert!(search.you_only, "Enter must activate the incoming toggle");
            assert_eq!(search.from_you, focus == FromYou);
            assert_eq!(ui.view.search_matches.len(), count);
            assert_eq!(search.current, count - 1);
            assert_selected_find_highlight(&mut ui, 100);
            key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
            room_screen(&mut ui, 100, 60);
            assert_eq!(ui.chat_search.as_ref().unwrap().current, count - 2);
        }
        key(&mut ui, KeyCode::Enter, KeyModifiers::SHIFT);
        room_screen(&mut ui, 100, 60);
        assert!(!ui.chat_search.as_ref().unwrap().from_you);
        assert_eq!(ui.view.search_matches.len(), 6);
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        for (code, modifiers, expected) in [
            (KeyCode::Enter, KeyModifiers::NONE, 4),
            (KeyCode::Enter, KeyModifiers::SHIFT, 5),
            (KeyCode::Down, KeyModifiers::NONE, 0),
        ] {
            key(&mut ui, code, modifiers);
            room_screen(&mut ui, 100, 60);
            assert_eq!(ui.chat_search.as_ref().unwrap().current, expected);
            assert_selected_find_highlight(&mut ui, 100);
        }
    }
}

/// Locate the actual rendered text, so the click test catches layout/hit drift.
fn find_panel_text(ui: &mut BusUi, cols: u16, text: &str) -> (u16, u16) {
    let screen = room_screen(ui, cols, 60);
    let chars: Vec<_> = screen.chars().collect();
    let panel = ui.view.search_box;
    for y in panel.y..panel.bottom() {
        let row: String = chars[usize::from(y * cols)..usize::from((y + 1) * cols)]
            .iter()
            .collect();
        if let Some(byte) = row.find(text) {
            return (row[..byte].chars().count() as u16, y);
        }
    }
    panic!("missing {text:?} at {cols} columns:\n{screen}");
}

fn assert_find_toggle_clicks(widths: [u16; 2]) {
    use super::super::chat_search::SearchFocus::{FromYou, Query, ToYou};
    use crossterm::event::{
        MouseButton::Left,
        MouseEventKind::{Down, Up},
    };
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    ui.open_chat_search();
    type_text(&mut ui, "needle");
    // Resize the same open panel, then click every painted box/label cell.
    for cols in widths {
        for (label, focus, count) in [("* → You", ToYou, 6), ("You → *", FromYou, 1)] {
            let (label_x, y) = find_panel_text(&mut ui, cols, label);
            for x in label_x - 4..label_x + 7 {
                mouse(&mut ui, Down(Left), x, y);
                mouse(&mut ui, Up(Left), x, y);
                room_screen(&mut ui, cols, 60);
                let search = ui.chat_search.as_ref().unwrap();
                assert_eq!(search.focus, focus, "cols {cols}, cell {x},{y}");
                assert_eq!(search.you_only, focus == ToYou);
                assert_eq!(search.from_you, focus == FromYou);
                assert_eq!(ui.view.search_matches.len(), count);
                assert_eq!(search.current, count - 1);
                assert_selected_find_highlight(&mut ui, cols);
                // A second click toggles off, including inside the double-click window.
                mouse(&mut ui, Down(Left), x, y);
                mouse(&mut ui, Up(Left), x, y);
                room_screen(&mut ui, cols, 60);
                assert_eq!(ui.view.search_matches.len(), 9);
                assert!(!ui.chat_search.as_ref().unwrap().you_only);
                assert!(!ui.chat_search.as_ref().unwrap().from_you);
            }
        }
        let field = ui.view.search_field;
        for (x, y) in [(field.x, field.y), (field.x + 1, field.y + 1)] {
            mouse(&mut ui, Down(Left), x, y);
            mouse(&mut ui, Up(Left), x, y);
            assert_eq!(ui.chat_search.as_ref().unwrap().focus, Query);
        }
    }
}

#[test]
fn clicking_find_toggle_boxes_and_labels_at_narrow_width() {
    assert_find_toggle_clicks([60, 100]);
}

#[test]
fn clicking_find_toggle_boxes_and_labels_at_wide_width() {
    assert_find_toggle_clicks([100, 60]);
}

fn assert_selected_find_highlight(ui: &mut BusUi, cols: u16) {
    ui.compute_view(cols, 60);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, 60));
    ui.render(&mut buffer);
    let current = ui.chat_search.as_ref().unwrap().current;
    let (row, range) = &ui.view.search_matches[current];
    assert!(on_screen(ui, *row));
    let line = &ui.history.cached()[*row];
    let column = unicode_width::UnicodeWidthStr::width(&line.text[..range.start]) as u16;
    let y = ui.view.history_text.y + (*row - ui.main_scroll) as u16;
    assert_eq!(
        buffer[(ui.view.history_text.x + column, y)].bg,
        ratatui::style::Color::Rgb(240, 190, 70)
    );
}
