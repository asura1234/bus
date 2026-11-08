#[test]
fn creation_events_survive_snapshots_from_before_the_created_target() {
    let (mut ui, old_room, _) = fixture();
    let mut created = (*ui.snapshot).clone();
    let room = created.state.create_room("new room").unwrap();
    let mut intermediate = (*ui.snapshot).clone();
    intermediate.revision += 1;
    ui.receive_event(BusEvent::RoomCreated {
        room,
        command_id: 0,
    });
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
    ui.receive_event(BusEvent::AgentAdded {
        agent,
        command_id: 0,
    });
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

fn submit_room_form(ui: &mut BusUi, name: &str) {
    ui.action(render::Action::NewRoom);
    for c in name.chars() {
        key(ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(ui, KeyCode::Enter, KeyModifiers::NONE);
}

fn submit_agent_form(ui: &mut BusUi) {
    ui.action(render::Action::NewAgent);
    let Some(forms::Form::Agent {
        name,
        provider,
        cwd,
        ..
    }) = &mut ui.form
    else {
        panic!("agent form was not opened");
    };
    name.text = "first-agent".into();
    *provider = Some(Provider::ClaudeCode);
    cwd.text = "/projects/first".into();
    key(ui, KeyCode::Enter, KeyModifiers::NONE);
}

fn open_edited_file_form(ui: &mut BusUi) {
    ui.action(render::Action::Files);
    let Some(forms::Form::Files(editor)) = &mut ui.form else {
        panic!("file form was not opened");
    };
    editor.text = "newer-input.txt".into();
}

fn assert_edited_file_form_survives(ui: &BusUi, room: RoomId) {
    assert_eq!(ui.room, Some(room));
    assert!(ui.terminal.is_none());
    let Some(forms::Form::Files(editor)) = &ui.form else {
        panic!("newer file form was discarded");
    };
    assert_eq!(editor.text, "newer-input.txt");
}

fn settle_creation_command(ui: &mut BusUi) {
    let ids: Vec<_> = ui.pending.iter().map(|pending| pending.id).collect();
    assert!(ids.iter().any(|id| {
        ui.pending.iter().any(|pending| {
            pending.id == *id
                && matches!(
                    pending.command,
                    BusCommand::CreateRoom(_)
                        | BusCommand::AddAgent(_)
                        | BusCommand::AddOrchestrator(..)
                )
        })
    }));
    for id in &ids {
        if let Some(pending) = ui.pending.iter_mut().find(|pending| pending.id == *id) {
            pending.enqueued = true;
        }
        ui.receive_event(BusEvent::CommandFinished {
            command_id: *id,
            result: Ok(()),
        });
    }
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = *ids.last().expect("commands are pending");
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(!ui.pending.iter().any(|pending| matches!(
        pending.command,
        BusCommand::CreateRoom(_) | BusCommand::AddAgent(_) | BusCommand::AddOrchestrator(..)
    )));
}

#[test]
fn room_created_ack_after_settlement_completes_its_origin_form() {
    let (mut ui, _, _) = fixture();
    submit_room_form(&mut ui, "created-room");
    settle_creation_command(&mut ui);
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    let mut snapshot = (*ui.snapshot).clone();
    let created = snapshot.state.create_room("created-room").unwrap();

    ui.receive_event(BusEvent::RoomCreated {
        room: created,
        command_id,
    });

    assert!(ui.form.is_none());
    assert_eq!(ui.room, Some(created));
}

#[test]
fn room_created_ack_after_dismiss_stays_in_the_selected_room() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let selected = snapshot.state.create_room("selected").unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    submit_room_form(&mut ui, "created-room");
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    settle_creation_command(&mut ui);
    ui.action(render::Action::Room(selected));
    assert!(ui.form.is_none());
    assert_eq!(ui.room, Some(selected));
    assert_ne!(ui.room, Some(first));

    let mut snapshot = (*ui.snapshot).clone();
    let created = snapshot.state.create_room("created-room").unwrap();
    ui.receive_event(BusEvent::RoomCreated {
        room: created,
        command_id,
    });

    assert_eq!(
        ui.room,
        Some(selected),
        "a stale acknowledgement left the room selected before it arrived"
    );
    assert!(
        !ui.pending.iter().any(|pending| matches!(
            pending.command,
            BusCommand::SelectRoom(id) if id == created
        )),
        "a stale acknowledgement selected the created room"
    );
}

#[test]
fn agent_added_ack_after_esc_does_not_open_the_created_terminal() {
    let (mut ui, room, _) = fixture();
    submit_agent_form(&mut ui);
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    settle_creation_command(&mut ui);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert_eq!(ui.room, Some(room));
    assert!(ui.terminal.is_none());

    let mut snapshot = (*ui.snapshot).clone();
    let added = snapshot
        .state
        .create_agent(
            room,
            "first-agent",
            Provider::ClaudeCode,
            "/projects/first".into(),
            None,
        )
        .unwrap();
    ui.receive_event(BusEvent::AgentAdded {
        agent: added,
        command_id,
    });

    assert_eq!(ui.room, Some(room));
    assert!(
        ui.terminal.is_none(),
        "a stale acknowledgement opened the created terminal after Esc"
    );
    assert!(
        !ui.pending.iter().any(|pending| matches!(
            pending.command,
            BusCommand::FocusTerminal(id) if id == added
        )),
        "a stale acknowledgement focused the created terminal"
    );
}

#[test]
fn agent_added_ack_after_settlement_completes_its_origin_form() {
    let (mut ui, room, _) = fixture();
    submit_agent_form(&mut ui);
    settle_creation_command(&mut ui);
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    let mut snapshot = (*ui.snapshot).clone();
    let added = snapshot
        .state
        .create_agent(
            room,
            "first-agent",
            Provider::ClaudeCode,
            "/projects/first".into(),
            None,
        )
        .unwrap();

    ui.receive_event(BusEvent::AgentAdded {
        agent: added,
        command_id,
    });

    assert!(ui.form.is_none());
    assert_eq!(ui.terminal, Some(added));
}

#[test]
fn room_created_ack_preserves_a_newer_non_dev_form_and_its_room_context() {
    let (mut ui, room, _) = fixture();
    submit_room_form(&mut ui, "created-room");
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::CreateRoom(_))));
    ui.action(render::Action::Cancel);
    open_edited_file_form(&mut ui);

    let mut snapshot = (*ui.snapshot).clone();
    let created = snapshot.state.create_room("created-room").unwrap();
    ui.receive_event(BusEvent::RoomCreated {
        room: created,
        command_id,
    });

    assert_edited_file_form_survives(&ui, room);
    assert!(!ui.pending.iter().any(|pending| matches!(
        pending.command,
        BusCommand::SelectRoom(id) if id == created
    )));
}

