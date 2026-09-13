use super::*;
use crate::{
    bus::{model::*, runtime::*},
    input::TerminalKey,
    raw_input::RawInputEvent,
};
use crossterm::event::{KeyCode, KeyModifiers};
use std::sync::Arc;
#[path = "history_tests.rs"]
mod history_tests;
fn fixture() -> (BusUi, RoomId, AgentId) {
    let mut state = BusState::default();
    let room = state.create_room("room").unwrap();
    let agent = state
        .create_agent(
            room,
            "author",
            Provider::Codex,
            "/project".into(),
            Some("main".into()),
        )
        .unwrap();
    let ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    (ui, room, agent)
}
fn key(ui: &mut BusUi, code: KeyCode, modifiers: KeyModifiers) {
    ui.input(
        &RawInputEvent::Key(TerminalKey::new(code, modifiers)),
        false,
        &mut Default::default(),
    );
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
fn creation_events_survive_snapshots_from_before_the_created_target() {
    let (mut ui, old_room, _) = fixture();
    let mut created = (*ui.snapshot).clone();
    let room = created.state.create_room("new room").unwrap();
    let mut intermediate = (*ui.snapshot).clone();
    intermediate.revision += 1;
    ui.receive_event(BusEvent::RoomCreated(room));
    ui.receive_snapshot(Arc::new(intermediate));
    assert_eq!(ui.room, Some(room), "intermediate snapshot is not deletion");
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::SelectRoom(id) if id == room)));
    created.revision += 2;
    ui.receive_snapshot(Arc::new(created.clone()));
    assert!(ui.locals.contains_key(&room));
    assert!(ui.locals.contains_key(&old_room));

    let mut added = created.clone();
    let agent = added
        .state
        .create_agent(room, "new agent", Provider::Codex, "/project".into(), None)
        .unwrap();
    created.revision += 1;
    ui.receive_event(BusEvent::AgentAdded(agent));
    ui.receive_snapshot(Arc::new(created));
    assert_eq!(ui.terminal, Some(agent));
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::FocusTerminal(id) if id == agent)));
    added.revision += 2;
    ui.receive_snapshot(Arc::new(added));
    assert_eq!(ui.terminal, Some(agent));
}

#[test]
fn composer_symbol_shortcuts_work_for_legacy_and_shifted_terminal_keys() {
    for (symbol, base) in [('@', '2'), ('+', '=')] {
        for input in [
            TerminalKey::new(KeyCode::Char(symbol), KeyModifiers::NONE),
            TerminalKey::new(KeyCode::Char(symbol), KeyModifiers::SHIFT),
            TerminalKey::new(KeyCode::Char(base), KeyModifiers::SHIFT)
                .with_shifted_codepoint(symbol as u32),
        ] {
            let (mut ui, room, agent) = fixture();
            ui.locals.get_mut(&room).unwrap().text.insert("keep draft");
            ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
            ui.input(
                &RawInputEvent::Key(input.clone()),
                false,
                &mut Default::default(),
            );
            if symbol == '@' {
                assert!(ui.recipient_menu, "picker missing for {input:?}");
                assert!(ui.form.is_none());
            } else {
                assert!(
                    matches!(ui.form, Some(forms::Form::Files(_))),
                    "file picker missing for {input:?}"
                );
            }
            assert_eq!(ui.locals[&room].text.text, "keep draft");
            assert_eq!(ui.locals[&room].recipients, [agent].into());
            assert!(ui.send_intent.is_none());
        }
    }
}

#[test]
fn composer_symbol_shortcuts_leave_paste_notes_and_form_text_literal() {
    let (mut ui, room, _) = fixture();
    for event in [
        RawInputEvent::Paste("@author + file.md".into()),
        RawInputEvent::Text(crate::input::TextCommit::new("@+")),
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
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    for symbol in ['@', '+'] {
        key(&mut ui, KeyCode::Char(symbol), KeyModifiers::SHIFT);
    }
    let Some(forms::Form::Files(editor)) = &ui.form else {
        panic!("file form lost")
    };
    assert_eq!(editor.text, "~/@+");
    assert!(ui.send_intent.is_none());
}

fn composer_rect(ui: &BusUi) -> ratatui::layout::Rect {
    ui.view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Composer)
        .expect("composer hit area")
        .rect
}

