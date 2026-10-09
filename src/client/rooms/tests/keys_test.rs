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