#[test]
fn room_created_ack_completes_its_still_current_form() {
    let (mut ui, _, _) = fixture();
    submit_room_form(&mut ui, "created-room");
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    let mut snapshot = (*ui.snapshot).clone();
    let created = snapshot.state.create_room("created-room").unwrap();

    ui.receive_event(BusEvent::RoomCreated {
        room: created,
        command_id,
    });

    assert!(ui.form.is_none());
    assert_eq!(ui.room, Some(created));
}

#[test]
fn agent_added_ack_preserves_a_newer_non_dev_form_and_its_room_context() {
    let (mut ui, room, _) = fixture();
    submit_agent_form(&mut ui);
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::AddAgent(_))));
    ui.action(render::Action::Cancel);
    open_edited_file_form(&mut ui);

    let mut snapshot = (*ui.snapshot).clone();
    let added = snapshot
        .state
        .create_agent(
            room,
            "first-agent",
            Provider::ClaudeCode,
            "/projects/first".into(),
            None,
        )
        .unwrap();
    ui.receive_event(BusEvent::AgentAdded {
        agent: added,
        command_id,
    });

    assert_edited_file_form_survives(&ui, room);
    assert!(!ui.pending.iter().any(|pending| matches!(
        pending.command,
        BusCommand::FocusTerminal(id) if id == added
    )));
}

#[test]
fn agent_added_ack_completes_its_still_current_form() {
    let (mut ui, room, _) = fixture();
    submit_agent_form(&mut ui);
    let command_id = ui.creation_origins.back().expect("creation origin").id;
    let mut snapshot = (*ui.snapshot).clone();
    let added = snapshot
        .state
        .create_agent(
            room,
            "first-agent",
            Provider::ClaudeCode,
            "/projects/first".into(),
            None,
        )
        .unwrap();

    ui.receive_event(BusEvent::AgentAdded {
        agent: added,
        command_id,
    });

    assert!(ui.form.is_none());
    assert_eq!(ui.terminal, Some(added));
}

#[test]
fn external_creation_ack_preserves_a_newer_form_and_rename() {
    let (mut ui, room, _) = fixture();
    open_edited_file_form(&mut ui);
    let mut snapshot = (*ui.snapshot).clone();
    let created = snapshot.state.create_room("external-room").unwrap();
    let added = snapshot
        .state
        .create_agent(
            room,
            "external-agent",
            Provider::ClaudeCode,
            "/projects/external".into(),
            None,
        )
        .unwrap();

    ui.receive_event(BusEvent::RoomCreated {
        room: created,
        command_id: 0,
    });
    ui.receive_event(BusEvent::AgentAdded {
        agent: added,
        command_id: 0,
    });

    assert_edited_file_form_survives(&ui, room);

    let (mut ui, room, _) = fixture();
    ui.rename = Some(forms::Rename {
        target: forms::RenameTarget::Room(room),
        editor: editor::Editor::new("newer-room-name".into()),
    });
    ui.receive_event(BusEvent::RoomCreated {
        room: created,
        command_id: 0,
    });
    assert_eq!(ui.room, Some(room));
    assert_eq!(
        ui.rename.as_ref().map(|rename| rename.editor.text.as_str()),
        Some("newer-room-name")
    );
}