fn room_screen(ui: &mut BusUi, cols: u16, rows: u16) -> String {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    buffer.content.iter().map(|cell| cell.symbol()).collect()
}

#[test]
fn room_chrome_uses_shared_phosphor_green() {
    let (mut ui, _, _) = fixture();
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let editor = composer_rect(&ui);
    let green = ratatui::style::Color::Rgb(102, 255, 102);
    for (x, y, symbol) in [
        (1, 3, "#"),                       // Sidebar room hash.
        (30, 1, "#"),                      // Room title hash.
        (1, 4, "─"),                       // Rooms/agents separator.
        (27, 0, "│"),                      // Column separator.
        (29, 2, "┌"),                      // Notes box.
        (29, 7, "─"),                      // History separator.
        (editor.x - 1, editor.y - 3, "┌"), // Composer box.
        (editor.x, editor.y - 1, "─"),     // Composer toolbar separator.
    ] {
        assert_eq!(buffer[(x, y)].symbol(), symbol);
        assert_eq!(buffer[(x, y)].fg, green, "accent at ({x}, {y})");
    }
    assert_ne!(buffer[(editor.x, editor.y)].fg, green);
    assert_ne!(buffer[(3, 3)].fg, green); // The room name keeps its text color.
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
    assert!(room_screen(&mut ui, 100, 30).contains("No rooms"));
    let mut reopened = BusUi::new(Arc::new(snapshot));
    assert!(!reopened.seed_first_room);
    assert!(room_screen(&mut reopened, 100, 30).contains("No rooms"));
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
        .find(|hit| hit.action == render::Action::Delete(super::deletion::DeleteTarget::Room(room)))
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
    for (x, y, symbol) in [(29, 2, "┌"), (98, 2, "┐"), (29, 6, "└"), (98, 6, "┘")] {
        assert_eq!(buffer[(x, y)].symbol(), symbol);
        assert_eq!(buffer[(x, y)].fg, color);
    }
    for x in 30..98 {
        for y in [2, 6, 7] {
            assert_eq!(buffer[(x, y)].symbol(), "─");
            assert_eq!(buffer[(x, y)].fg, color);
        }
    }
    assert_eq!(buffer[(30, 3)].symbol(), "R");
    assert_eq!(buffer[(30, 8)].symbol(), "Y");
    assert_eq!(buffer[(30, 9)].symbol(), "h");
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
        for notice in [false, true, false] {
            ui.error = notice.then(|| "Save failed".into());
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
    }
}

#[test]
fn sidebar_has_uppercase_headings_and_a_matching_divider_after_room_names() {
    let (mut ui, _, _) = fixture();
    for count in [1, 4] {
        let mut snapshot = (*ui.snapshot).clone();
        for index in 1..count {
            snapshot
                .state
                .create_room(&format!("room-{index}"))
                .unwrap();
        }
        ui.receive_snapshot(Arc::new(snapshot));
        ui.compute_view(100, 40);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 40));
        ui.render(&mut buffer);
        let row_text = |y| (1..26).map(|x| buffer[(x, y)].symbol()).collect::<String>();
        assert!(row_text(1).starts_with("ROOMS"));
        assert!(row_text(4 + count).starts_with("AGENTS"));
        let color = buffer[(27, 0)].fg;
        for x in 1..26 {
            assert_eq!(buffer[(x, 3 + count)].symbol(), "─");
            assert_eq!(buffer[(x, 3 + count)].fg, color);
        }
        assert_eq!(buffer[(25, 1)].symbol(), "+");
        assert_eq!(buffer[(25, 4 + count)].symbol(), "+");
    }
}

