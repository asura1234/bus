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
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let copied = drag_copy(&mut ui, (rect.x + 6, rect.y), (rect.x + 10, rect.y));
    assert_eq!(copied.as_deref(), Some("brave"));
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
fn quote_preserves_newer_local_text_recipients_and_escaped_multiline_content() {
    let (mut ui, room, agent) = fixture();
    // Quote a durable request reply, rather than an obsolete latest-only cache.
    let mut snapshot = (*ui.snapshot).clone();
    let request = snapshot
        .state
        .submit_message_with(
            room,
            Draft {
                text: "question".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            1000,
            true,
        )
        .unwrap()[0];
    snapshot
        .state
        .finish_for_test(request, "a\n\"quoted\" \\ @author", 5000);
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().text.insert("new local");
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.quote(request);
    assert_eq!(
        ui.locals[&room].text.text,
        "new local\nauthor: \"a\n\\\"quoted\\\" \\\\ @author\"\n"
    );
    assert!(ui.locals[&room].recipients.contains(&agent));
    assert!(!ui.notes_focus);
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

#[test]
fn the_composer_caret_stays_on_the_row_when_a_joined_emoji_fits_at_its_edge() {
    let (mut ui, room, _) = fixture();
    ui.compute_view(100, 30);
    let width = composer_rect(&ui).width;
    let draft = format!("{}👩‍💻b", "a".repeat(usize::from(width - 3)));
    ui.locals.get_mut(&room).unwrap().text.insert(&draft);
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let cursor = ui.cursor().expect("composer caret");

    assert_eq!((cursor.x, cursor.y), (rect.right() - 1, rect.y));
    assert_eq!(buffer[(rect.right() - 1, rect.y)].symbol(), "b");
}