#[test]
fn older_room_created_ack_preserves_a_newer_room_form_awaiting_its_own_ack() {
    let (mut ui, room, _) = fixture();
    submit_room_form(&mut ui, "first-created");
    let first_command_id = ui.creation_origins.back().expect("first origin").id;
    settle_creation_command(&mut ui);
    let mut snapshot = (*ui.snapshot).clone();
    let first_created = snapshot.state.create_room("first-created").unwrap();

    // The first command has settled but its RoomCreated acknowledgement is still
    // outstanding (the dev acknowledgement hold produces exactly this order).
    ui.action(render::Action::Cancel);
    submit_room_form(&mut ui, "second-created");
    assert!(matches!(ui.form, Some(forms::Form::Room(_))));
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::CreateRoom(_))));

    ui.receive_event(BusEvent::RoomCreated {
        room: first_created,
        command_id: first_command_id,
    });

    assert!(
        matches!(ui.form, Some(forms::Form::Room(_))),
        "the first command's acknowledgement closed the newer room form"
    );
    assert_eq!(
        ui.room,
        Some(room),
        "the first command's acknowledgement moved the visible room"
    );
}

#[test]
fn ctrl_chords_open_the_agent_and_file_pickers() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("keep draft");
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(ui.recipient_menu && ui.form.is_none());
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("keep draft");
    key(&mut ui, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert!(matches!(ui.form, Some(forms::Form::Files(_))));
    assert_eq!(ui.locals[&room].text.text, "keep draft");
}

#[test]
fn settings_space_does_not_toggle_color_blind_mode() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);

    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);

    assert!(!ui.settings.color_blind_mode);
}

#[test]
fn settings_compaction_limit_is_visible_and_editable_with_arrow_keys() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("Max compactions per agent"), "{screen}");
    assert!(screen.contains("‹ 5 ›"), "{screen}");
    for _ in 0..ui.compaction_field() {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    assert_eq!(ui.settings_field, ui.compaction_field());
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(
        serde_json::to_value(&ui.settings).unwrap()["max_compactions_per_agent"],
        6
    );
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::SetMaxCompactionsPerAgent(6))));
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(
        serde_json::to_value(&ui.settings).unwrap()["max_compactions_per_agent"],
        5
    );
}

#[test]
fn settings_compaction_limit_mouse_controls_respect_the_allowed_range() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 120, 40);
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::AdjustCompactionLimit(true)));
    ui.settings.max_compactions_per_agent = 100;
    let before = ui.pending.len();
    ui.action(render::Action::AdjustCompactionLimit(true));
    assert_eq!(ui.settings.max_compactions_per_agent, 100);
    assert_eq!(ui.pending.len(), before);
    ui.settings.max_compactions_per_agent = 1;
    ui.action(render::Action::AdjustCompactionLimit(false));
    assert_eq!(ui.settings.max_compactions_per_agent, 1);
    assert_eq!(ui.pending.len(), before);
}

#[test]
fn settings_idle_agents_alert_is_visible_and_editable_with_arrow_keys() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("Idle agents alert (1–10)"), "{screen}");
    assert!(screen.contains("‹ 5 ›"), "{screen}");
    for _ in 0..ui.idle_agents_field() {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    assert_eq!(ui.settings_field, ui.idle_agents_field());
    assert_eq!(ui.idle_agents_field(), ui.compaction_field() + 1);
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(ui.settings.idle_agents_alert, 6);
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::SetIdleAgentsAlert(6))));
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(ui.settings.idle_agents_alert, 5);
}

#[test]
fn settings_idle_agents_alert_mouse_controls_respect_the_allowed_range() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 120, 40);
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::AdjustIdleAgentsAlert(true)));
    ui.settings.idle_agents_alert = 10;
    let before = ui.pending.len();
    ui.action(render::Action::AdjustIdleAgentsAlert(true));
    assert_eq!(ui.settings.idle_agents_alert, 10);
    assert_eq!(ui.pending.len(), before);
    ui.settings.idle_agents_alert = 1;
    ui.action(render::Action::AdjustIdleAgentsAlert(false));
    assert_eq!(ui.settings.idle_agents_alert, 1);
    assert_eq!(ui.pending.len(), before);
    ui.action(render::Action::AdjustIdleAgentsAlert(true));
    assert_eq!(ui.settings.idle_agents_alert, 2);
    assert_eq!(ui.pending.len(), before + 1);
}