#[test]
fn entire_sidebar_scrolls_together_and_all_agents_remain_reachable() {
    use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    for index in 1..16 {
        snapshot
            .state
            .create_room(&format!("room-{index}"))
            .unwrap();
        snapshot
            .state
            .create_agent(
                room,
                &format!("agent-{index}"),
                Provider::Codex,
                "/project".into(),
                None,
            )
            .unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    let start = room_screen(&mut ui, 100, 30);
    mouse(&mut ui, ScrollDown, 2, 1); // Wheel over the room heading scrolls the whole column.
    let moved = room_screen(&mut ui, 100, 30);
    assert!(
        !moved.contains("ROOMS") && !moved.contains("Rooms"),
        "heading must scroll with content"
    );
    assert_ne!(moved, start);
    for _ in 0..80 {
        mouse(&mut ui, ScrollDown, 2, 20);
        ui.compute_view(100, 30);
    }
    assert!(room_screen(&mut ui, 100, 30).contains("agent-15"));
    let bottom = room_screen(&mut ui, 100, 30);
    for _ in 0..10 {
        mouse(&mut ui, ScrollDown, 2, 1);
        ui.compute_view(100, 30);
    }
    assert_eq!(
        room_screen(&mut ui, 100, 30),
        bottom,
        "must not overscroll to a blank column"
    );
    for _ in 0..80 {
        mouse(&mut ui, ScrollUp, 2, 20);
        ui.compute_view(100, 30);
    }
    assert_eq!(room_screen(&mut ui, 100, 30), start);
    assert_eq!(ui.main_scroll, 0);
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
fn help_enter_is_local_and_preserves_room_context() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .notes
        .insert("remember this");
    let before = serde_json::to_value(&ui.snapshot.state).unwrap();
    ui.input(
        &RawInputEvent::Paste("/help".into()),
        false,
        &mut Default::default(),
    );
    assert!(ui.form.is_none(), "paste alone must not execute commands");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 120, 40).contains("Keyboard shortcuts"));
    assert!(ui.locals[&room].text.text.is_empty());
    assert_eq!(ui.locals[&room].recipients, [agent].into());
    assert_eq!(ui.locals[&room].notes.text, "remember this");
    assert_eq!(serde_json::to_value(&ui.snapshot.state).unwrap(), before);
    assert!(ui.send_intent.is_none());
    assert!(ui
        .pending
        .iter()
        .all(|p| matches!(p.command, BusCommand::SetDraftText(..))));
    ui.input(
        &RawInputEvent::Paste("do not insert into draft".into()),
        false,
        &mut Default::default(),
    );
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn help_scrolls_on_short_terminals_without_sending() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert(" /help ");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let start = room_screen(&mut ui, 60, 12);
    assert!(start.contains("Keyboard shortcuts"));
    key(&mut ui, KeyCode::PageDown, KeyModifiers::NONE);
    assert_ne!(room_screen(&mut ui, 60, 12), start);
    key(&mut ui, KeyCode::End, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 60, 12).contains("Ctrl+Q"));
    key(&mut ui, KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(room_screen(&mut ui, 60, 12), start);
    assert!(ui.send_intent.is_none());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.form.is_none());
}

#[test]
fn help_only_intercepts_the_whole_composer_command() {
    for text in ["/help me with this", "quote: /help", "/helper"] {
        let (mut ui, room, agent) = fixture();
        ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
        ui.locals.get_mut(&room).unwrap().text.insert(text);
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ui.send_intent, Some(room));
        assert!(ui.form.is_none());
        assert_eq!(ui.locals[&room].text.text, text);
    }
    let (mut ui, room, _) = fixture();
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("/help".into()),
        false,
        &mut Default::default(),
    );
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].notes.text, "/help\n");
    assert!(ui.form.is_none());
}

