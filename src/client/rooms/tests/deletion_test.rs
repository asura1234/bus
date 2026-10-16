use super::*;

#[test]
fn pending_delete_keeps_sidebar_details_and_only_the_original_delete_button() {
    let (mut ui, _room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_agent_details_disclosed(agent, true)
        .unwrap();
    snapshot.state.prepare_delete_agent(agent).unwrap();
    snapshot
        .state
        .set_agent_error(agent, Some("The terminal could not be closed".into()))
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    let screen = room_screen(&mut ui, 100, 40);
    assert!(screen.contains("main"));
    assert!(screen.contains("/project"));
    assert!(!screen.contains("Retry delete"));
    assert_eq!(
        ui.view
            .hits
            .iter()
            .filter(|hit| {
                hit.action == render::Action::Delete(deletion::DeleteTarget::Agent(agent))
            })
            .count(),
        1
    );
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Details(agent)));
}

#[test]
fn confirmed_delete_reaches_worker_before_prior_submit_snapshot_arrives() {
    let (mut ui, room, agent) = fixture();
    let (handle, commands) = BusHandle::test_channel(Arc::clone(&ui.snapshot));
    ui.handle = Some(handle);
    let submit = ui.queue(BusCommand::Submit(room), Effect::Submit(room, 0));
    ui.tick();
    assert!(matches!(commands.try_recv(), Ok((id, BusCommand::Submit(_))) if id == submit));
    // Worker is polling after Submit; no new snapshot or ack has reached the UI.
    ui.queue(
        BusCommand::SetNotes(room, "keep notes".into()),
        Effect::None,
    );
    ui.start_delete(deletion::DeleteTarget::Agent(agent));
    ui.confirm_delete();
    let delete = ui.deletion.as_ref().unwrap().command_id.unwrap();
    ui.tick();
    assert!(matches!(
        commands.try_recv(),
        Ok((_, BusCommand::SetNotes(_, _)))
    ));
    assert!(
        matches!(commands.try_recv(), Ok((id, BusCommand::DeleteAgent(target))) if id == delete && target == agent)
    );
    ui.tick();
    assert!(
        commands.try_recv().is_err(),
        "must not send twice while awaiting ack"
    );
    assert_eq!(
        ui.pending.len(),
        3,
        "exact acknowledgements still retire commands"
    );
    assert!(ui.pending.iter().all(|pending| pending.enqueued));
}

#[test]
fn deletion_snapshot_retires_failed_saves_for_the_deleted_room() {
    let (mut ui, room, _) = fixture();
    let attachment = ui.queue(
        BusCommand::AttachFile(room, "/missing.md".into()),
        Effect::Files(room),
    );
    ui.pending.front_mut().unwrap().enqueued = true;
    ui.start_delete(deletion::DeleteTarget::Room(room));
    ui.confirm_delete();
    let delete = ui.deletion.as_ref().unwrap().command_id.unwrap();
    ui.pending.back_mut().unwrap().enqueued = true;
    ui.receive_event(BusEvent::CommandFinished {
        command_id: attachment,
        result: Err("missing file".into()),
    });
    ui.receive_event(BusEvent::CommandFinished {
        command_id: delete,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.delete_room(room).unwrap();
    snapshot.last_command_id = delete;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(
        ui.failed.is_empty(),
        "ack must not revive failed saves after deletion"
    );
    ui.request_quit();
    ui.settle();
    assert_eq!(ui.pending.len(), 1);
    assert!(matches!(ui.pending[0].command, BusCommand::Shutdown));
}

#[test]
fn delete_room_warning_blocks_underlying_input_and_escape_preserves_draft() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut ui, room, agent) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("keep this draft");
    ui.compute_view(100, 30);
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 3);
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("Delete room"), "{screen}");
    assert!(screen.contains("1 agent") && screen.contains("session data"));
    assert!(!screen.contains("Project files"));
    assert!(screen.contains("Cancel (Esc)") && screen.contains("OK (Enter)"));
    assert!(
        ui.pending.is_empty(),
        "opening a warning must not delete or navigate"
    );
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    key(&mut ui, KeyCode::F(6), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("must not edit".into()),
        false,
        &mut Default::default(),
    );
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 1);
    assert!(room_screen(&mut ui, 100, 30).contains("Delete room"));
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(!room_screen(&mut ui, 100, 30).contains("Delete room"));
    assert_eq!(ui.locals[&room].text.text, "keep this draft");
    assert!(ui.snapshot.state.room(room).is_some());
    assert!(ui.snapshot.state.agent(agent).is_some());
    assert!(ui.pending.is_empty());
}

