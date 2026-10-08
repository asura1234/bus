use super::*;

#[test]
fn native_shell_composes_bus_before_any_server_frame_and_uses_same_resize_geometry() {
    let (ui, _, _) = fixture();
    let mut shell = crate::client::compositor::ClientShellState::new(
        crate::client::compositor::ClientShellConfig::from_config(&crate::config::Config::default()),
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
    let mut shell = crate::client::compositor::ClientShellState::new(
        crate::client::compositor::ClientShellConfig::from_config(&crate::config::Config::default()),
    );
    shell.bus = Some(ui);
    shell.snapshot = Some(Box::new(crate::client::compositor::tests::snapshot()));
    shell.pane_surface = Some(crate::client::compositor::tests::surface());
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
fn working_agent_projection_updates_hold_native_terminal_instead_of_flashing_placeholder() {
    use crate::client::compositor::tests::{snapshot, surface};
    let (mut ui, _, agent) = fixture();
    ui.open_terminal(agent);
    ui.receive_event(BusEvent::TerminalFocused {
        agent,
        pane_id: "pane_1".into(),
    });
    let mut shell = crate::client::compositor::ClientShellState::new(
        crate::client::compositor::ClientShellConfig::from_config(&crate::config::Config::default()),
    );
    shell.bus = Some(ui);
    shell.apply_active_snapshot(Box::new(snapshot()));
    shell.set_pane_surface(surface());
    let screen = |shell: &mut crate::client::compositor::ClientShellState| {
        shell.compose(100, 30).map(|frame| {
            frame
                .cells
                .iter()
                .map(|c| c.symbol.as_str())
                .collect::<String>()
        })
    };
    let at_revision = |revision| {
        let mut snapshot = snapshot();
        snapshot.revision = revision;
        Box::new(snapshot)
    };
    let surface_at = |revision| {
        let mut surface = surface();
        surface.projection_revision = revision;
        surface
    };
    assert!(screen(&mut shell).is_some_and(|text| text.contains("LIVE")));

    // Spinner titles advance the projection several times a second; its
    // surface follows separately. The presented terminal must stay on screen.
    shell.apply_active_snapshot(at_revision(2));
    assert_eq!(screen(&mut shell), None, "hold the last terminal frame");
    let outcome = shell.handle_input_bytes(b"x");
    assert!(
        outcome.requests.iter().any(|r| matches!(
            r,
            crate::protocol::ClientMessage::ClientShellPaneInput { pane_id, .. }
                if pane_id == "pane_1"
        )),
        "typing must not be swallowed while the pair catches up"
    );
    shell.set_pane_surface(surface_at(2));
    assert!(screen(&mut shell).is_some_and(|text| text.contains("LIVE")));

    // The successor surface may also arrive before its snapshot.
    shell.set_pane_surface(surface_at(3));
    assert_eq!(screen(&mut shell), None, "hold the last terminal frame");
    shell.apply_active_snapshot(at_revision(3));
    assert!(screen(&mut shell).is_some_and(|text| text.contains("LIVE")));
}

#[test]
fn ctrl_c_in_focused_agent_terminal_is_forwarded_and_does_not_quit_bus() {
    let (mut ui, room, agent) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("keep this draft");
    ui.text_changed(room);
    ui.open_terminal(agent);
    ui.receive_event(BusEvent::TerminalFocused {
        agent,
        pane_id: "pane_1".into(),
    });
    let mut shell = shell_with(ui);
    for _ in 0..2 {
        let outcome = shell.handle_input_bytes(b"\x03");
        assert!(!outcome.detach);
        assert!(
            forwards_ctrl_c_to(&outcome, "pane_1"),
            "Ctrl+C must reach the agent unchanged"
        );
        let ui = shell.bus.as_ref().unwrap();
        assert!(ui.quitting.is_none(), "Ctrl+C must not quit Bus");
        assert!(ui.terminal.is_some(), "the agent terminal stays open");
        assert_eq!(ui.locals[&room].text.text, "keep this draft");
    }
}

#[test]
fn ctrl_c_in_room_clears_the_draft_and_never_quits_bus() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("clear me");
    ui.text_changed(room);
    let mut shell = shell_with(ui);
    // The first press clears the draft; an empty composer then ignores it.
    for expected in ["", ""] {
        let outcome = shell.handle_input_bytes(b"\x03");
        assert!(!outcome.detach);
        assert!(!outcome.requests.iter().any(|r| matches!(
            r,
            crate::protocol::ClientMessage::ClientShellPaneInput { .. }
        )));
        let ui = shell.bus.as_ref().unwrap();
        assert!(ui.quitting.is_none(), "Ctrl+C must not quit Bus");
        assert_eq!(ui.locals[&room].text.text, expected);
    }
    let ui = shell.bus.as_ref().unwrap();
    assert!(ui.pending.iter().any(|p| matches!(&p.command, BusCommand::SetDraftText(id, text) if *id == room && text.is_empty())));
}

#[test]
fn ctrl_c_in_forms_and_unfocused_terminals_does_nothing() {
    for view in ["form", "rename", "terminal_not_ready"] {
        let (mut ui, room, agent) = fixture();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("keep this draft");
        ui.text_changed(room);
        match view {
            "form" => ui.action(render::Action::NewRoom),
            "rename" => key(&mut ui, KeyCode::F(2), KeyModifiers::NONE),
            _ => ui.open_terminal(agent),
        }
        assert!(ui.form.is_some() || ui.rename.is_some() || ui.terminal.is_some());
        assert!(view != "rename" || ui.rename.is_some());
        let mut shell = shell_with(ui);
        let outcome = shell.handle_input_bytes(b"\x03");
        assert!(!outcome.detach);
        assert!(!forwards_ctrl_c_to(&outcome, "pane_1"), "{view}");
        let ui = shell.bus.as_ref().unwrap();
        assert!(
            ui.quitting.is_none(),
            "Ctrl+C must not quit Bus from {view}"
        );
        assert_eq!(ui.locals[&room].text.text, "keep this draft");
    }
}

#[test]
fn ctrl_q_still_quits_from_room_and_agent_terminal_after_saving() {
    for view in ["room", "terminal"] {
        let (mut ui, room, agent) = fixture();
        ui.locals
            .get_mut(&room)
            .unwrap()
            .text
            .insert("keep this draft");
        ui.text_changed(room);
        if view == "terminal" {
            ui.open_terminal(agent);
            ui.receive_event(BusEvent::TerminalFocused {
                agent,
                pane_id: "pane_1".into(),
            });
        }
        let mut shell = shell_with(ui);
        let outcome = shell.handle_input_bytes(b"\x11");
        let ui = shell.bus.as_ref().unwrap();
        assert!(ui.quitting.is_some(), "Ctrl+Q must quit from {view}");
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
    }
}

#[test]
fn ctrl_q_exits_the_client_only_after_bus_saved() {
    let (ui, _, _) = fixture();
    let mut shell = shell_with(ui);
    let outcome = shell.handle_input_bytes(b"\x11");
    assert!(!outcome.detach, "Ctrl+Q must wait for the save");
    assert!(!shell.bus_exit_ready());
    let ui = shell.bus.as_mut().unwrap();
    assert!(ui.quitting.is_some(), "Ctrl+Q must save and quit");
    // Stands in for the Shutdown acknowledgement after every save landed; the
    // client then detaches and stops the server (see shell_runtime tests).
    ui.exit_ready = true;
    assert!(shell.bus_exit_ready());
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
        KeyModifiers::CONTROL,
    ] {
        key(&mut ui, KeyCode::Char('c'), modifiers);
    }
    assert!(ui.quitting.is_none());
    key(&mut ui, KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert!(ui.quitting.is_some());
}

#[test]
fn ctrl_c_keeps_a_draft_that_is_already_being_sent_and_stays_open() {
    let (mut ui, room, agent) = fixture();
    let local = ui.locals.get_mut(&room).unwrap();
    local.recipients.insert(agent);
    local.text.insert("on its way");
    ui.text_changed(room);
    ui.request_send(room);
    key(&mut ui, KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(ui.quitting.is_none());
    assert_eq!(ui.locals[&room].text.text, "on its way");
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(&p.command, BusCommand::SetDraftText(_, text) if text.is_empty())));
}

