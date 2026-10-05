use super::*;

/// MASTER plus two work rooms; `claude-orch` in MASTER orchestrates `pr-123`.
fn master_fixture() -> (BusUi, RoomId, RoomId, RoomId, AgentId) {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let pr = state.create_room("pr-123").unwrap();
    let other = state.create_room("pr-456").unwrap();
    let orchestrator = state
        .create_agent(
            master,
            "claude-orch",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state
        .set_agent_orchestrates(orchestrator, Some(pr))
        .unwrap();
    let ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    (ui, master, pr, other, orchestrator)
}

fn sidebar_rows(ui: &mut BusUi) -> Vec<String> {
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    (0..30)
        .map(|y| {
            (1..27)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn deletable_rooms(ui: &BusUi) -> Vec<RoomId> {
    ui.view
        .hits
        .iter()
        .filter_map(|hit| match hit.action {
            render::Action::Delete(super::deletion::DeleteTarget::Room(room)) => Some(room),
            _ => None,
        })
        .collect()
}

#[test]
fn sidebar_puts_master_in_its_own_section_above_rooms() {
    let (mut ui, master, pr, _, _) = master_fixture();
    // Work rooms stay the landing room; MASTER is not the default.
    assert_eq!(ui.room, Some(pr));
    ui.open_room(master);
    let rows = sidebar_rows(&mut ui);
    assert!(rows[1].starts_with("MASTER"), "{rows:?}");
    assert!(rows[3].starts_with("# MASTER"), "{rows:?}");
    assert!(rows[5].starts_with("ROOMS"), "{rows:?}");
    assert!(rows[5].ends_with('+'), "{rows:?}");
    assert!(rows[7].starts_with("# pr-123 ◆"), "{rows:?}");
    assert_eq!(rows[8], "# pr-456");
    // The open MASTER room never offers deletion.
    assert!(deletable_rooms(&ui).is_empty());
    assert!(rows[9].starts_with('─'), "{rows:?}");
    assert!(rows[10].starts_with("AGENTS"), "{rows:?}");
    assert!(rows[12].starts_with("claude-orch"), "{rows:?}");
    assert!(rows[13].starts_with("Claude Code → pr-123"), "{rows:?}");
}

#[test]
fn work_rooms_keep_their_delete_button_and_row_hit_targets() {
    let (mut ui, master, pr, other, _) = master_fixture();
    for room in [pr, other] {
        ui.open_room(room);
        sidebar_rows(&mut ui);
        assert_eq!(deletable_rooms(&ui), [room]);
    }
    let room_row = |ui: &BusUi, room: RoomId| {
        ui.view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::Room(room))
            .map(|hit| hit.rect.y)
    };
    assert_eq!(room_row(&ui, master), Some(3));
    assert_eq!(room_row(&ui, pr), Some(7));
    assert_eq!(room_row(&ui, other), Some(8));
}

#[test]
fn master_room_cannot_be_deleted_from_the_ui() {
    let (mut ui, master, _, _, _) = master_fixture();
    ui.open_room(master);
    ui.start_delete(super::deletion::DeleteTarget::Room(master));
    assert!(ui.deletion.is_none());
    assert_eq!(
        ui.error.as_deref(),
        Some("The MASTER room cannot be renamed or deleted")
    );
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::DeleteRoom(_))));
}

#[test]
fn adding_an_agent_in_master_asks_which_unorchestrated_room_it_orchestrates() {
    let (mut ui, master, _, other, _) = master_fixture();
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("Orchestrates room"), "{screen}");
    assert!(screen.contains("< none >"), "{screen}");

    // pr-123 already has an orchestrator, so the only choices are none and pr-456.
    ui.action(render::Action::Orchestrates);
    assert!(room_screen(&mut ui, 100, 30).contains("< pr-456 >"));
    ui.action(render::Action::Orchestrates);
    assert!(room_screen(&mut ui, 100, 30).contains("< none >"));
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);

    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        name.insert("codex-orch");
        *cwd = editor::Editor::new("/repo".into());
    }
    ui.action(render::Action::Provider(Provider::Codex));
    ui.action(render::Action::Add);
    assert!(ui.pending.iter().any(|p| matches!(
        &p.command,
        BusCommand::AddOrchestrator(input, room)
            if input.room == master && input.name == "codex-orch" && *room == other
    )));
}

#[test]
fn work_room_agent_forms_have_no_orchestrates_choice() {
    let (mut ui, _, pr, _, _) = master_fixture();
    ui.open_room(pr);
    ui.action(render::Action::NewAgent);
    assert!(matches!(
        ui.form,
        Some(forms::Form::Agent {
            orchestrates: None,
            ..
        })
    ));
    assert!(!room_screen(&mut ui, 100, 30).contains("Orchestrates room"));
}