#[test]
fn delete_agent_enter_queues_once_and_escape_returns_to_the_same_terminal() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut ui, _, agent) = fixture();
    ui.open_terminal(agent);
    ui.receive_event(BusEvent::TerminalFocused {
        agent,
        pane_id: "pane".into(),
    });
    ui.pending.clear();
    ui.compute_view(100, 30);
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 7);
    assert!(room_screen(&mut ui, 100, 30).contains("Delete agent"));
    assert!(
        !ui.terminal_ready(Some("pane")),
        "modal must capture terminal input"
    );
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.terminal_ready(Some("pane")));
    assert!(ui.pending.is_empty());
    ui.compute_view(100, 30);
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 7);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.pending.len(), 1);
    assert!(matches!(ui.pending[0].command, BusCommand::DeleteAgent(id) if id == agent));
    assert!(
        ui.snapshot.state.agent(agent).is_some(),
        "wait for durable success"
    );
    let command_id = ui.pending[0].id;
    ui.receive_event(BusEvent::CommandFinished {
        command_id,
        result: Err("terminal stop failed".into()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = command_id;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("Deletion did not finish") && screen.contains("OK (Enter)"));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        ui.pending.is_empty(),
        "failure acknowledgement must not retry deletion"
    );
    assert!(ui.deletion.is_none());
}

#[test]
fn confirming_room_delete_cancels_unsent_dispatch_but_preserves_other_drafts() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("not sent");
    ui.send_intent = Some(room);
    ui.queue(BusCommand::Submit(room), Effect::Submit(room, 0));
    ui.compute_view(100, 30);
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 3);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.send_intent.is_none());
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::Submit(_))));
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::DeleteRoom(id) if id == room)));
    assert_eq!(
        ui.locals[&room].text.text, "not sent",
        "keep local text until success"
    );
}

#[test]
fn confirmed_room_deletion_reconciles_removed_agents_and_preserves_another_room() {
    use crossterm::event::{MouseButton, MouseEventKind};
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let keep = snapshot.state.create_room("keep").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&keep).unwrap().text.insert("other draft");
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("deleted draft");
    ui.open_terminal(agent);
    ui.pending.clear();
    ui.compute_view(100, 30);
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 3);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let command_id = ui.pending[0].id;
    ui.pending[0].enqueued = true;
    ui.receive_event(BusEvent::CommandFinished {
        command_id,
        result: Ok(()),
    });
    ui.settle();
    assert!(
        ui.deletion.is_some(),
        "must wait for matching durable snapshot"
    );
    assert!(ui.locals.contains_key(&room));
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.delete_room(room).unwrap();
    snapshot.last_command_id = command_id;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(ui.deletion.is_none());
    assert_eq!(ui.room, Some(keep));
    assert!(ui.terminal.is_none() && ui.target_pane.is_none());
    assert!(!ui.locals.contains_key(&room));
    assert_eq!(ui.locals[&keep].text.text, "other draft");
    assert!(ui
        .pending
        .iter()
        .all(|pending| matches!(pending.command, BusCommand::SelectRoom(id) if id == keep)));
}

