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

fn hit_rows(ui: &BusUi, action: render::Action) -> Vec<u16> {
    ui.view
        .hits
        .iter()
        .filter(|hit| hit.action == action)
        .map(|hit| hit.rect.y)
        .collect()
}

fn click(ui: &mut BusUi, row: u16) {
    use crossterm::event::{MouseButton, MouseEventKind};
    super::mouse(ui, MouseEventKind::Down(MouseButton::Left), 3, row);
}

#[test]
fn work_room_sidebar_lists_master_then_rooms_then_its_agents() {
    let (mut ui, master, pr, other, orchestrator) = master_fixture();
    // Work rooms stay the landing room; MASTER is not the default.
    assert_eq!(ui.room, Some(pr));
    let mut snapshot = (*ui.snapshot).clone();
    let worker = snapshot
        .state
        .create_agent(pr, "worker", Provider::Codex, "/repo".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(pr);
    let rows = sidebar_rows(&mut ui);
    assert_eq!(rows[0], "", "{rows:?}");
    assert!(rows[1].starts_with("# MASTER"), "{rows:?}");
    assert_eq!(rows[2], "", "{rows:?}");
    assert!(
        rows[3].starts_with("ROOMS") && rows[3].ends_with('+'),
        "{rows:?}"
    );
    assert!(rows[5].starts_with("# pr-123 ◆"), "{rows:?}");
    assert_eq!(rows[6], "# pr-456           Idle", "{rows:?}");
    assert!(rows[7].starts_with('─'), "{rows:?}");
    assert!(
        rows[8].starts_with("AGENTS") && rows[8].ends_with('+'),
        "{rows:?}"
    );
    assert!(rows[10].starts_with("worker"), "{rows:?}");
    assert!(
        !rows.iter().any(|row| row.trim() == "MASTER"),
        "no MASTER section header: {rows:?}"
    );
    assert!(!rows.iter().any(|row| row.starts_with("claude-orch")));

    assert_eq!(hit_rows(&ui, render::Action::Room(master)), [1, 1]);
    assert_eq!(hit_rows(&ui, render::Action::NewRoom), [3]);
    assert_eq!(hit_rows(&ui, render::Action::Room(pr)), [5, 5]);
    assert_eq!(hit_rows(&ui, render::Action::Room(other)), [6, 6]);
    assert_eq!(hit_rows(&ui, render::Action::NewAgent), [8]);
    assert_eq!(hit_rows(&ui, render::Action::Agent(worker))[0], 10);
    assert!(hit_rows(&ui, render::Action::Agent(orchestrator)).is_empty());

    click(&mut ui, 6);
    assert_eq!(ui.room, Some(other));
}

#[test]
fn master_sidebar_lists_its_agents_before_rooms() {
    let (mut ui, master, pr, other, orchestrator) = master_fixture();
    ui.open_room(master);
    let rows = sidebar_rows(&mut ui);
    assert_eq!(rows[0], "", "{rows:?}");
    assert!(rows[1].starts_with("# MASTER"), "{rows:?}");
    assert_eq!(rows[2], "", "{rows:?}");
    assert!(
        rows[3].starts_with("AGENTS") && rows[3].ends_with('+'),
        "{rows:?}"
    );
    assert!(rows[5].starts_with("claude-orch"), "{rows:?}");
    assert_eq!(rows[6], "Claude Code", "{rows:?}");
    assert_eq!(rows[7], "#pr-123", "{rows:?}");
    assert_eq!(rows[8], "", "{rows:?}");
    assert!(rows[9].starts_with('─'), "{rows:?}");
    assert!(
        rows[10].starts_with("ROOMS") && rows[10].ends_with('+'),
        "{rows:?}"
    );
    assert!(rows[12].starts_with("# pr-123 ◆"), "{rows:?}");
    assert_eq!(rows[13], "# pr-456           Idle", "{rows:?}");
    // The open MASTER room never offers deletion.
    assert!(deletable_rooms(&ui).is_empty());

    assert_eq!(hit_rows(&ui, render::Action::Room(master)), [1, 1]);
    assert_eq!(hit_rows(&ui, render::Action::NewAgent), [3]);
    assert_eq!(
        hit_rows(&ui, render::Action::Agent(orchestrator)),
        [5, 5, 6]
    );
    assert_eq!(hit_rows(&ui, render::Action::Reassign(orchestrator)), [7]);
    assert_eq!(hit_rows(&ui, render::Action::NewRoom), [10]);
    assert_eq!(hit_rows(&ui, render::Action::Room(pr)), [12, 12]);
    assert_eq!(hit_rows(&ui, render::Action::Room(other)), [13, 13]);
    assert_eq!(
        render::sidebar_room_row(&ui.snapshot.state, other, Some(master)),
        Some(13)
    );

    // Clicking a room under MASTER's agents opens it in the work order.
    click(&mut ui, 13);
    assert_eq!(ui.room, Some(other));
    let rows = sidebar_rows(&mut ui);
    assert!(rows[3].starts_with("ROOMS"), "{rows:?}");
    assert_eq!(hit_rows(&ui, render::Action::Room(other)), [6, 6]);
}

#[test]
fn master_agents_show_three_lines_without_an_expand_control() {
    let (mut ui, master, _, _, orchestrator) = master_fixture();
    let mut state = ui.snapshot.state.clone();
    state
        .set_agent_details_disclosed(orchestrator, true)
        .unwrap();
    ui.snapshot = Arc::new(BusSnapshot {
        state,
        revision: 1,
        last_command_id: 0,
        error: None,
    });
    ui.open_room(master);
    let rows = sidebar_rows(&mut ui);
    assert!(rows[5].starts_with("claude-orch"), "{rows:?}");
    assert_eq!(rows[6], "Claude Code", "{rows:?}");
    assert_eq!(rows[7], "#pr-123", "{rows:?}");
    // Even disclosed, no branch or cwd lines follow before the divider.
    assert_eq!(rows[8], "", "{rows:?}");
    assert!(rows[9].starts_with('─'), "{rows:?}");
    assert!(!rows.iter().any(|row| row.contains("/repo")), "{rows:?}");
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Details(orchestrator)));
    let hit_row = |action| {
        ui.view
            .hits
            .iter()
            .filter(|hit| hit.action == action)
            .map(|hit| hit.rect.y)
            .collect::<Vec<_>>()
    };
    assert_eq!(hit_row(render::Action::Reassign(orchestrator)), [7]);
    assert!(hit_row(render::Action::Agent(orchestrator)).contains(&6));
}

