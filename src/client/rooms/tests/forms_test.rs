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
    for _ in 0..10 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
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
fn settings_footer_advertises_enter_as_the_only_toggle_key() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);

    let screen = room_screen(&mut ui, 100, 30);

    assert!(screen.contains("Enter toggles"), "{screen}");
    assert!(!screen.contains("Space"), "{screen}");
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
fn agent_enter_adds_from_non_model_fields_with_its_own_directory_and_escape_cancels() {
    for field in [0, 2, 3] {
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
        // Suggestions must not hijack Enter; Tab remains path completion.
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
    assert!(ui.visible_error().is_none());
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

        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);

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
        assert!(
            ui.visible_error()
                .is_some_and(|error| error.contains(missing)),
            "{missing}: {:?}",
            ui.visible_error()
        );
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