#[test]
fn help_cancels_same_room_unsent_intent_before_it_can_become_an_agent_prompt() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("original prompt");
    ui.text_changed(room);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.send_intent, Some(room));
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("/help".into());
    ui.text_changed(room);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Keyboard shortcuts"));
    assert!(ui.send_intent.is_none());
    let mut snapshot = (*ui.snapshot).clone();
    for pending in &mut ui.pending {
        pending.result = Some(Ok(()));
        snapshot.last_command_id = pending.id;
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::Submit(_))));
    assert!(ui.locals[&room].text.text.is_empty());
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
fn agent_enter_adds_from_every_field_with_its_own_directory_and_escape_cancels() {
    for field in 0..4 {
        let (mut ui, room, _) = fixture();
        ui.form = Some(forms::Form::Agent {
            name: editor::Editor::new("frontend".into()),
            provider: Provider::ClaudeCode,
            cwd: editor::Editor::new("/projects/frontend".into()),
            args: editor::Editor::new("--model sonnet".into()),
            field,
        });
        // Suggestions must not hijack Enter; Tab remains path completion.
        ui.suggestions
            .entries
            .push(crate::bus::launch::PathSuggestion {
                path: "/projects/backend".into(),
                is_directory: true,
            });
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert!(ui
            .pending
            .iter()
            .any(|p| matches!(&p.command, BusCommand::AddAgent(input)
            if input.room == room && input.name == "frontend" && input.cwd == "/projects/frontend"
            && input.provider == Provider::ClaudeCode && input.extra_args == "--model sonnet")));
        let (mut ui, _, _) = fixture();
        ui.action(render::Action::NewAgent);
        key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
        assert!(ui.form.is_none());
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::AddAgent(_))));
    }
}

#[test]
fn room_creation_needs_only_name_and_does_not_inherit_agent_pwd_error() {
    let (mut ui, _, _) = fixture();
    ui.error = Some("Agent PWD must be an absolute path or ~/ path".into());
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = ui.error.clone();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.action(render::Action::NewRoom);
    ui.input(
        &RawInputEvent::Paste("frontend-and-backend".into()),
        false,
        &mut Default::default(),
    );
    assert!(!room_screen(&mut ui, 120, 40).contains("PWD"));
    // A new failure with identical text is still visible, as are unrelated
    // runtime failures; dismissing an old operation is not a blanket mute.
    ui.error = Some("Agent PWD must be an absolute path or ~/ path".into());
    assert!(room_screen(&mut ui, 120, 40).contains("PWD"));
    ui.error = None;
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = Some("Runtime connection unavailable".into());
    ui.receive_snapshot(Arc::new(snapshot));
    assert!(room_screen(&mut ui, 120, 40).contains("Runtime connection unavailable"));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.pending.iter().any(
        |p| matches!(&p.command, BusCommand::CreateRoom(name) if name == "frontend-and-backend")
    ));
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewRoom);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::CreateRoom(_))));
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
    key(&mut ui, KeyCode::Char('e'), KeyModifiers::CONTROL);
    room_screen(&mut ui, 100, 30);
    assert_eq!(composer_rect(&ui).height, 3);
    key(&mut ui, KeyCode::Char('e'), KeyModifiers::CONTROL);
    room_screen(&mut ui, 100, 30);
    assert_eq!(composer_rect(&ui), full);
    assert_eq!(ui.locals[&room].text.text, draft);
    assert_eq!(ui.locals[&room].recipients, [agent].into());
    assert!(ui.send_intent.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Add notes"));
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('@'), KeyModifiers::NONE);
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
fn composer_full_height_picker_covers_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"underlying-draft-suffix-should-not-bleed-through\n".repeat(80));
    ui.compute_view(100, 30);
    key(&mut ui, KeyCode::Char('@'), KeyModifiers::NONE);
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
fn composer_full_height_long_file_details_stay_readable() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"long draft\n".repeat(80));
    ui.detail_path = Some(format!("/tmp/{}/required-detail-tail.md", "a".repeat(70)));
    assert!(room_screen(&mut ui, 100, 30).contains("required-detail-tail.md"));
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
    key(&mut ui, KeyCode::Char('@'), KeyModifiers::NONE);
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
fn pending_focus_blocks_previous_pane_until_exact_native_target_arrives() {
    let (mut ui, _, agent) = fixture();
    ui.open_terminal(agent);
    assert!(!ui.terminal_ready(Some("old-pane")));
    ui.receive_event(BusEvent::TerminalFocused {
        agent,
        pane_id: "new-pane".into(),
    });
    assert!(!ui.terminal_ready(Some("old-pane")));
    assert!(ui.terminal_ready(Some("new-pane")));
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::LeaveRoom)));
}
#[test]
fn paste_and_composition_never_send_while_enter_and_newline_have_distinct_meanings() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.input(
        &RawInputEvent::Paste("hello\nworld".into()),
        false,
        &mut Default::default(),
    );
    ui.input(
        &RawInputEvent::Text(crate::input::TextCommit::new("文")),
        false,
        &mut Default::default(),
    );
    key(&mut ui, KeyCode::Enter, KeyModifiers::SHIFT);
    key(&mut ui, KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(ui.locals[&room].text.text, "hello\nworld文\n\n");
    assert!(ui.send_intent.is_none());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.send_intent, Some(room));
}
#[test]
fn path_drop_attaches_literal_multiple_paths_without_changing_draft_or_recipients() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("keep");
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.input(
        &RawInputEvent::Paste("'/one/a b.md' /two/@reviewer.md".into()),
        false,
        &mut Default::default(),
    );
    assert_eq!(ui.locals[&room].text.text, "keep");
    assert!(ui.locals[&room].recipients.contains(&agent));
    assert_eq!(
        ui.pending
            .iter()
            .filter(|p| matches!(p.command, BusCommand::AttachFile(..)))
            .count(),
        2
    );
    assert!(ui.send_intent.is_none());
}
#[test]
fn room_renders_without_native_snapshot_and_only_approved_chrome_and_collapsed_metadata() {
    let (mut ui, _, _) = fixture();
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
    assert_eq!(
        text.matches("# room").count(),
        2,
        "sidebar and room heading"
    );
    for required in ["ROOMS", "AGENTS", "# room", "author", "Codex", "Quote"] {
        if required != "Quote" {
            assert!(text.contains(required), "missing {required}");
        }
    }
    for absent in [
        "Workspaces",
        "Handoff",
        "Terminal",
        "/project",
        "main",
        "Back",
    ] {
        assert!(!text.contains(absent), "unexpected {absent}");
    }
    assert!(ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::NewRoom));
}
#[test]
fn stale_suggestions_cannot_replace_current_query_and_forms_leave_room() {
    let (mut ui, _, _) = fixture();
    key(&mut ui, KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert!(ui.form.is_some());
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::LeaveRoom)));
    ui.suggestions.query_id = 9;
    ui.receive_event(BusEvent::Suggestions {
        query_id: 8,
        result: Ok(vec![crate::bus::launch::PathSuggestion {
            path: "/stale".into(),
            is_directory: true,
        }]),
    });
    assert!(ui.suggestions.entries.is_empty());
}

