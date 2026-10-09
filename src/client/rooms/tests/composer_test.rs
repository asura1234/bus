#[test]
fn multi_paragraph_bracketed_paste_with_host_color_replies_round_trips_into_draft() {
    let expected = "First paragraph.\n\n第二段 🚌\n\nThird @author + paragraph.\n\nFourth paragraph.\n\nFifth paragraph.\n";
    for terminator in ["\x1b\\", "\x07"] {
        let mut payload = format!("\x1b]10;rgb:eeee/dddd/cccc{terminator}");
        for (index, ch) in expected.chars().enumerate() {
            payload.push(ch);
            if index == 20 {
                for palette_index in 0..=255 {
                    payload.push_str(&format!(
                        "\x1b]4;{palette_index};rgb:1111/2222/3333{terminator}"
                    ));
                }
            }
        }
        payload.push_str(&format!("\x1b]11;rgb:0000/1111/2222{terminator}"));
        payload.push_str("\x1b[201~");

        for chunk_size in [1, 7, payload.len()] {
            let (mut ui, room, _) = fixture();
            let mut framer = crate::protocol::keys::host::RawInputByteFramer::for_host_input();
            framer.host_color_query_sent();
            assert!(framer.push(b"\x1b[200~").is_empty());
            let mut chunks = Vec::new();
            for chunk in payload.as_bytes().chunks(chunk_size) {
                chunks.extend(framer.push(chunk));
            }
            chunks.extend(framer.flush_timeout());
            let mut replies = 0;
            let mut pastes = 0;
            for chunk in chunks {
                for event in crate::protocol::keys::host::parse_raw_input_bytes_sync(&chunk) {
                    match &event {
                        RawInputEvent::HostDefaultColor { .. }
                        | RawInputEvent::HostPaletteColors { .. } => replies += 1,
                        RawInputEvent::Paste(_) => pastes += 1,
                        other => panic!("unexpected paste input: {other:?}"),
                    }
                    ui.input(&event, false, &mut Default::default());
                }
            }
            assert_eq!(
                ui.locals[&room].text.text, expected,
                "chunk size {chunk_size}"
            );
            assert_eq!(replies, 258, "host replies must retain their routing");
            assert_eq!(pastes, 1, "one paste must remain one edit");
            assert!(ui.send_intent.is_none());
            assert!(ui.pending.iter().all(|pending| !matches!(
                pending.command,
                BusCommand::Submit(_) | BusCommand::AttachFile(..)
            )));
        }
    }
}

#[test]
fn multi_paragraph_bracketed_paste_preserves_terminal_line_endings_in_draft() {
    let expected = "\nFirst paragraph.\n\n第二段 🚌\n\nThird paragraph.\n\nFourth paragraph.\n\nFifth paragraph.\n";
    for line_ending in ["\n", "\r", "\r\n"] {
        let (mut ui, room, _) = fixture();
        let raw = format!("\x1b[200~{}\x1b[201~", expected.replace('\n', line_ending));
        for event in crate::protocol::keys::host::parse_raw_input_bytes_sync(raw.as_bytes()) {
            ui.input(&event, false, &mut Default::default());
        }
        assert_eq!(ui.locals[&room].text.text, expected, "{line_ending:?}");
        assert!(ui.send_intent.is_none());
    }
}

#[test]
fn typing_plus_in_the_composer_inserts_plus() {
    for input in shifted_symbol_encodings('+', '=') {
        let (mut ui, room, _) = fixture();
        ui.input(
            &RawInputEvent::Key(input.clone()),
            false,
            &mut Default::default(),
        );
        assert_eq!(ui.locals[&room].text.text, "+", "for {input:?}");
        assert!(ui.form.is_none() && !ui.recipient_menu, "for {input:?}");
    }
}

