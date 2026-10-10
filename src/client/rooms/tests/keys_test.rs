use super::*;

#[test]
fn ctrl_e_moves_to_line_end_and_shift_toggles_composer_size() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("hello\nworld");
    ui.locals.get_mut(&room).unwrap().text.cursor = 0;
    key(&mut ui, KeyCode::Char('e'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&room].text.cursor, 5);
    assert_eq!(ui.locals[&room].composer_size, ComposerSize::Auto);
    key(
        &mut ui,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert_eq!(ui.locals[&room].composer_size, ComposerSize::Full);
}

#[test]
fn ctrl_r_searches_history_and_shift_opens_a_room_form() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().recall = vec!["alpha path".into(), "beta note".into()];
    key(&mut ui, KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert!(ui.form.is_none());
    assert_eq!(ui.locals[&room].text.text, "beta note");
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "alpha path");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.history_search.is_none());
    assert_eq!(ui.locals[&room].text.text, "alpha path");
    key(
        &mut ui,
        KeyCode::Char('r'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    assert!(matches!(ui.form, Some(forms::Form::Room(_))));
}

#[test]
fn up_browses_recall_and_esc_esc_saves_the_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().recall = vec!["older".into(), "newer".into()];
    ui.locals.get_mut(&room).unwrap().text.insert("live");
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "newer");
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "older");
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "live");
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.locals[&room].text.text.is_empty());
    assert_eq!(
        ui.locals[&room].recall.last().map(String::as_str),
        Some("live")
    );
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "live");
}

#[test]
fn stash_backslash_enter_and_ctrl_g_use_composer_chords() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("parked");
    key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert!(ui.locals[&room].text.text.is_empty());
    key(&mut ui, KeyCode::Char('s'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&room].text.text, "parked");
    ui.locals.get_mut(&room).unwrap().text = Default::default();
    key(&mut ui, KeyCode::Char('\\'), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "\n");
    assert!(ui.send_intent.is_none());
    let mut outcome = crate::client::compositor::ClientShellInput::default();
    ui.input(
        &RawInputEvent::Key(TerminalKey::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
        false,
        &mut outcome,
    );
    assert!(outcome.actions.iter().any(|action| matches!(
        action,
        crate::client::compositor::ClientShellAction::EditComposer
    )));
}

#[test]
fn ctrl_v_does_not_insert_a_letter() {
    let (mut ui, room, _) = fixture();
    key(&mut ui, KeyCode::Char('v'), KeyModifiers::CONTROL);
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn apply_external_edit_replaces_the_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("before");
    ui.apply_external_edit(room, "from editor".into());
    assert_eq!(ui.locals[&room].text.text, "from editor");
}

#[test]
fn composer_yank_rotation_replaces_the_yanked_selection_without_corrupting_unicode() {
    let (mut ui, room, _) = fixture();
    for text in ["older", "界"] {
        ui.input(
            &RawInputEvent::Paste(text.into()),
            false,
            &mut Default::default(),
        );
        key(&mut ui, KeyCode::Char('u'), KeyModifiers::CONTROL);
    }
    ui.input(
        &RawInputEvent::Paste("atail".into()),
        false,
        &mut Default::default(),
    );
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    assert_eq!(
        drag_copy(&mut ui, (rect.x, rect.y), (rect.x, rect.y)).as_deref(),
        Some("a")
    );

    key(&mut ui, KeyCode::Char('y'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&room].text.text, "界tail");
    key(&mut ui, KeyCode::Char('y'), KeyModifiers::ALT);

    assert_eq!(ui.locals[&room].text.text, "oldertail");
}

#[test]
fn cursor_movement_preserves_live_draft_while_browsing_recall() {
    let (mut ui, room, _) = fixture();
    let local = ui.locals.get_mut(&room).unwrap();
    local.text.insert("live draft");
    local.recall = vec!["recalled prompt".into()];

    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "recalled prompt");
    assert_eq!(ui.locals[&room].history_index, Some(0));
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);

    assert_eq!(ui.locals[&room].text.text, "live draft");
}