#[test]
fn native_shell_composes_bus_before_any_server_frame_and_uses_same_resize_geometry() {
    let (ui, _, _) = fixture();
    let mut shell = crate::client::shell::ClientShellState::new(
        crate::client::shell::ClientShellConfig::from_config(&crate::config::Config::default()),
    );
    shell.bus = Some(ui);
    let frame = shell.compose(100, 30).unwrap();
    let text: String = frame.cells.iter().map(|c| c.symbol.as_str()).collect();
    assert!(text.contains("ROOMS"));
    assert!(text.contains("# room"));
    assert_eq!(shell.surface_size(100, 30).rows, 30);
    assert_eq!(shell.surface_size(100, 30).cols, 72);
}

#[test]
fn native_terminal_bytes_are_held_during_focus_then_forwarded_to_matching_pane() {
    let (mut ui, _, agent) = fixture();
    ui.open_terminal(agent);
    let mut shell = crate::client::shell::ClientShellState::new(
        crate::client::shell::ClientShellConfig::from_config(&crate::config::Config::default()),
    );
    shell.bus = Some(ui);
    shell.snapshot = Some(Box::new(crate::client::shell::tests::snapshot()));
    shell.pane_surface = Some(crate::client::shell::tests::surface());
    assert!(shell.handle_input_bytes(b"1\r").requests.is_empty());
    shell
        .bus
        .as_mut()
        .unwrap()
        .receive_event(BusEvent::TerminalFocused {
            agent,
            pane_id: "pane_1".into(),
        });
    shell.compose(100, 30);
    let outcome = shell.handle_input_bytes(b"1\r");
    assert!(outcome.requests.iter().any(|r|matches!(r,crate::protocol::ClientMessage::ClientShellPaneInput {pane_id,..} if pane_id=="pane_1")));
    let frame = shell.compose(100, 30).unwrap();
    assert_eq!(frame.cursor.as_ref().unwrap().x, 29);
    let text: String = frame.cells.iter().map(|c| c.symbol.as_str()).collect();
    assert!(text.contains("LIVE"));
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
fn quit_waits_for_draft_then_shutdown_ack_instead_of_exiting_immediately() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("last edit");
    ui.text_changed(room);
    let mut outcome = crate::client::shell::ClientShellInput::default();
    ui.input(
        &RawInputEvent::Key(TerminalKey::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
        false,
        &mut outcome,
    );
    assert!(!outcome.detach);
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::Shutdown)));
}