#[test]
fn settings_sidebar_width_is_visible_and_editable_with_arrow_keys() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("SIDEBAR"), "{screen}");
    assert!(screen.contains("Sidebar width (20–60)"), "{screen}");
    assert!(screen.contains("‹ 28 ›"), "{screen}");
    for _ in 0..ui.sidebar_width_field() {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    assert_eq!(ui.settings_field, ui.sidebar_width_field());
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(ui.settings.sidebar_width, 29);
    assert!(ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::SetSidebarWidth(29))));
    // The sidebar itself is already wider on the next frame.
    room_screen(&mut ui, 120, 40);
    assert_eq!(ui.view.sidebar.width, 29);
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(ui.settings.sidebar_width, 28);
    assert!(room_screen(&mut ui, 120, 40).contains("‹ 28 ›"));
}

#[test]
fn quick_stepper_presses_survive_the_echo_of_an_earlier_press() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 120, 40);
    for _ in 0..ui.sidebar_width_field() {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    assert_eq!(ui.settings_field, ui.sidebar_width_field());
    // Three presses before the coordinator answers the first.
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert_eq!(ui.settings.sidebar_width, 31);
    let echo = |width| {
        BusEvent::SettingsChanged(crate::messaging::prefs::settings::BusSettings {
            sidebar_width: width,
            color_blind_mode: true,
            ..Default::default()
        })
    };
    ui.receive_event(echo(29));
    assert_eq!(
        ui.settings.sidebar_width, 31,
        "the first echo must not undo later presses"
    );
    assert!(ui.settings.color_blind_mode, "other fields still arrive");
    assert!(room_screen(&mut ui, 120, 40).contains("‹ 31 ›"));
    // Once every press is answered, the echo is the truth again.
    for pending in ui.pending.iter_mut() {
        pending.result = Some(Ok(()));
    }
    ui.receive_event(echo(31));
    assert_eq!(ui.settings.sidebar_width, 31);
    ui.receive_event(echo(40));
    assert_eq!(ui.settings.sidebar_width, 40);
}

#[test]
fn settings_sidebar_width_mouse_controls_respect_the_allowed_range() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 120, 40);
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::AdjustSidebarWidth(true)));
    ui.settings.sidebar_width = 60;
    let before = ui.pending.len();
    ui.action(render::Action::AdjustSidebarWidth(true));
    assert_eq!(ui.settings.sidebar_width, 60);
    assert_eq!(ui.pending.len(), before);
    ui.settings.sidebar_width = 20;
    ui.action(render::Action::AdjustSidebarWidth(false));
    assert_eq!(ui.settings.sidebar_width, 20);
    assert_eq!(ui.pending.len(), before);
    ui.action(render::Action::AdjustSidebarWidth(true));
    assert_eq!(ui.settings.sidebar_width, 21);
    assert_eq!(ui.pending.len(), before + 1);
}

#[test]
fn settings_footer_advertises_enter_as_the_action_key() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);

    let screen = room_screen(&mut ui, 100, 30);

    assert!(screen.contains("Enter changes"), "{screen}");
    assert!(!screen.contains("Space"), "{screen}");
}

#[test]
fn help_enter_is_local_and_preserves_room_context() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
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
    assert_eq!(serde_json::to_value(&ui.snapshot.state).unwrap(), before);
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
        assert_eq!(ui.submissions.len(), 1);
        assert!(ui.form.is_none());
        assert_eq!(ui.locals[&room].text.text, "");
        assert_eq!(ui.submissions[0].draft.text, text);
    }
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
    assert_eq!(ui.submissions.len(), 1);
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("/help".into());
    ui.text_changed(room);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Keyboard shortcuts"));
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
        .any(|p| matches!(p.command, BusCommand::Submit(..))));
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn agent_enter_adds_from_non_model_fields_with_its_own_directory_and_escape_cancels() {
    for field in [0, 3] {
        let (mut ui, room, _) = fixture();
        ui.form = Some(forms::Form::Agent {
            name: editor::Editor::new("frontend".into()),
            provider: Some(Provider::ClaudeCode),
            provider_cursor: Provider::ClaudeCode,
            cwd: editor::Editor::new("/projects/frontend".into()),
            args: Box::new(editor::Editor::new("--model sonnet".into())),
            field,
            orchestrates: None,
            prompt: None,
        });
        // Only the PWD field owns suggestions; a stale entry elsewhere must not hijack Enter.
        ui.suggestions
            .entries
            .push(crate::agents::providers::suggest::PathSuggestion {
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
fn new_agent_starts_without_a_selected_model() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);

    assert!(room_screen(&mut ui, 100, 30).contains("Choose model"));
}

#[test]
fn add_agent_form_keeps_model_and_pwd_labels_concise() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::ClaudeCode));

    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("< Claude Code >"), "{screen}");
    assert!(screen.contains("PWD"), "{screen}");
    assert!(!screen.contains("arrows /"), "{screen}");
    assert!(!screen.contains("(this agent only)"), "{screen}");
}