#[test]
fn notes_deletion_preserves_the_composer_recall_draft() {
    let (mut ui, room, _) = fixture();
    let local = ui.locals.get_mut(&room).unwrap();
    local.text.insert("live draft");
    local.notes.insert("notes");
    local.recall = vec!["recalled prompt".into()];

    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].notes.text, "note");
    assert_eq!(ui.locals[&room].text.text, "recalled prompt");
    assert_eq!(ui.locals[&room].history_index, Some(0));
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);

    assert_eq!(ui.locals[&room].text.text, "live draft");
}

#[test]
fn recipient_menu_all_and_agent_rows_keep_their_toggle_behavior() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot
        .state
        .create_agent(room, "second", Provider::Codex, "/project".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));

    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].recipients, [agent, other].into());
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    assert!(ui.locals[&room].recipients.is_empty());
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].recipients, [agent].into());
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);

    assert!(ui.locals[&room].recipients.is_empty());
}

#[test]
fn switching_rooms_restores_the_recall_source_even_after_an_old_save_ack() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot.state.create_room("second").unwrap();
    snapshot
        .state
        .set_draft_text(second, "second draft")
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    let local = ui.locals.get_mut(&first).unwrap();
    local.text.insert("first live draft");
    local.recall.push("first recalled prompt".into());
    key(&mut ui, KeyCode::Char('r'), KeyModifiers::CONTROL);
    let preview = ui
        .pending
        .iter_mut()
        .find(|pending| {
            matches!(&pending.command, BusCommand::SetDraftText(room, text)
            if *room == first && text == "first recalled prompt")
        })
        .unwrap();
    preview.enqueued = true;
    let preview_id = preview.id;

    ui.action(render::Action::Room(second));

    assert!(ui.history_search.is_none());
    assert_eq!(ui.locals[&first].text.text, "first live draft");
    assert_eq!(ui.locals[&second].text.text, "second draft");
    let restored_id = ui
        .pending
        .iter()
        .find(|pending| {
            matches!(&pending.command, BusCommand::SetDraftText(room, text)
            if *room == first && text == "first live draft")
        })
        .expect("the originating draft is saved back to its own room")
        .id;

    ui.receive_event(BusEvent::CommandFinished {
        command_id: preview_id,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_draft_text(first, "first recalled prompt")
        .unwrap();
    snapshot.last_command_id = preview_id;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert_eq!(ui.locals[&first].text.text, "first live draft");
    assert_eq!(ui.locals[&second].text.text, "second draft");

    ui.receive_event(BusEvent::CommandFinished {
        command_id: restored_id,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_draft_text(first, "first live draft")
        .unwrap();
    snapshot.last_command_id = restored_id;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);

    assert_eq!(ui.locals[&second].text.text, "second draftx");
    assert_eq!(ui.locals[&first].text.text, "first live draft");
    assert_eq!(
        ui.snapshot.state.room(first).unwrap().draft.text,
        "first live draft"
    );
}

#[test]
fn reopening_the_current_room_preserves_recall_and_its_accepted_prompt() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot.state.create_room("second").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    let local = ui.locals.get_mut(&first).unwrap();
    local.text.insert("live draft");
    local.recall.push("recalled prompt".into());
    key(&mut ui, KeyCode::Char('r'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::NONE);

    key(&mut ui, KeyCode::F(6), KeyModifiers::NONE);

    assert_eq!(ui.history_search.as_ref().unwrap().query, "p");
    assert_eq!(ui.locals[&first].text.text, "recalled prompt");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.history_search.is_none());
    ui.open_room(second);
    ui.open_room(first);
    assert_eq!(ui.locals[&first].text.text, "recalled prompt");
}

#[test]
fn room_switching_preserves_up_down_recall_without_ctrl_r() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot.state.create_room("second").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    let local = ui.locals.get_mut(&first).unwrap();
    local.text.insert("live draft");
    local.recall.push("recalled prompt".into());
    key(&mut ui, KeyCode::Up, KeyModifiers::NONE);

    ui.open_room(second);
    ui.open_room(first);

    assert_eq!(ui.locals[&first].text.text, "recalled prompt");
    assert_eq!(ui.locals[&first].history_index, Some(0));
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(ui.locals[&first].text.text, "live draft");
}