#[test]
fn ctrl_c_quits_bus_in_any_view_without_forwarding_or_discarding_pending_saves() {
    for view in ["room", "form", "terminal"] {
        let (mut ui, room, agent) = fixture();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("keep this draft");
        ui.text_changed(room);
        if view == "form" {
            ui.action(render::Action::NewRoom);
        } else if view == "terminal" {
            ui.open_terminal(agent);
            ui.receive_event(BusEvent::TerminalFocused {
                agent,
                pane_id: "pane_1".into(),
            });
        }
        let mut shell = crate::client::shell::ClientShellState::new(
            crate::client::shell::ClientShellConfig::from_config(&crate::config::Config::default()),
        );
        shell.bus = Some(ui);
        shell.snapshot = Some(Box::new(crate::client::shell::tests::snapshot()));
        shell.pane_surface = Some(crate::client::shell::tests::surface());
        shell.compose(100, 30);
        let outcome = shell.handle_input_bytes(b"\x03");
        let ui = shell.bus.as_mut().unwrap();
        assert!(ui.quitting.is_some(), "Ctrl+C must quit from {view}");
        assert!(
            !outcome.detach && !ui.exit_ready,
            "wait for save acknowledgements"
        );
        assert!(!outcome.requests.iter().any(|r| matches!(
            r,
            crate::protocol::ClientMessage::ClientShellPaneInput { .. }
        )));
        assert_eq!(ui.locals[&room].text.text, "keep this draft");
        assert!(ui.pending.iter().any(|p| matches!(&p.command, BusCommand::SetDraftText(id, text) if *id == room && text == "keep this draft")));
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::Shutdown)));
    }
}

#[test]
fn ctrl_c_release_and_modified_copy_chords_do_not_quit_bus() {
    let (mut ui, _, _) = fixture();
    let mut released = TerminalKey::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    released.kind = crossterm::event::KeyEventKind::Release;
    ui.input(
        &RawInputEvent::Key(released),
        false,
        &mut Default::default(),
    );
    for modifiers in [
        KeyModifiers::SUPER,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ] {
        key(&mut ui, KeyCode::Char('c'), modifiers);
    }
    assert!(ui.quitting.is_none());
    key(&mut ui, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(ui.quitting.is_some());
}

#[test]
fn keyboard_rename_is_available_for_terminal_and_escape_cancels() {
    let (mut ui, _, agent) = fixture();
    ui.open_terminal(agent);
    ui.receive_event(BusEvent::TerminalFocused {
        agent,
        pane_id: "pane".into(),
    });
    let mut outcome = crate::client::shell::ClientShellInput::default();
    ui.input(
        &RawInputEvent::Key(TerminalKey::new(KeyCode::F(2), KeyModifiers::NONE)),
        true,
        &mut outcome,
    );
    assert!(ui.rename.is_some());
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.rename.is_none());
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
fn failed_attachment_cannot_be_erased_by_different_successful_attachment() {
    let (mut ui, room, _) = fixture();
    ui.queue(
        BusCommand::AttachFile(room, "/missing".into()),
        Effect::Files(room),
    );
    ui.queue(
        BusCommand::AttachFile(room, "/existing".into()),
        Effect::Files(room),
    );
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 1,
        result: Err("missing".into()),
    });
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 2,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = 2;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert_eq!(ui.failed.len(), 1);
}