#[test]
fn model_menu_enter_selects_without_submitting_or_showing_an_error() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Field(1));

    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);

    assert!(matches!(ui.form, Some(forms::Form::Agent { field: 2, .. })));
    assert!(ui
        .pending
        .iter()
        .all(|pending| !matches!(pending.command, BusCommand::AddAgent(_))));
    assert!(ui.error.is_none());
    assert!(room_screen(&mut ui, 100, 30).contains("Claude Code"));
}

#[test]
fn add_agent_rejects_each_missing_required_field_and_keeps_the_form_open() {
    for missing in ["Name", "Model", "PWD"] {
        let (mut ui, _, _) = fixture();
        ui.action(render::Action::NewAgent);
        if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
            name.text = "frontend".into();
            if missing == "PWD" {
                cwd.text.clear();
            }
        }
        if missing != "Model" {
            ui.action(render::Action::Provider(Provider::Codex));
        }
        if missing == "Name" {
            if let Some(forms::Form::Agent { name, .. }) = &mut ui.form {
                name.text.clear();
            }
        }

        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| key(&mut ui, KeyCode::Enter, KeyModifiers::NONE));

        assert!(
            matches!(ui.form, Some(forms::Form::Agent { .. })),
            "{missing}"
        );
        assert!(
            ui.pending
                .iter()
                .all(|pending| !matches!(pending.command, BusCommand::AddAgent(_))),
            "{missing}"
        );
        let error = format!("{missing} is required.");
        let screen = room_screen(&mut ui, 100, 40);
        assert!(!screen.contains(&error), "{missing}: {screen}");
        let logs = capture.text();
        assert_eq!(
            logs.matches("bus.form.validation_failed").count(),
            1,
            "{missing}: {logs}"
        );
        assert!(
            logs.contains(&format!("field=\"{}\"", missing.to_lowercase())),
            "{missing}: {logs}"
        );
        assert!(logs.contains(&error), "{missing}: {logs}");
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
    ui.error = Some("Agent PWD must be an absolute path or ~/ path".into());
    assert!(!room_screen(&mut ui, 120, 40).contains("PWD"));
    ui.error = None;
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = Some("Runtime connection unavailable".into());
    ui.receive_snapshot(Arc::new(snapshot));
    assert!(!room_screen(&mut ui, 120, 40).contains("Runtime connection unavailable"));
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
fn path_completion_uses_the_first_result_after_the_query_changes() {
    use crate::agents::providers::suggest::PathSuggestion;
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Files);
    ui.receive_event(BusEvent::Suggestions {
        query_id: ui.suggestions.query_id,
        result: Ok(["/first", "/second", "/third"]
            .into_iter()
            .map(|path| PathSuggestion {
                path: path.into(),
                is_directory: true,
            })
            .collect()),
    });
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(ui.suggestions.selected, 2);
    ui.input(
        &RawInputEvent::Paste("nested".into()),
        false,
        &mut Default::default(),
    );
    ui.receive_event(BusEvent::Suggestions {
        query_id: ui.suggestions.query_id,
        result: Ok(vec![PathSuggestion {
            path: "/nested".into(),
            is_directory: true,
        }]),
    });

    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);

    assert_eq!(
        ui.form.as_mut().unwrap().editor_mut().unwrap().text,
        "/nested/"
    );
}

#[test]
fn provider_menu_erases_the_pwd_suffix_under_its_choice() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    if let Some(forms::Form::Agent { cwd, .. }) = &mut ui.form {
        *cwd = editor::Editor::new("/projects/underlying-directory-suffix".into());
    }
    ui.action(render::Action::Field(1));
    ui.compute_view(100, 30);
    let choice = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Provider(Provider::Cursor))
        .expect("the provider menu offers Cursor")
        .rect;
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let label: String = (choice.x..choice.right())
        .map(|x| buffer[(x, choice.y)].symbol())
        .collect();

    assert_eq!(
        label.trim(),
        "Cursor",
        "a provider choice must erase the PWD text that its menu covers"
    );
}