#[test]
fn deleting_agent_prunes_checked_recipient_and_last_room_stays_deleted_after_restart() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.delete_agent(agent).unwrap();
    ui.receive_snapshot(Arc::new(snapshot.clone()));
    assert!(ui.locals[&room].recipients.is_empty());
    assert_eq!(ui.room, Some(room));
    snapshot.state.delete_room(room).unwrap();
    ui.receive_snapshot(Arc::new(snapshot.clone()));
    assert!(ui.room.is_none() && ui.locals.is_empty());
    assert!(room_screen(&mut ui, 100, 30)
        .contains("No rooms. Click ROOMS + or press Ctrl+Shift+R to add one."));
    let mut reopened = BusUi::new(Arc::new(snapshot));
    assert!(!reopened.seed_first_room);
    assert!(room_screen(&mut reopened, 100, 30)
        .contains("No rooms. Click ROOMS + or press Ctrl+Shift+R to add one."));
}

#[test]
fn delete_popup_buttons_are_clickable_after_resize_and_repeated_enter_is_ignored() {
    use crossterm::event::{KeyEventKind, MouseButton, MouseEventKind};
    let (mut ui, room, _) = fixture();
    ui.compute_view(100, 30);
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), 25, 3);
    for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
        let mut enter = TerminalKey::new(KeyCode::Enter, KeyModifiers::NONE);
        enter.kind = kind;
        ui.input(&RawInputEvent::Key(enter), false, &mut Default::default());
    }
    assert!(ui.pending.is_empty());
    let screen = room_screen(&mut ui, 60, 12);
    assert!(screen.contains("Cancel (Esc)") && screen.contains("OK (Enter)"));
    let cancel = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::CancelDelete)
        .unwrap()
        .rect;
    mouse(
        &mut ui,
        MouseEventKind::Down(MouseButton::Left),
        cancel.x,
        cancel.y,
    );
    assert!(ui.deletion.is_none() && ui.pending.is_empty());
    ui.compute_view(60, 12);
    let delete = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Delete(deletion::DeleteTarget::Room(room)))
        .unwrap()
        .rect;
    mouse(
        &mut ui,
        MouseEventKind::Down(MouseButton::Left),
        delete.x,
        delete.y,
    );
    ui.compute_view(60, 12);
    let ok = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::ConfirmDelete)
        .unwrap()
        .rect;
    mouse(&mut ui, MouseEventKind::Down(MouseButton::Left), ok.x, ok.y);
    assert_eq!(ui.pending.len(), 1);
    assert!(matches!(ui.pending[0].command, BusCommand::DeleteRoom(id) if id == room));
}

#[test]
fn only_the_selected_room_offers_a_delete_button() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot.state.create_room("second").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    for selected in [first, second] {
        ui.open_room(selected);
        ui.compute_view(100, 30);
        let deletable: Vec<_> = ui
            .view
            .hits
            .iter()
            .filter_map(|hit| match hit.action {
                render::Action::Delete(deletion::DeleteTarget::Room(room)) => Some(room),
                _ => None,
            })
            .collect();
        assert_eq!(deletable, [selected]);
    }
}

#[test]
fn delete_warning_ctrl_c_preserves_the_underlying_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("keep this draft");
    ui.start_delete(deletion::DeleteTarget::Room(room));

    key(&mut ui, KeyCode::Char('c'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);

    assert_eq!(ui.locals[&room].text.text, "keep this draft");
    assert!(ui.pending.is_empty());
    assert!(ui.deletion.is_none());
}

#[test]
fn recipient_menu_enter_does_not_select_all_after_the_highlighted_agent_disappears() {
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .create_agent(room, "second", Provider::Codex, "/project".into(), None)
        .unwrap();
    let removed = snapshot
        .state
        .create_agent(room, "third", Provider::Codex, "/project".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    for _ in 0..3 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    assert_eq!(ui.recipient_index, 3);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.delete_agent(removed).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    assert!(ui.recipient_menu);
    assert!(ui.locals[&room].recipients.is_empty());

    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);

    assert!(
        ui.locals[&room].recipients.len() <= 1,
        "only the explicitly highlighted All row may select every surviving agent"
    );
}