#[test]
fn add_agent_provider_selection_exposes_all_choices_as_mouse_targets() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Field(1));
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(
        text.contains("Claude Code") && text.contains("Cursor"),
        "{text}"
    );
}

#[test]
fn failed_file_is_removable_and_does_not_permanently_block_later_send() {
    let (mut ui, room, _) = fixture();
    ui.queue(
        BusCommand::AttachFile(room, "/missing.md".into()),
        Effect::Files(room),
    );
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 1,
        result: Err("missing".into()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    ui.compute_view(100, 30);
    assert!(ui.view.hits.iter().any(|hit|matches!(&hit.action,render::Action::RemoveFile(path) if path.to_str()==Some("/missing.md"))));
    ui.action(render::Action::RemoveFile("/missing.md".into()));
    assert!(ui.failed.is_empty());
}

#[test]
fn completed_shutdown_exits_only_after_its_exact_success_and_snapshot() {
    let (mut ui, _, _) = fixture();
    ui.request_quit();
    ui.settle();
    let id = ui.pending.front().unwrap().id;
    assert!(matches!(ui.pending[0].command, BusCommand::Shutdown));
    ui.receive_event(BusEvent::CommandFinished {
        command_id: id,
        result: Ok(()),
    });
    ui.settle();
    assert!(!ui.exit_ready);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = id;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(ui.exit_ready);
}

#[test]
fn updates_arriving_in_terminal_preserve_newer_local_room_edits_and_unread() {
    let (mut ui, room, agent) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("draft in room");
    ui.text_changed(room);
    ui.open_terminal(agent);
    let mut json = serde_json::to_value(&ui.snapshot.state).unwrap();
    json["rooms"][room.0.to_string()]["unread_count"] = serde_json::json!(2);
    json["agents"][agent.0.to_string()]["status"] = serde_json::json!("blocked");
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    assert_eq!(ui.snapshot.state.room(room).unwrap().unread_count, 2);
    assert_eq!(
        ui.snapshot.state.agent(agent).unwrap().status,
        RuntimeStatus::Blocked
    );
    assert_eq!(ui.locals[&room].text.text, "draft in room");
    assert_eq!(ui.terminal, Some(agent));
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
fn hook_consent_clears_old_pwd_suggestions_before_enter_can_confirm() {
    let (mut ui, room, _) = fixture();
    ui.suggestions.query_id = 9;
    ui.suggestions
        .entries
        .push(crate::bus::launch::PathSuggestion {
            path: "/old".into(),
            is_directory: true,
        });
    ui.receive_event(BusEvent::SetupRequired {
        input: crate::bus::launch::AddAgent {
            room,
            name: "fixture".into(),
            provider: Provider::Codex,
            cwd: "/project".into(),
            extra_args: String::new(),
            consent_project_hooks: false,
        },
        notice: crate::bus::launch::SetupNotice {
            path: "/project/.codex/hooks.json".into(),
            message: "review hooks".into(),
        },
    });
    assert!(ui.suggestions.entries.is_empty());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(&p.command,BusCommand::AddAgent(input) if input.consent_project_hooks)));
}

fn mouse(ui: &mut BusUi, kind: crossterm::event::MouseEventKind, column: u16, row: u16) {
    ui.input(
        &RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }),
        false,
        &mut Default::default(),
    );
}

