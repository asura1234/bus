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
    });
    assert_eq!(ui.room, Some(first));
    assert!(ui.terminal.is_none() && ui.target_pane.is_none());
    assert!(
        matches!(ui.pending.back().unwrap().command, BusCommand::SelectRoom(id) if id == first)
    );
    assert_eq!(ui.locals[&first].text.text, "unsaved local draft");
    assert_eq!(serde_json::to_value(&ui.snapshot.state).unwrap(), before);
}