#[test]
fn enter_retry_after_failed_queued_send_uses_immediate_delivery() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::ALT);
    let failed_save = ui.pending.front().expect("draft save").id;
    ui.receive_event(BusEvent::CommandFinished {
        command_id: failed_save,
        result: Err("Storage paused; retrying".into()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = failed_save;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(ui.send_intent.is_none());
    assert_eq!(ui.locals[&room].text.text, "x");

    ui.receive_event(BusEvent::StorageRecovered);
    assert!(ui.failed.is_empty());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let saved = ui.pending.front().expect("retried draft save").id;
    ui.receive_event(BusEvent::CommandFinished {
        command_id: saved,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, "x").unwrap();
    snapshot.last_command_id = saved;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(
        ui.pending
            .iter()
            .any(|pending| { matches!(pending.command, BusCommand::Submit(id) if id == room) }),
        "Enter should submit immediately after recovery, got {:?}",
        ui.pending
    );
}

#[test]
fn recovered_draft_saves_wait_for_snapshot_and_retry_the_latest_edits() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("initial draft");
    ui.text_changed(room);
    ui.locals.get_mut(&room).unwrap().notes.insert("room notes");
    ui.notes_changed(room);
    let ids: Vec<_> = ui
        .pending
        .iter_mut()
        .map(|pending| {
            pending.enqueued = true;
            pending.id
        })
        .collect();
    for id in &ids {
        ui.receive_event(BusEvent::CommandFinished {
            command_id: *id,
            result: Err("Storage paused; retrying".into()),
        });
    }
    ui.receive_event(BusEvent::StorageRecovered);
    ui.settle();
    assert_eq!(
        ui.pending.len(),
        2,
        "recovery must preserve the snapshot barrier"
    );
    assert!(ui.pending.iter().all(|pending| ids.contains(&pending.id)));

    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(" plus newer edit");
    ui.text_changed(room);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = *ids.last().unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();

    assert!(ui.failed.is_empty());
    assert!(ui.error.is_none());
    assert_eq!(
        ui.pending.len(),
        2,
        "coalesce retries to the latest save per field"
    );
    assert!(ui.pending.iter().any(|pending| {
        matches!(&pending.command, BusCommand::SetDraftText(id, text)
            if *id == room && text == "initial draft plus newer edit")
    }));
    assert!(ui.pending.iter().any(|pending| {
        matches!(&pending.command, BusCommand::SetNotes(id, text)
            if *id == room && text == "room notes")
    }));
    assert!(ui
        .pending
        .iter()
        .all(|pending| pending.result.is_none() && !pending.enqueued));
}

#[test]
fn storage_recovery_keeps_a_rejected_room_creation_error_visible() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewRoom);
    for c in "new room".chars() {
        key(&mut ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.pending.iter().any(|pending| {
        matches!(&pending.command, BusCommand::CreateRoom(name) if name == "new room")
    }));
    let ids: Vec<_> = ui
        .pending
        .iter_mut()
        .map(|pending| {
            pending.enqueued = true;
            pending.id
        })
        .collect();
    for id in &ids {
        ui.receive_event(BusEvent::CommandFinished {
            command_id: *id,
            result: Err("Storage paused; retrying".into()),
        });
    }
    ui.receive_event(BusEvent::StorageRecovered);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = *ids.last().unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();

    assert!(!ui
        .snapshot
        .state
        .rooms()
        .any(|room| room.name == "new room"));
    assert!(ui.pending.is_empty(), "room creation has not been retried");
    assert!(
        ui.visible_error().is_some(),
        "recovery must not hide rejection of an operation that was not retried"
    );
}