#[test]
fn every_shifted_symbol_types_into_the_composer() {
    let shifted = [
        ('~', '`'),
        ('!', '1'),
        ('@', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('^', '6'),
        ('&', '7'),
        ('*', '8'),
        ('(', '9'),
        (')', '0'),
        ('_', '-'),
        ('+', '='),
        ('{', '['),
        ('}', ']'),
        ('|', '\\'),
        (':', ';'),
        ('"', '\''),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
    ];
    for (symbol, base) in shifted {
        for input in shifted_symbol_encodings(symbol, base) {
            let (mut ui, room, agent) = fixture();
            ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
            ui.input(
                &RawInputEvent::Key(input.clone()),
                false,
                &mut Default::default(),
            );
            assert_eq!(
                ui.locals[&room].text.text,
                symbol.to_string(),
                "{symbol} was not typed for {input:?}"
            );
            assert!(ui.form.is_none() && !ui.recipient_menu);
            assert!(ui.send_intent.is_none());
        }
    }
}

#[test]
fn symbols_stay_literal_in_paste_notes_and_forms() {
    let (mut ui, room, _) = fixture();
    for event in [
        RawInputEvent::Paste("@author + file.md".into()),
        RawInputEvent::Text(crate::protocol::keys::TextCommit::new("@+")),
    ] {
        ui.input(&event, false, &mut Default::default());
    }
    assert_eq!(ui.locals[&room].text.text, "@author + file.md@+");
    assert!(!ui.recipient_menu && ui.form.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    for symbol in ['@', '+'] {
        key(&mut ui, KeyCode::Char(symbol), KeyModifiers::SHIFT);
    }
    assert_eq!(ui.locals[&room].notes.text, "@+");
    assert!(!ui.recipient_menu && ui.form.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('o'), KeyModifiers::CONTROL);
    for symbol in ['@', '+'] {
        key(&mut ui, KeyCode::Char(symbol), KeyModifiers::SHIFT);
    }
    let Some(forms::Form::Files(editor)) = &ui.form else {
        panic!("file form lost")
    };
    assert_eq!(editor.text, "~/@+");
    assert!(ui.send_intent.is_none());
}

#[test]
fn enter_without_checked_agents_shows_error_and_preserves_draft() {
    let (mut ui, room, _) = fixture();
    for text in ["hello", "@author hello", "/help me", "/other", ""] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new(text.into());
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert!(ui.send_intent.is_none());
        assert!(ui
            .pending
            .iter()
            .all(|p| !matches!(p.command, BusCommand::Submit(_))));
        assert_eq!(ui.locals[&room].text.text, text);
        assert!(room_screen(&mut ui, 100, 30).contains("Choose agents with @ before sending."));
    }
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("/help".into());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Keyboard shortcuts"));
    assert!(ui.send_intent.is_none());
}

#[test]
fn composer_autosave_is_silent_but_save_errors_remain_visible() {
    let (mut ui, room, _) = fixture();
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(ui.pending.iter().any(|pending| {
        matches!(&pending.command, BusCommand::SetDraftText(id, text) if *id == room && text == "x")
    }), "typing must still autosave");
    assert!(!room_screen(&mut ui, 100, 30).contains("Saving"));
    ui.error = Some("Could not save: disk full".into());
    assert!(room_screen(&mut ui, 100, 30).contains("Could not save: disk full"));
}

#[test]
fn composer_grows_with_newlines_and_wrapping_then_shrinks_after_deletion() {
    let (mut ui, room, _) = fixture();
    ui.compute_view(100, 30);
    let compact = composer_rect(&ui);
    for _ in 0..7 {
        key(&mut ui, KeyCode::Enter, KeyModifiers::SHIFT);
    }
    ui.compute_view(100, 30);
    let grown = composer_rect(&ui);
    assert_eq!(grown.height, 8, "every new blank line gets space");
    assert!(grown.y < compact.y);
    assert_eq!(grown.bottom(), compact.bottom());
    assert!(ui.send_intent.is_none());
    for _ in 0..7 {
        key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    }
    ui.compute_view(100, 30);
    assert_eq!(composer_rect(&ui), compact);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"界".repeat(150));
    ui.compute_view(100, 30);
    assert_eq!(composer_rect(&ui).height, 5, "wide wrapped text grows too");
    ui.compute_view(80, 30);
    assert!(
        composer_rect(&ui).height > 5,
        "resizing recomputes visual rows"
    );
}