#[test]
fn room_rows_show_status_and_keep_marker_and_unread_count_readable() {
    let (ui, master, pr, other, orchestrator) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let worker = snapshot
        .state
        .create_agent(other, "worker", Provider::Codex, "/repo".into(), None)
        .unwrap();
    for agent in [orchestrator, worker] {
        snapshot.state.confirm_hook_setup(agent).unwrap();
    }
    snapshot
        .state
        .observe_status(orchestrator, RuntimeStatus::Working, 1)
        .unwrap();
    snapshot.state.observe_dialog(worker, true).unwrap();
    let mut value = serde_json::to_value(&snapshot.state).unwrap();
    value["rooms"][pr.0.to_string()]["name"] = serde_json::json!("a-very-long-room-name");
    value["rooms"][pr.0.to_string()]["unread_count"] = serde_json::json!(7);
    snapshot.state = serde_json::from_value(value).unwrap();
    snapshot.revision += 1;
    let mut ui = BusUi::new(Arc::new(snapshot));
    ui.open_room(other);
    let rows = sidebar_rows(&mut ui);

    assert_eq!(rows[1], "# MASTER        Working", "{rows:?}");
    // The name gives way so the marker, unread count and status all fit.
    assert_eq!(rows[5], "# a-very-lon… ◆  7 Idle", "{rows:?}");
    assert_eq!(rows[6], "# pr-456        Blocked ×", "{rows:?}");

    let status_hit = |room: RoomId| {
        ui.view
            .hits
            .iter()
            .filter(|hit| hit.action == render::Action::Room(room))
            .max_by_key(|hit| hit.rect.x)
            .unwrap()
            .rect
    };
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    // Rooms share the agents' animated status colors.
    let blocked = status_hit(other);
    assert_eq!(blocked.width, 7);
    assert_eq!(
        buffer[(blocked.x, blocked.y)].fg,
        ratatui::style::Color::Rgb(128, 44, 52)
    );
    let working = status_hit(master);
    assert_eq!(
        buffer[(working.x, working.y)].fg,
        ratatui::style::Color::Rgb(102, 255, 102)
    );
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
    assert_eq!(room_row(&ui, master), Some(1));
    assert_eq!(room_row(&ui, pr), Some(5));
    assert_eq!(room_row(&ui, other), Some(6));
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
        BusCommand::AddOrchestrator(input, spec)
            if input.room == master && input.name == "codex-orch" && spec.room == Some(other)
    )));
}

