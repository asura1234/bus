use super::*;

fn fixture() -> (BusUi, RoomId, RoomId, AgentId, AgentId) {
    let mut state = BusState::default();
    let first = state.create_room("first").unwrap();
    let second = state.create_room("second").unwrap();
    let cwd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let a = state
        .create_agent(first, "one", Provider::Cursor, cwd.clone(), None)
        .unwrap();
    let b = state
        .create_agent(second, "two", Provider::Codex, cwd, None)
        .unwrap();
    state.set_draft_text(first, "saved draft").unwrap();
    let ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 1,
        last_command_id: 0,
        error: None,
    }));
    (ui, first, second, a, b)
}

#[test]
fn dev_focus_cross_room_updates_visible_target_and_keeps_old_focus_guard() {
    let (mut ui, first, second, a, b) = fixture();
    ui.locals.get_mut(&first).unwrap().text = Editor::new("unsaved local draft".into());
    ui.receive_event(BusEvent::DevFocusRequested {
        room: second,
        agent: Some(b),
        form_generation: None,
    });
    assert_eq!(ui.room, Some(second));
    assert_eq!(ui.terminal, Some(b));
    assert_eq!(ui.target_pane, None);
    assert!(matches!(ui.pending.back().unwrap().command, BusCommand::FocusTerminal(id) if id == b));
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::SelectRoom(_))));
    ui.receive_event(BusEvent::TerminalFocused {
        agent: a,
        pane_id: "old-pane".into(),
    });
    assert_eq!(ui.target_pane, None);
    ui.receive_event(BusEvent::TerminalFocused {
        agent: b,
        pane_id: "new-pane".into(),
    });
    assert_eq!(ui.target_pane.as_deref(), Some("new-pane"));
    assert_eq!(ui.locals[&first].text.text, "unsaved local draft");
}

#[test]
fn dev_room_focus_returns_to_room_without_replacing_drafts_or_persistent_state() {
    let (mut ui, first, second, _a, b) = fixture();
    ui.open_terminal(b);
    ui.room = Some(second);
    ui.target_pane = Some("pane".into());
    ui.locals.get_mut(&first).unwrap().text = Editor::new("unsaved local draft".into());
    let before = serde_json::to_value(&ui.snapshot.state).unwrap();
    ui.receive_event(BusEvent::DevFocusRequested {
        room: first,
        agent: None,
        form_generation: None,
    });
    assert_eq!(ui.room, Some(first));
    assert!(ui.terminal.is_none() && ui.target_pane.is_none());
    assert!(
        matches!(ui.pending.back().unwrap().command, BusCommand::SelectRoom(id) if id == first)
    );
    assert_eq!(ui.locals[&first].text.text, "unsaved local draft");
    assert_eq!(serde_json::to_value(&ui.snapshot.state).unwrap(), before);
}

#[test]
fn dev_focus_ack_preserves_a_form_opened_after_the_request() {
    let (mut ui, first, second, _a, b) = fixture();
    ui.form = Some(Form::Files(Editor::default()));
    let Some(Form::Files(editor)) = &mut ui.form else {
        panic!("file form was not opened");
    };
    editor.text = "typed-after-focus-request.txt".into();

    ui.receive_event(BusEvent::DevFocusRequested {
        room: second,
        agent: Some(b),
        form_generation: None,
    });

    assert_eq!(ui.room, Some(first));
    assert!(ui.terminal.is_none());
    let Some(Form::Files(editor)) = &ui.form else {
        panic!("newer file form was discarded");
    };
    assert_eq!(editor.text, "typed-after-focus-request.txt");
    assert!(!ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::FocusTerminal(id) if id == b)));
}

#[test]
fn dev_focus_ack_completes_when_form_predates_request() {
    let (mut ui, _first, second, _a, b) = fixture();
    ui.form = Some(Form::Files(Editor::new("preexisting-input.txt".into())));

    ui.receive_event(BusEvent::DevFocusRequested {
        room: second,
        agent: Some(b),
        form_generation: Some(0),
    });

    assert_eq!(ui.room, Some(second));
    assert_eq!(ui.terminal, Some(b));
    assert!(ui.form.is_none());
}

#[test]
fn dev_focus_ack_preserves_a_rename_opened_after_the_request() {
    use crate::client::rooms::forms::RenameTarget;

    let (mut ui, first, second, _a, b) = fixture();
    // Stamp the request before the rename opens. The production opener must
    // advance form generation, or this acknowledgement still owns the rename.
    let requested_generation = ui.form_generation;
    ui.start_rename(RenameTarget::Room(first));
    let Some(rename) = &mut ui.rename else {
        panic!("rename did not open");
    };
    rename.editor.text = "typed-after-focus-request".into();

    ui.receive_event(BusEvent::DevFocusRequested {
        room: second,
        agent: Some(b),
        form_generation: Some(requested_generation),
    });

    assert_eq!(ui.room, Some(first), "stale focus moved the room context");
    assert!(ui.terminal.is_none());
    let Some(rename) = &ui.rename else {
        panic!("rename opened after the focus request was discarded");
    };
    assert_eq!(rename.editor.text, "typed-after-focus-request");
    assert!(!ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::FocusTerminal(id) if id == b)));
}

#[test]
fn dev_focus_ack_completes_a_rename_that_predates_the_request() {
    use crate::client::rooms::forms::RenameTarget;

    let (mut ui, first, second, _a, b) = fixture();
    ui.start_rename(RenameTarget::Room(first));
    let requested_generation = ui.form_generation;

    ui.receive_event(BusEvent::DevFocusRequested {
        room: second,
        agent: Some(b),
        form_generation: Some(requested_generation),
    });

    assert_eq!(ui.room, Some(second));
    assert_eq!(ui.terminal, Some(b));
    assert!(ui.rename.is_none());
}

#[test]
fn dev_quit_runs_the_ui_save_and_quit_and_dev_settings_apply() {
    let (mut ui, _first, _second, _a, _b) = fixture();
    assert!(ui.quitting.is_none());
    ui.receive_event(BusEvent::DevQuitRequested);
    assert!(ui.quitting.is_some());
    ui.receive_event(BusEvent::SettingsChanged(
        crate::messaging::prefs::settings::BusSettings {
            color_blind_mode: true,
            ..Default::default()
        },
    ));
    assert!(ui.settings.color_blind_mode);
}