#[test]
fn room_not_ready_state_never_requires_a_second_bus_confirmation() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut value = serde_json::to_value(&snapshot.state).unwrap();
    value["agents"][agent.0.to_string()]["hook_setup_confirmed"] = serde_json::json!(false);
    snapshot.state = serde_json::from_value(value).unwrap();
    snapshot
        .state
        .set_agent_error(
            agent,
            Some("Not ready; waiting for the provider terminal.".into()),
        )
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().text.insert("keep draft");
    ui.compute_view(120, 40);

    key(&mut ui, KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert!(ui.form.is_none());
    assert!(!ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::CompleteHookSetup(_))));
    assert_eq!(ui.locals[&room].text.text, "keep draft");
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
fn quit_waits_for_draft_then_shutdown_ack_instead_of_exiting_immediately() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("last edit");
    ui.text_changed(room);
    let mut outcome = crate::client::compositor::ClientShellInput::default();
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
fn option_delete_and_option_arrows_edit_by_word_from_terminal_bytes() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("keep these words");
    let mut shell = crate::client::compositor::ClientShellState::new(
        crate::client::compositor::ClientShellConfig::from_config(&crate::config::Config::default()),
    );
    shell.bus = Some(ui);
    shell.snapshot = Some(Box::new(crate::client::compositor::tests::snapshot()));
    shell.pane_surface = Some(crate::client::compositor::tests::surface());
    shell.compose(100, 30);
    let draft = |shell: &crate::client::compositor::ClientShellState| {
        let local = &shell.bus.as_ref().unwrap().locals[&room];
        (local.text.text.clone(), local.text.cursor)
    };
    for (bytes, text, cursor) in [
        (b"\x1b\x7f".as_slice(), "keep these ", 11),
        (b"\x1b[127;3u", "keep ", 5),
        (b"\x1bb", "keep ", 0),
        (b"\x1b[1;3C", "keep ", 4),
        (b"\x1b[1;3D", "keep ", 0),
    ] {
        shell.handle_input_bytes(bytes);
        assert_eq!(draft(&shell), (text.to_owned(), cursor), "{bytes:?}");
    }
    assert!(shell.bus.as_ref().unwrap().quitting.is_none());
}

#[test]
fn keyboard_rename_is_available_for_terminal_and_escape_cancels() {
    let (mut ui, _, agent) = fixture();
    ui.open_terminal(agent);
    ui.receive_event(BusEvent::TerminalFocused {
        agent,
        pane_id: "pane".into(),
    });
    let mut outcome = crate::client::compositor::ClientShellInput::default();
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
        orchestrator: None,
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