#[test]
fn composer_uses_full_height_and_toggle_preserves_draft_and_recipients() {
    let (mut ui, room, agent) = fixture();
    let draft = (0..80)
        .map(|n| format!("line-{n:02}\n"))
        .collect::<String>();
    ui.locals.get_mut(&room).unwrap().text.insert(&draft);
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    room_screen(&mut ui, 100, 30);
    let full = composer_rect(&ui);
    assert_eq!(
        full.height,
        30 - ui.view.recipient_bar.height - 4,
        "use every row below the recipient chips and borders"
    );
    assert!(full.bottom() <= 28);
    key(
        &mut ui,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    room_screen(&mut ui, 100, 30);
    assert_eq!(composer_rect(&ui).height, 3);
    key(
        &mut ui,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    room_screen(&mut ui, 100, 30);
    assert_eq!(composer_rect(&ui), full);
    assert_eq!(ui.locals[&room].text.text, draft);
    assert_eq!(ui.locals[&room].recipients, [agent].into());
    assert!(ui.send_intent.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Add notes"));
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(room_screen(&mut ui, 100, 30).contains("[x] author"));
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.send_intent, Some(room));
}

#[test]
fn composer_page_keys_scroll_without_editing_and_typing_returns_to_caret() {
    let (mut ui, room, _) = fixture();
    let draft = (0..90)
        .map(|n| format!("draft-{n:02}\n"))
        .collect::<String>();
    ui.locals.get_mut(&room).unwrap().text.insert(&draft);
    let bottom = room_screen(&mut ui, 100, 30);
    assert!(bottom.contains("draft-89"));
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    let previous = room_screen(&mut ui, 100, 30);
    assert_ne!(previous, bottom, "Page Up must move the draft viewport");
    assert!(!previous.contains("draft-89"));
    assert!(ui.cursor().is_none(), "do not show a fake cursor offscreen");
    assert_eq!(ui.locals[&room].text.text, draft);
    assert_eq!(ui.locals[&room].text.cursor, draft.len());
    assert_eq!(
        ui.main_scroll, 0,
        "draft scrolling is separate from replies"
    );
    key(&mut ui, KeyCode::PageDown, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("draft-89"));
    for _ in 0..10 {
        key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
        ui.compute_view(100, 30);
    }
    assert!(room_screen(&mut ui, 100, 30).contains("draft-00"));
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("draft-89"));
    assert!(ui.cursor().is_some());
    assert_eq!(ui.locals[&room].text.text, format!("{draft}x"));
    for (cols, rows) in [(60, 12), (100, 45)] {
        room_screen(&mut ui, cols, rows);
        let rect = composer_rect(&ui);
        let cursor = ui.cursor().unwrap();
        assert!(rect.bottom() < rows);
        assert!(rect.contains((cursor.x, cursor.y).into()));
    }
}

#[test]
fn consecutive_sends_keep_checked_recipients_and_queue_the_successor() {
    // Resetting persisted recipients after Submit breaks the second Enter even
    // though the actual UI still shows the same checked recipient.
    fn process_commands(ui: &mut BusUi, state: &mut BusState) {
        ui.settle();
        while let Some(pending) = ui.pending.front().cloned() {
            let result = match pending.command {
                BusCommand::SetDraftText(room, text) => state.set_draft_text(room, &text),
                BusCommand::SetRecipients(room, ids) => state.set_draft_recipients(room, ids),
                BusCommand::Submit(room) => state.submit_draft(room, 10).map(|_| ()),
                command => panic!("unexpected command: {command:?}"),
            };
            ui.receive_event(BusEvent::CommandFinished {
                command_id: pending.id,
                result: result.map_err(|error| error.to_string()),
            });
            ui.receive_snapshot(Arc::new(BusSnapshot {
                state: state.clone(),
                revision: pending.id,
                last_command_id: pending.id,
                error: None,
            }));
            ui.settle();
        }
    }

    let (mut ui, room, agent) = fixture();
    let mut state = ui.snapshot.state.clone();
    let file = std::path::PathBuf::from("/project/context.md");
    state.attach_file(room, file.clone()).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    for text in ["first prompt", "queued successor"] {
        ui.input(
            &RawInputEvent::Paste(text.into()),
            false,
            &mut Default::default(),
        );
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        process_commands(&mut ui, &mut state);
        assert!(ui.error.is_none(), "send failed: {:?}", ui.error);
        assert!(ui.locals[&room].text.text.is_empty());
        assert_eq!(ui.locals[&room].recipients, [agent].into());
        assert!(state.room(room).unwrap().draft.text.is_empty());
        assert!(state.room(room).unwrap().draft.files.is_empty());
        if text == "first prompt" {
            let first = state.queued_requests(agent)[0];
            assert_eq!(
                state.request(first).unwrap().prompt.files,
                vec![file.clone()]
            );
            state.begin_submission(first, "launch", 0).unwrap();
            state
                .record_submission(
                    first,
                    SubmissionOutcome::Confirmed {
                        provider_session_id: Some("session".into()),
                        provider_turn_id: Some("turn".into()),
                    },
                )
                .unwrap();
        }
    }
    let first = state.agent(agent).unwrap().current_request.unwrap();
    assert_eq!(state.request(first).unwrap().phase, RequestPhase::Active);
    assert_eq!(state.request(first).unwrap().prompt.text, "first prompt");
    assert_eq!(state.queued_requests(agent).len(), 1);
    let next = state.request(state.queued_requests(agent)[0]).unwrap();
    assert_eq!(next.phase, RequestPhase::Queued);
    assert_eq!(next.prompt.text, "queued successor");
    assert_eq!(next.prompt.recipient_ids, [agent].into());
    assert!(next.prompt.files.is_empty());
    assert_eq!(
        state.room(room).unwrap().draft.recipient_ids,
        [agent].into()
    );
    assert_eq!(ui.snapshot.state, state);
}

#[test]
fn wrap_ranges_break_between_words_and_cover_every_source_byte() {
    let text = "hello world\nabcdefghij 界界xy";
    let rows: Vec<_> = render::wrap_ranges(text, 5)
        .into_iter()
        .map(|range| &text[range])
        .collect();
    assert_eq!(rows, ["hello ", "world", "abcde", "fghij ", "界界", "xy"]);
    assert_eq!(rows.concat(), text.replace('\n', ""));
}

#[test]
fn composer_wraps_whole_words_and_places_the_caret_on_the_wrapped_row() {
    let (mut ui, room, _) = fixture();
    ui.compute_view(100, 30);
    let width = usize::from(composer_rect(&ui).width);
    let filler = "a".repeat(width - 3);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&format!("{filler} hello world"));
    assert_eq!(
        composer_rows(&mut ui, 100, 30)[..2],
        [filler, "hello world".into()]
    );
    let rect = composer_rect(&ui);
    let cursor = ui.cursor().unwrap();
    assert_eq!((cursor.x, cursor.y), (rect.x + 11, rect.y + 1));
    // The caret before a wrapped word sits where that word starts.
    ui.locals.get_mut(&room).unwrap().text.cursor = width - 2;
    ui.compute_view(100, 30);
    let cursor = ui.cursor().unwrap();
    assert_eq!((cursor.x, cursor.y), (rect.x, rect.y + 1));
}

