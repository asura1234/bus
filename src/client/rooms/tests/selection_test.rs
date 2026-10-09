#[test]
fn dragging_a_rendered_prompt_copies_exactly_the_dragged_text_and_highlights_it() {
    let (mut ui, room, agent) = fixture();
    let source = "alpha **beta** gamma delta epsilon zeta eta theta\nsecond line";
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, source).unwrap();
    snapshot.state.submit_draft(room, 1).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(60, 30);
    let text = ui.view.history_text;
    let row_of = |ui: &BusUi, prefix: &str| {
        let index = ui
            .history
            .cached()
            .iter()
            .position(|line| line.text.starts_with(prefix))
            .unwrap();
        text.y + (index - ui.main_scroll) as u16
    };
    let first = row_of(&ui, "alpha beta");
    let second = row_of(&ui, "second");
    assert!(second > first + 1, "the long prompt must soft-wrap");
    // From "beta" to the end of "second": the soft wrap rejoins with a space,
    // the real line end stays, and the Markdown markers are not copied.
    let copied = drag_copy(&mut ui, (text.x + 6, first), (text.x + 5, second));
    assert_eq!(
        copied.as_deref(),
        Some("beta gamma delta epsilon zeta eta theta\nsecond")
    );
    ui.compute_view(60, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 30));
    ui.render(&mut buffer);
    let tint = ratatui::style::Color::Rgb(44, 88, 56);
    assert_ne!(buffer[(text.x, first)].bg, tint, "before the press point");
    assert_eq!(buffer[(text.x + 6, first)].bg, tint);
    assert_eq!(buffer[(text.x + 5, second)].bg, tint);
    assert_ne!(
        buffer[(text.x + 7, second)].bg,
        tint,
        "after the release point"
    );
    // Typing ends the history selection.
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(ui.history_selection.is_none());
}

#[test]
fn dragging_the_draft_copies_and_typing_replaces_the_selection() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("hello brave world");
    ui.notes_focus = true;
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let copied = drag_copy(&mut ui, (rect.x + 6, rect.y), (rect.x + 10, rect.y));
    assert_eq!(copied.as_deref(), Some("brave"));
    assert!(!ui.notes_focus, "pressing the draft focuses it");
    ui.input(
        &RawInputEvent::Paste("bold".into()),
        false,
        &mut Default::default(),
    );
    assert_eq!(ui.locals[&room].text.text, "hello bold world");
    assert!(ui.pending.iter().any(|p| matches!(&p.command,
        BusCommand::SetDraftText(id, text) if *id == room && text == "hello bold world")));
}

#[test]
fn dragging_notes_copies_and_a_plain_click_only_moves_the_caret() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::*};
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_room_notes(room, "goal: ship it")
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    let notes = ui.view.notes;
    assert_eq!(pointer(&mut ui, Down(Left), notes.x + 6, notes.y), None);
    assert_eq!(pointer(&mut ui, Drag(Left), notes.x + 8, notes.y), None);
    // Routine coordinator snapshots must not reset notes mid-selection.
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let copied = pointer(&mut ui, Up(Left), notes.x + 8, notes.y);
    assert_eq!(copied.as_deref(), Some("shi"));
    assert!(ui.notes_focus);
    assert_eq!(pointer(&mut ui, Down(Left), notes.x + 2, notes.y), None);
    assert_eq!(pointer(&mut ui, Up(Left), notes.x + 2, notes.y), None);
    let local = &ui.locals[&room];
    assert_eq!((local.notes.cursor, local.notes.selection()), (2, None));
    assert_eq!(local.notes.text, "goal: ship it");
}

#[test]
fn quote_preserves_newer_local_text_recipients_and_escaped_multiline_content() {
    let (mut ui, room, agent) = fixture();
    // Build a durable reply fixture through serialization, without exercising provider IO.
    let mut json = serde_json::to_value(&ui.snapshot.state).unwrap();
    json["rooms"][room.0.to_string()]["latest_replies"][agent.0.to_string()] = serde_json::json!({"request_id":100,"agent_id":agent.0,"text":"a\n\"quoted\" \\ @author","received_at_ms":5000});
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state = serde_json::from_value(json).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().text.insert("new local");
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.notes_focus = true;
    ui.quote(RequestId(100));
    assert_eq!(
        ui.locals[&room].text.text,
        "new local\nauthor: \"a\n\\\"quoted\\\" \\\\ @author\"\n"
    );
    assert!(ui.locals[&room].recipients.contains(&agent));
    assert!(!ui.notes_focus);
    assert!(ui.send_intent.is_none());
}

/// Typed text in every input sits on the box's own background: the grey
/// highlight is for selections and selected rows only, and the cursor still
/// shows where typing goes.
#[test]
fn typed_text_in_inputs_has_no_highlight_background() {
    let highlight = ratatui::style::Color::Rgb(46, 48, 58);
    let typed_cells = |ui: &mut BusUi, text: &str| {
        ui.compute_view(100, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let cursor = ui
            .view
            .cursor
            .as_ref()
            .expect("the focused input shows a cursor");
        let row: String = (0..100).map(|x| buffer[(x, cursor.y)].symbol()).collect();
        let start = row
            .find(text)
            .unwrap_or_else(|| panic!("{text} not on {row:?}"));
        let start = row[..start].chars().count() as u16;
        (0..text.len() as u16)
            .map(|offset| buffer[(start + offset, cursor.y)].bg)
            .collect::<Vec<_>>()
    };
    let type_text = |ui: &mut BusUi, text: &str| {
        for c in text.chars() {
            key(ui, KeyCode::Char(c), KeyModifiers::NONE);
        }
    };

    let (mut ui, room, _) = fixture();
    ui.open_room(room);
    type_text(&mut ui, "composerdraft");
    let cells = typed_cells(&mut ui, "composerdraft");
    assert!(
        cells.iter().all(|bg| *bg != highlight),
        "composer: {cells:?}"
    );

    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    type_text(&mut ui, "notesline");
    let cells = typed_cells(&mut ui, "notesline");
    assert!(cells.iter().all(|bg| *bg != highlight), "notes: {cells:?}");

    let (mut ui, room, _) = fixture();
    ui.open_room(room);
    ui.action(render::Action::NewAgent);
    type_text(&mut ui, "formname");
    let cells = typed_cells(&mut ui, "formname");
    assert!(cells.iter().all(|bg| *bg != highlight), "form: {cells:?}");
}

#[test]
fn dragging_after_a_joined_emoji_copies_the_rendered_character() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("a👩‍💻b");
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    assert_eq!(buffer[(rect.x + 3, rect.y)].symbol(), "b");

    let copied = drag_copy(&mut ui, (rect.x + 3, rect.y), (rect.x + 3, rect.y));

    assert_eq!(copied.as_deref(), Some("b"));
}

#[test]
fn dragging_a_combining_character_copies_the_whole_rendered_cell() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("ae\u{301}b");
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    assert_eq!(buffer[(rect.x + 1, rect.y)].symbol(), "e\u{301}");

    let copied = drag_copy(&mut ui, (rect.x + 1, rect.y), (rect.x + 1, rect.y));

    assert_eq!(copied.as_deref(), Some("e\u{301}"));
}

#[test]
fn the_composer_caret_counts_a_joined_emoji_as_two_cells() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("a👩‍💻b");
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);

    let cursor = ui.cursor().expect("composer caret");

    assert_eq!((cursor.x, cursor.y), (rect.x + 4, rect.y));
}