#[test]
fn overflow_rooms_remain_reachable_after_early_selection_and_restart() {
    use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut rooms = vec![first];
    for index in 2..=40 {
        rooms.push(snapshot.state.create_room(&format!("room{index}")).unwrap());
    }
    ui.receive_snapshot(Arc::new(snapshot));
    // Every room remains reachable with a single whole-sidebar scroll offset.
    ui.open_room(rooms[6]);
    ui.open_room(first);
    ui.compute_view(100, 30);
    mouse(&mut ui, ScrollDown, 2, 3);
    mouse(&mut ui, ScrollDown, 2, 3);
    ui.compute_view(100, 30);
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::Room(first)));
    mouse(&mut ui, ScrollUp, 2, 3);
    mouse(&mut ui, ScrollUp, 2, 3);
    for restarted in [false, true] {
        if restarted {
            ui = BusUi::new(ui.snapshot.clone());
        }
        for &wanted in &rooms {
            for _ in 0..rooms.len() {
                ui.compute_view(100, 30);
                if ui
                    .view
                    .hits
                    .iter()
                    .any(|h| h.action == render::Action::Room(wanted))
                {
                    break;
                }
                mouse(&mut ui, ScrollDown, 2, 3);
            }
            let hit = ui
                .view
                .hits
                .iter()
                .find(|h| h.action == render::Action::Room(wanted))
                .expect("every room must remain reachable independently of selected room")
                .clone();
            mouse(
                &mut ui,
                crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
                hit.rect.x,
                hit.rect.y,
            );
            assert_eq!(ui.room, Some(wanted));
            for _ in 0..rooms.len() {
                mouse(&mut ui, ScrollUp, 2, 3);
            }
        }
        assert_eq!(
            ui.sidebar_scroll, 0,
            "scrolling up restores the whole column"
        );
    }
}

#[test]
fn overflow_mixed_file_chips_keep_full_path_inspection_and_removal() {
    use crossterm::event::MouseEventKind::{Moved, ScrollDown, ScrollUp};
    let (mut ui, room, _) = fixture();
    let paths: Vec<std::path::PathBuf> = [
        format!("/one/{}.md", "very-long-name-".repeat(10)),
        "/two/ordinary-second-attachment.md".into(),
        "/three/ordinary-third-attachment.md".into(),
        "/missing/failed-overflow-attachment.md".into(),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let mut snapshot = (*ui.snapshot).clone();
    for path in &paths[..3] {
        snapshot.state.attach_file(room, path.clone()).unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.failed.push(Pending {
        id: 50,
        command: BusCommand::AttachFile(room, paths[3].display().to_string()),
        effect: Effect::Files(room),
        enqueued: true,
        result: Some(Err("missing".into())),
    });
    ui.compute_view(100, 30);
    let next = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::ScrollFiles(true))
        .unwrap()
        .clone();
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        next.rect.x,
        next.rect.y,
    );
    ui.compute_view(100, 30);
    assert!(ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::RemoveFile(paths[1].clone())));
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::RemoveFile(paths[0].clone())));
    let previous = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::ScrollFiles(false))
        .unwrap()
        .clone();
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        previous.rect.x,
        previous.rect.y,
    );
    for path in &paths {
        for _ in 0..paths.len() {
            ui.compute_view(100, 30);
            if ui
                .view
                .hits
                .iter()
                .any(|h| h.action == render::Action::RemoveFile(path.clone()))
            {
                break;
            }
            let files = ui.view.files;
            mouse(&mut ui, ScrollDown, files.x + 1, files.y);
        }
        let hit = ui
            .view
            .hits
            .iter()
            .find(|h| h.action == render::Action::RemoveFile(path.clone()))
            .expect("every successful and failed attachment must have a reachable removal target")
            .clone();
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let chip: String = (hit.rect.x..hit.rect.right())
            .map(|x| buffer[(x, hit.rect.y)].symbol())
            .collect();
        assert!(
            chip.contains(if path == &paths[3] { "! ×]" } else { "×]" }),
            "clipped chip retains error/removal suffix: {chip}"
        );
        mouse(&mut ui, Moved, hit.rect.x, hit.rect.y);
        assert_eq!(ui.detail_path.as_deref(), path.to_str());
        mouse(
            &mut ui,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            hit.rect.x,
            hit.rect.y,
        );
        if path == &paths[3] {
            assert!(ui.failed.is_empty());
        } else {
            assert!(ui.pending.iter().any(|p| matches!(&p.command, BusCommand::RemoveFile(id, removed) if *id == room && removed == path)));
        }
        for _ in 0..paths.len() {
            let files = ui.view.files;
            mouse(&mut ui, ScrollUp, files.x + 1, files.y);
        }
    }
    assert_eq!(ui.main_scroll, 0, "file scrolling must not move replies");
}