#[test]
fn pending_text_commands_coalesce_without_overwriting_the_inflight_generation() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("a");
    ui.text_changed(room);
    ui.pending.front_mut().unwrap().enqueued = true;
    for _ in 0..100 {
        ui.locals.get_mut(&room).unwrap().text.insert("x");
        ui.text_changed(room);
    }
    assert_eq!(ui.pending.len(), 2);
    assert!(matches!(&ui.pending[0].command,BusCommand::SetDraftText(_,text) if text=="a"));
    assert!(matches!(&ui.pending[1].command,BusCommand::SetDraftText(_,text) if text.len()==101));
}

#[test]
fn notes_room_navigation_and_all_recipients_remain_room_local() {
    let (mut ui, room, agent) = fixture();
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("notes @literal".into()),
        false,
        &mut Default::default(),
    );
    ui.action(render::Action::Recipients);
    ui.action(render::Action::Recipient(None));
    assert!(ui.locals[&room].recipients.contains(&agent));
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(other);
    assert!(ui.locals[&other].notes.text.is_empty());
    assert!(ui.locals[&other].recipients.is_empty());
    ui.open_room(room);
    assert_eq!(ui.locals[&room].notes.text, "notes @literal");
}

#[test]
fn storage_recovery_in_the_failed_command_batch_requeues_the_unsaved_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("retained draft");
    ui.text_changed(room);
    let command_id = ui.pending.front().unwrap().id;
    ui.pending.front_mut().unwrap().enqueued = true;

    // The worker can acknowledge a command while storage is paused, then
    // recover in the same loop before it publishes the resulting snapshot.
    ui.receive_event(BusEvent::CommandFinished {
        command_id,
        result: Err("Storage paused; retrying".into()),
    });
    ui.receive_event(BusEvent::StorageRecovered);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = command_id;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();

    assert!(
        ui.pending.iter().any(|pending| {
            matches!(&pending.command, BusCommand::SetDraftText(id, text)
                if *id == room && text == "retained draft")
        }),
        "recovery must retry the rejected local edit even before it was settled"
    );
    assert!(ui.failed.is_empty());
    assert!(ui.error.is_none());
}

#[test]
fn deleting_a_recalled_prompt_keeps_the_edit_as_the_live_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().recall = vec!["previous prompt".into()];
    ui.input(
        &RawInputEvent::Paste("unfinished draft".into()),
        false,
        &mut Default::default(),
    );

    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "previous prompt");
    key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "previous promp");
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);

    assert_eq!(
        ui.locals[&room].text.text, "previous promp",
        "editing recalled text must end recall navigation, as typing does"
    );
    assert_eq!(ui.locals[&room].history_index, None);
}

#[test]
fn deleting_the_line_continuation_marker_restores_enter_to_send() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.input(
        &RawInputEvent::Paste("ready to send".into()),
        false,
        &mut Default::default(),
    );

    key(&mut ui, KeyCode::Char('\\'), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "ready to send");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);

    assert_eq!(ui.locals[&room].text.text, "ready to send");
    assert_eq!(ui.send_intent, Some(room));
}