fn typed(ui: &mut BusUi, text: &str) {
    for c in text.chars() {
        key(ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
}

fn prompt_text(ui: &BusUi) -> String {
    match &ui.form {
        Some(forms::Form::Agent {
            prompt: Some(prompt),
            ..
        }) => prompt.editor.text.clone(),
        _ => panic!("no MASTER agent form"),
    }
}

fn expected_prompt(agent: &str, room: Option<(&str, RoomId)>) -> String {
    let docs = crate::bus::entry::data_dir().map_or_else(
        || std::path::PathBuf::from("<BUS_DATA_DIR>/docs"),
        |data| crate::bus::orchestrator::docs_dir(&data),
    );
    crate::bus::orchestrator::fill(
        crate::bus::orchestrator::DEFAULT_PROMPT,
        &crate::bus::orchestrator::PromptValues {
            room: room.map(|(name, id)| (name.into(), id)),
            agent: agent.into(),
            docs,
        },
    )
}

#[test]
fn the_master_agent_form_prefills_the_prompt_and_refills_it_until_edited() {
    let (mut ui, master, _, other, _) = master_fixture();
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    assert!(matches!(
        &ui.form,
        Some(forms::Form::Agent { cwd, .. }) if cwd.text == "~/"
    ));
    assert_eq!(prompt_text(&ui), expected_prompt("", None));
    assert!(room_screen(&mut ui, 100, 40).contains("System prompt"));

    typed(&mut ui, "orch");
    assert_eq!(prompt_text(&ui), expected_prompt("orch", None));
    ui.action(render::Action::Orchestrates);
    assert_eq!(
        prompt_text(&ui),
        expected_prompt("orch", Some(("pr-456", other)))
    );

    ui.action(render::Action::Field(forms::PROMPT_FIELD));
    typed(&mut ui, "Mine. ");
    let edited = prompt_text(&ui);
    assert!(edited.starts_with("Mine. # Bus orchestrator"), "{edited}");
    ui.action(render::Action::Orchestrates);
    assert_eq!(prompt_text(&ui), edited, "an edited prompt is kept");
}

#[test]
fn enter_adds_a_prompt_line_and_ctrl_enter_adds_the_orchestrator_with_it() {
    let (mut ui, master, _, other, _) = master_fixture();
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    typed(&mut ui, "orch");
    ui.action(render::Action::Orchestrates);
    ui.action(render::Action::Provider(Provider::ClaudeCode));
    if let Some(forms::Form::Agent { cwd, args, .. }) = &mut ui.form {
        *cwd = editor::Editor::new("/repo".into());
        **args = editor::Editor::new("--model sonnet".into());
    }
    ui.action(render::Action::Field(forms::PROMPT_FIELD));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::AddOrchestrator(..))));
    let text = prompt_text(&ui);
    assert!(text.starts_with('\n'), "{text:?}");

    key(&mut ui, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(ui.pending.iter().any(|p| matches!(
        &p.command,
        BusCommand::AddOrchestrator(input, spec)
            if input.cwd == "/repo"
                && input.extra_args == "--model sonnet"
                && input.provider == Provider::ClaudeCode
                && spec.room == Some(other)
                && spec.system_prompt.as_deref() == Some(text.as_str())
    )));
}