fn agent_form_with_open_pwd_suggestions(master: bool) -> BusUi {
    use crate::agents::providers::suggest::PathSuggestion;
    let (mut ui, room, _) = fixture();
    ui.form = Some(forms::Form::Agent {
        name: editor::Editor::new("frontend".into()),
        provider: Some(Provider::ClaudeCode),
        provider_cursor: Provider::ClaudeCode,
        cwd: editor::Editor::new("/projects/fr".into()),
        args: Box::new(editor::Editor::new(String::new())),
        field: 2,
        orchestrates: master.then_some(forms::Orchestrates(Some(room))),
        prompt: master.then(|| {
            Box::new(forms::PromptField {
                editor: editor::Editor::new("You orchestrate.".into()),
                filled: String::new(),
            })
        }),
    });
    ui.receive_event(BusEvent::Suggestions {
        query_id: ui.suggestions.query_id,
        result: Ok([
            "/projects/frontend",
            "/projects/framework",
            "/projects/fresh",
        ]
        .into_iter()
        .map(|path| PathSuggestion {
            path: path.into(),
            is_directory: true,
        })
        .collect()),
    });
    // Hits must exist: the old handler ignores Tab until the list is drawn.
    room_screen(&mut ui, 120, 40);
    ui
}

fn agent_focus(ui: &BusUi) -> (usize, usize, String) {
    let field = match &ui.form {
        Some(forms::Form::Agent { field, .. }) => *field,
        _ => usize::MAX,
    };
    let cwd = match &ui.form {
        Some(forms::Form::Agent { cwd, .. }) => cwd.text.clone(),
        _ => String::new(),
    };
    (field, ui.suggestions.selected, cwd)
}

fn submitted_launch(ui: &BusUi) -> Option<(String, String)> {
    ui.pending
        .iter()
        .find_map(|pending| match &pending.command {
            BusCommand::AddAgent(input) | BusCommand::AddOrchestrator(input, _) => {
                Some((input.cwd.clone(), input.extra_args.clone()))
            }
            _ => None,
        })
}

#[test]
fn tab_moves_agent_form_focus_while_pwd_suggestions_are_open() {
    for master in [false, true] {
        let mut ui = agent_form_with_open_pwd_suggestions(master);
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(
            agent_focus(&ui),
            (3, 0, "/projects/fr".to_string()),
            "Tab must focus Additional launch args; master={master}"
        );
        assert_eq!(submitted_launch(&ui), None);
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        let after_args = if master { 4 } else { 0 };
        assert_eq!(
            agent_focus(&ui).0,
            after_args,
            "the next Tab leaves Additional launch args; master={master}"
        );

        let mut ui = agent_form_with_open_pwd_suggestions(master);
        key(&mut ui, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            agent_focus(&ui),
            (1, 0, "/projects/fr".to_string()),
            "Shift+Tab must focus Agent, not the previous suggestion; master={master}"
        );
    }
}

#[test]
fn up_and_down_are_the_only_pwd_list_navigation() {
    for master in [false, true] {
        let mut ui = agent_form_with_open_pwd_suggestions(master);
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(
            agent_focus(&ui),
            (2, 2, "/projects/fr".to_string()),
            "Down moves the highlight and stops on the last row; master={master}"
        );
        assert_eq!(submitted_launch(&ui), None);
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(agent_focus(&ui).1, 1);
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(
            agent_focus(&ui),
            (2, 0, "/projects/fr".to_string()),
            "Up moves the highlight and stops on the first row; master={master}"
        );
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(
            agent_focus(&ui),
            (3, 1, "/projects/fr".to_string()),
            "Tab must not move the directory highlight; master={master}"
        );
    }
}

#[test]
fn enter_submits_the_agent_form_from_each_unambiguous_field() {
    for master in [false, true] {
        let mut ui = agent_form_with_open_pwd_suggestions(master);
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            submitted_launch(&ui),
            Some(("/projects/fr".into(), String::new())),
            "Enter in PWD must Add the typed directory; master={master}"
        );
        assert_eq!(agent_focus(&ui).2, "/projects/fr");

        let mut ui = agent_form_with_open_pwd_suggestions(master);
        ui.action(render::Action::Field(0));
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            submitted_launch(&ui).map(|(cwd, _)| cwd),
            Some("/projects/fr".into()),
            "Enter in Name must Add; master={master}"
        );

        let mut ui = agent_form_with_open_pwd_suggestions(master);
        if let Some(forms::Form::Agent { field, args, .. }) = &mut ui.form {
            *field = 3;
            **args = editor::Editor::new("--verbose".into());
        }
        room_screen(&mut ui, 120, 40);
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            submitted_launch(&ui),
            Some(("/projects/fr".into(), "--verbose".into())),
            "Enter in Additional launch args must Add; master={master}"
        );

        let mut ui = agent_form_with_open_pwd_suggestions(master);
        if let Some(forms::Form::Agent {
            field, provider, ..
        }) = &mut ui.form
        {
            *field = 1;
            *provider = None;
        }
        room_screen(&mut ui, 120, 40);
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(
            submitted_launch(&ui),
            None,
            "Enter on Agent chooses the model; master={master}"
        );
        assert!(matches!(
            &ui.form,
            Some(forms::Form::Agent {
                provider: Some(Provider::ClaudeCode),
                field: 2,
                ..
            })
        ));

        if master {
            let mut ui = agent_form_with_open_pwd_suggestions(true);
            ui.action(render::Action::Field(forms::PROMPT_FIELD));
            key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
            assert_eq!(submitted_launch(&ui), None);
            assert!(matches!(
                &ui.form,
                Some(forms::Form::Agent { prompt: Some(prompt), .. })
                    if prompt.editor.text.contains('\n')
            ));
        }
    }
}

#[test]
fn ctrl_enter_confirms_the_typed_pwd_even_with_directory_suggestions_open() {
    use crate::agents::providers::suggest::PathSuggestion;

    for expanded in [false, true] {
        let (mut ui, room, _) = fixture();
        ui.form = Some(forms::Form::Agent {
            name: editor::Editor::new("frontend".into()),
            provider: Some(Provider::ClaudeCode),
            provider_cursor: Provider::ClaudeCode,
            cwd: editor::Editor::new("/projects/".into()),
            args: Box::new(editor::Editor::new(String::new())),
            field: 2,
            orchestrates: None,
            prompt: None,
        });
        if expanded {
            ui.receive_event(BusEvent::Suggestions {
                query_id: ui.suggestions.query_id,
                result: Ok(vec![PathSuggestion {
                    path: "/projects/frontend".into(),
                    is_directory: true,
                }]),
            });
        }

        key(&mut ui, KeyCode::Enter, KeyModifiers::CONTROL);

        let submitted = ui
            .pending
            .iter()
            .find_map(|pending| match &pending.command {
                BusCommand::AddAgent(input) => Some((input.room, input.cwd.as_str())),
                _ => None,
            });
        assert_eq!(
            submitted,
            Some((room, "/projects/")),
            "Ctrl+Enter confirms the form's typed directory; suggestions open: {expanded}"
        );
    }
}

#[test]
fn master_pwd_suggestions_stay_visible_and_enter_submits_the_typed_directory() {
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let master = snapshot.state.ensure_master_room();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.action(render::Action::Room(master));
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::ClaudeCode));
    ui.action(render::Action::Field(2));
    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        *name = editor::Editor::new("orchestrator".into());
        *cwd = editor::Editor::new("/projects/fr".into());
    }
    ui.receive_event(BusEvent::Suggestions {
        query_id: ui.suggestions.query_id,
        result: Ok([
            "/projects/frontend",
            "/projects/framework",
            "/projects/fresh",
        ]
        .into_iter()
        .map(|path| crate::agents::providers::suggest::PathSuggestion {
            path: path.into(),
            is_directory: true,
        })
        .collect()),
    });
    for error in [None, Some("PWD must be a directory".to_owned())] {
        ui.error = error;
        let screen = room_screen(&mut ui, 120, 40);
        assert!(screen.contains("System prompt"), "{screen}");
        assert!(screen.contains("Add (Enter)"), "{screen}");
        assert!(screen.contains("/projects/frontend/"), "{screen}");
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        let screen = room_screen(&mut ui, 120, 40);
        let selected = ui.suggestions.selected;
        assert!(
            screen.contains(&format!(
                "{}/",
                ui.suggestions.entries[selected].path.display()
            )),
            "{screen}"
        );
    }
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(&ui.form, Some(forms::Form::Agent { cwd, .. }) if cwd.text == "/projects/fr"));
    assert!(ui.pending.iter().any(|p| matches!(
        &p.command,
        BusCommand::AddAgent(input) | BusCommand::AddOrchestrator(input, _)
            if input.cwd == "/projects/fr"
    )));
    assert_ne!(master, room);
}

#[test]
fn enter_uses_typed_pwd_when_the_suggestion_list_does_not_fit() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::ClaudeCode));
    ui.action(render::Action::Field(2));
    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        *name = editor::Editor::new("worker".into());
        *cwd = editor::Editor::new("/projects".into());
    }
    ui.receive_event(BusEvent::Suggestions {
        query_id: ui.suggestions.query_id,
        result: Ok(vec![crate::agents::providers::suggest::PathSuggestion {
            path: "/projects/hidden".into(),
            is_directory: true,
        }]),
    });
    let screen = room_screen(&mut ui, 120, 16);
    assert!(!screen.contains("/projects/hidden"), "{screen}");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.pending.iter().any(|p| matches!(&p.command,
        BusCommand::AddAgent(input) if input.cwd == "/projects")));
}