#[test]
fn hook_consent_keeps_the_orchestrator_prompt() {
    let (mut ui, master, _, other, _) = master_fixture();
    let spec = crate::bus::orchestrator::OrchestratorSpec {
        room: Some(other),
        system_prompt: Some("Run pr-456.".into()),
    };
    ui.receive_event(BusEvent::SetupRequired {
        input: crate::bus::launch::AddAgent {
            room: master,
            name: "codex-orch".into(),
            provider: Provider::Codex,
            cwd: "/repo".into(),
            extra_args: String::new(),
            consent_project_hooks: false,
        },
        orchestrator: Some(spec.clone()),
        notice: crate::bus::launch::SetupNotice {
            path: "/repo/.codex/hooks.json".into(),
            message: "review hooks".into(),
        },
    });
    ui.action(render::Action::Add);
    assert!(ui.pending.iter().any(|p| matches!(
        &p.command,
        BusCommand::AddOrchestrator(input, queued)
            if input.consent_project_hooks && *queued == spec
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

fn queued_orchestrates(ui: &BusUi) -> Vec<(AgentId, Option<RoomId>)> {
    ui.pending
        .iter()
        .filter_map(|p| match p.command {
            BusCommand::SetOrchestrates(agent, room) => Some((agent, room)),
            _ => None,
        })
        .collect()
}

#[test]
fn an_orchestrators_detail_line_reassigns_or_unassigns_it() {
    let (mut ui, master, pr, other, orchestrator) = master_fixture();
    ui.open_room(master);
    sidebar_rows(&mut ui);
    // The detail line is the button; the name still opens the terminal.
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Reassign(orchestrator)));
    ui.action(render::Action::Reassign(orchestrator));
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("claude-orch orchestrates"), "{screen}");
    assert!(screen.contains("< pr-123 >"), "{screen}");

    // Its own room stays a choice; the cycle runs pr-123, pr-456, then none.
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("< pr-456 >"));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert_eq!(queued_orchestrates(&ui), [(orchestrator, Some(other))]);

    ui.action(render::Action::Reassign(orchestrator));
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(queued_orchestrates(&ui).last(), Some(&(orchestrator, None)));

    ui.action(render::Action::Reassign(orchestrator));
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert_eq!(queued_orchestrates(&ui).len(), 2);
    let _ = pr;
}

#[test]
fn unassigned_master_agents_show_unassigned_and_work_agents_keep_their_detail_action() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let idle = state
        .create_agent(master, "spare", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let builder = state
        .create_agent(work, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    ui.open_room(master);
    let rows = sidebar_rows(&mut ui);
    assert!(rows.iter().any(|row| row == "#unassigned"), "{rows:?}");
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Reassign(idle)));

    ui.open_room(work);
    let rows = sidebar_rows(&mut ui);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("Codex") && !row.contains('→')),
        "{rows:?}"
    );
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Reassign(builder)));
}

#[test]
fn master_agent_form_validation_errors_are_visible_in_a_24_row_terminal() {
    let (mut ui, master, _, _, _) = master_fixture();
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Add);
    let screen = room_screen(&mut ui, 100, 24);
    assert!(
        screen.contains("Name and Model are required."),
        "validation must explain why Add did not submit in a 24-row terminal: {screen}"
    );
}