fn form_rows(ui: &mut BusUi, cols: u16, rows: u16) -> Vec<String> {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    (0..rows)
        .map(|y| (0..cols).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn provider_menu_stays_below_the_model_field_and_before_pwd() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Add);
    ui.action(render::Action::Field(1));
    form_rows(&mut ui, 100, 30);
    let model = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Field(1))
        .unwrap()
        .rect;
    let pwd = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Field(2))
        .unwrap()
        .rect;
    for provider in [
        Provider::Codex,
        Provider::ClaudeCode,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        let rect = ui
            .view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::Provider(provider))
            .unwrap()
            .rect;
        assert!(!model.intersects(rect), "{provider:?} covers Model");
        assert!(!pwd.intersects(rect), "{provider:?} covers PWD");
    }
}

#[test]
fn async_pwd_rejection_is_log_only_with_its_field_and_path() {
    let (mut ui, room, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::Codex));
    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        *name = editor::Editor::new("worker".into());
        *cwd = editor::Editor::new("/definitely/missing".into());
    }
    ui.action(render::Action::Add);
    let id = ui
        .pending
        .iter()
        .find(|pending| matches!(pending.command, BusCommand::AddAgent(_)))
        .unwrap()
        .id;
    let preceding: Vec<_> = ui
        .pending
        .iter()
        .filter(|pending| pending.id < id)
        .map(|pending| pending.id)
        .collect();
    let error = "Agent PWD does not exist";
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        for command_id in preceding {
            ui.receive_event(BusEvent::CommandFinished {
                command_id,
                result: Ok(()),
            });
        }
        ui.receive_event(BusEvent::CommandFinished {
            command_id: id,
            result: Err(error.into()),
        });
        let mut snapshot = (*ui.snapshot).clone();
        snapshot.last_command_id = id;
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
        ui.settle();
    });
    assert!(ui.error.is_none());
    assert!(matches!(ui.form, Some(forms::Form::Agent { .. })));
    assert!(!room_screen(&mut ui, 100, 40).contains(error));
    let logs = capture.text();
    assert_eq!(logs.matches("bus.command.failed").count(), 1, "{logs}");
    assert_eq!(logs.matches(error).count(), 1, "{logs}");
    assert!(logs.contains(&format!("command_id={id}")), "{logs}");
    assert!(
        logs.contains(&format!("room_id=Some({})", room.0)),
        "{logs}"
    );
    assert!(logs.contains("agent=\"worker\""), "{logs}");
    assert!(logs.contains("field=\"pwd\""), "{logs}");
    assert!(logs.contains("path=\"/definitely/missing\""), "{logs}");
}

#[test]
fn suggestion_failure_is_logged_without_a_form_error_dialog() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        ui.receive_event(BusEvent::Suggestions {
            query_id: ui.suggestions.query_id,
            result: Err("directory unreadable".into()),
        });
    });
    assert!(
        capture.text().contains("directory unreadable"),
        "{}",
        capture.text()
    );
    assert!(!room_screen(&mut ui, 100, 30).contains("OK (Enter)"));
}

#[test]
fn external_room_ack_preserves_the_form_of_a_rejected_creation() {
    let (mut ui, room, _) = fixture();
    submit_room_form(&mut ui, "rejected-room");
    let id = ui
        .pending
        .iter()
        .find(|pending| matches!(pending.command, BusCommand::CreateRoom(_)))
        .expect("room creation is pending")
        .id;
    ui.receive_event(BusEvent::CommandFinished {
        command_id: id,
        result: Err("room already exists".into()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = id;
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(
        matches!(ui.form, Some(forms::Form::Room(_))),
        "the rejected form must stay open for correction"
    );

    assert!(
        ui.creation_origins.iter().all(|origin| origin.id != id),
        "rejected creation origin was not retired"
    );
    let mut snapshot = (*ui.snapshot).clone();
    let external = snapshot.state.create_room("external-room").unwrap();
    ui.receive_event(BusEvent::RoomCreated {
        room: external,
        command_id: id,
    });
    assert!(
        matches!(ui.form, Some(forms::Form::Room(_))),
        "a replay of the rejected command closed its form"
    );
    ui.receive_event(BusEvent::RoomCreated {
        room: external,
        command_id: 0,
    });

    assert!(
        matches!(ui.form, Some(forms::Form::Room(_))),
        "an external acknowledgement closed the rejected creation's form"
    );
    assert_eq!(
        ui.room,
        Some(room),
        "an external acknowledgement moved the visible room"
    );
}
