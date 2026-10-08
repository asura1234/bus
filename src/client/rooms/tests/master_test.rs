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
    state.bind_orchestrator(orchestrator, pr).unwrap();
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
    assert_eq!(rows[1], "# MASTER", "{rows:?}");
    assert_eq!(rows[2], "", "{rows:?}");
    assert!(
        rows[3].starts_with("ROOMS") && rows[3].ends_with('+'),
        "{rows:?}"
    );
    assert_eq!(rows[5], "# pr-123           Idle ×", "{rows:?}");
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

    assert_eq!(hit_rows(&ui, render::Action::Room(master)), [1]);
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
    assert_eq!(rows[1], "# MASTER", "{rows:?}");
    assert_eq!(rows[2], "", "{rows:?}");
    assert!(
        rows[3].starts_with("AGENTS") && rows[3].ends_with('+'),
        "{rows:?}"
    );
    assert!(rows[5].starts_with("claude-orch"), "{rows:?}");
    assert_eq!(rows[6], "Claude Code", "{rows:?}");
    assert_eq!(rows[7], "# pr-123", "{rows:?}");
    // The card names its room exactly as the ROOMS list below does.
    assert!(
        rows[10..].iter().any(|row| row.starts_with("# pr-123 ")),
        "{rows:?}"
    );
    assert_eq!(rows[8], "", "{rows:?}");
    assert!(rows[9].starts_with('─'), "{rows:?}");
    assert!(
        rows[10].starts_with("ROOMS") && rows[10].ends_with('+'),
        "{rows:?}"
    );
    assert_eq!(rows[12], "# pr-123           Idle", "{rows:?}");
    assert_eq!(rows[13], "# pr-456           Idle", "{rows:?}");
    // The open MASTER room never offers deletion.
    assert!(deletable_rooms(&ui).is_empty());

    assert_eq!(hit_rows(&ui, render::Action::Room(master)), [1]);
    assert_eq!(hit_rows(&ui, render::Action::NewAgent), [3]);
    assert_eq!(
        hit_rows(&ui, render::Action::Agent(orchestrator)),
        [5, 5, 6]
    );
    assert_eq!(hit_rows(&ui, render::Action::NewRoom), [10]);
    // The orchestrator's #pr-123 line opens its room; it never reassigns.
    assert_eq!(hit_rows(&ui, render::Action::Room(pr)), [12, 12, 7]);
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
    let (mut ui, master, pr, _, orchestrator) = master_fixture();
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
    assert_eq!(rows[7], "# pr-123", "{rows:?}");
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
    assert!(hit_row(render::Action::Room(pr)).contains(&7));
    assert!(hit_row(render::Action::Agent(orchestrator)).contains(&6));
}

#[test]
fn room_rows_show_status_and_truncate_long_names() {
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
    // Neither the orchestrated-room marker nor an unread count is drawn.
    value["rooms"][pr.0.to_string()]["unread_count"] = serde_json::json!(7);
    snapshot.state = serde_json::from_value(value).unwrap();
    snapshot.revision += 1;
    let mut ui = BusUi::new(Arc::new(snapshot));
    ui.open_room(other);
    let rows = sidebar_rows(&mut ui);

    // MASTER shows just its name, even while its orchestrator works.
    assert_eq!(rows[1], "# MASTER", "{rows:?}");
    // The name gives way so the status fits.
    assert_eq!(rows[5], "# a-very-long-roo… Idle", "{rows:?}");
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
    assert_eq!(status_hit(master).x, 1, "MASTER has only its name hit");
    assert!((10..25).all(|x| buffer[(x, 1)].symbol() == " "));
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
    // pr-123 already has an orchestrator, so pr-456 is the only choice: there
    // is no "none", because an orchestrator exists only for its room.
    assert!(screen.contains("< pr-456 >"), "{screen}");
    ui.action(render::Action::Orchestrates);
    assert!(room_screen(&mut ui, 100, 30).contains("< pr-456 >"));
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);

    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        name.insert("cursor-orch");
        *cwd = editor::Editor::new("/repo".into());
    }
    ui.action(render::Action::Provider(Provider::Cursor));
    ui.action(render::Action::Add);
    assert!(ui.pending.iter().any(|p| matches!(
        &p.command,
        BusCommand::AddOrchestrator(input, spec)
            if input.room == master && input.name == "cursor-orch" && spec.room == other
    )));
}

#[test]
fn a_master_agent_cannot_be_added_without_a_free_work_room() {
    let (mut ui, master, _, other, _) = master_fixture();
    let mut state = ui.snapshot.state.clone();
    let taken = state
        .create_agent(master, "codex-orch", Provider::Codex, "/repo".into(), None)
        .unwrap();
    state.bind_orchestrator(taken, other).unwrap();
    ui.snapshot = Arc::new(BusSnapshot {
        state,
        revision: 1,
        last_command_id: 0,
        error: None,
    });
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    assert!(room_screen(&mut ui, 100, 30).contains("< no work room without an orchestrator >"));
    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        name.insert("spare");
        *cwd = editor::Editor::new("/repo".into());
    }
    ui.action(render::Action::Provider(Provider::Codex));
    ui.action(render::Action::Add);
    assert!(ui
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Orchestrates room is required")));
    assert!(!ui.pending.iter().any(|p| matches!(
        p.command,
        BusCommand::AddOrchestrator(..) | BusCommand::AddAgent(_)
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
    let docs = crate::utils::env::bus_data_dir().map_or_else(
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
    // The form starts on the only work room without an orchestrator.
    assert_eq!(
        prompt_text(&ui),
        expected_prompt("", Some(("pr-456", other)))
    );
    assert!(room_screen(&mut ui, 100, 40).contains("System prompt"));

    typed(&mut ui, "orch");
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
                && spec.room == other
                && spec.system_prompt.as_deref() == Some(text.as_str())
    )));
}

#[test]
fn hook_consent_keeps_the_orchestrator_prompt() {
    let (mut ui, master, _, other, _) = master_fixture();
    let spec = crate::bus::orchestrator::OrchestratorSpec {
        room: other,
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

#[test]
fn an_orchestrators_room_line_opens_its_room_and_never_reassigns() {
    let (mut ui, master, pr, _, orchestrator) = master_fixture();
    ui.open_room(master);
    sidebar_rows(&mut ui);
    // The detail line opens the orchestrated room; the name still opens the terminal.
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Room(pr) && hit.rect.y == 7));
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Agent(orchestrator)));
    ui.action(render::Action::Room(pr));
    assert_eq!(ui.room, Some(pr));
    assert!(ui.form.is_none());
    assert_eq!(
        ui.snapshot.state.agent(orchestrator).unwrap().orchestrates,
        Some(pr)
    );
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
    // Only saved state can hold one: Bus never creates an unassigned orchestrator.
    assert!(rows.iter().any(|row| row == "# unassigned"), "{rows:?}");
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Agent(idle)));

    ui.open_room(work);
    let rows = sidebar_rows(&mut ui);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("Codex") && !row.contains('→')),
        "{rows:?}"
    );
    assert!(ui
        .view
        .hits
        .iter()
        .any(|hit| hit.action == render::Action::Details(builder)));
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

/// Columns where `name` is drawn on the first screen row containing `marker`,
/// and those cells' colors.
fn name_colors(ui: &mut BusUi, marker: &str, name: &str) -> Vec<ratatui::style::Color> {
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let sidebar = ui.view.sidebar.width;
    for y in 0..30 {
        let cells: Vec<_> = (sidebar..100).map(|x| buffer[(x, y)].symbol()).collect();
        let row = cells.concat();
        if !row.contains(marker) {
            continue;
        }
        let start = cells
            .windows(name.chars().count())
            .position(|window| window.concat() == name)
            .expect("name on the marker row") as u16;
        return (0..name.chars().count() as u16)
            .map(|offset| buffer[(sidebar + start + offset, y)].fg)
            .collect();
    }
    panic!("no row with {marker:?}");
}

#[test]
fn orchestrators_are_green_in_work_room_messages_in_both_palettes() {
    let green = ratatui::style::Color::Rgb(102, 255, 102);
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
        .unwrap();
    let worker = state
        .create_agent(work, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    // Per-room allocation can give both the same color; force that case.
    let mut value = serde_json::to_value(&state).unwrap();
    for agent in [orchestrator, worker] {
        value["agents"][agent.0.to_string()]["color"] = serde_json::json!([255, 85, 255]);
        value["agents"][agent.0.to_string()]["accessible_color"] = serde_json::json!([255, 170, 0]);
    }
    let mut state: BusState = serde_json::from_value(value).unwrap();
    let draft = |to| Draft {
        text: "next step".into(),
        files: Vec::new(),
        recipient_ids: AgentRecipients::from([to]),
    };
    state
        .submit_message_from(work, draft(worker), Author::Agent(orchestrator), 5)
        .unwrap();
    state
        .submit_message_from(master, draft(orchestrator), Author::Human, 5)
        .unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));

    for (color_blind, worker_color) in [
        (false, ratatui::style::Color::Rgb(255, 85, 255)),
        (true, ratatui::style::Color::Rgb(255, 170, 0)),
    ] {
        ui.settings.color_blind_mode = color_blind;
        ui.open_room(work);
        assert_eq!(name_colors(&mut ui, "orch → builder", "orch"), [green; 4]);
        assert_eq!(
            name_colors(&mut ui, "orch → builder", "builder"),
            [worker_color; 7]
        );
        // Workers never get You's green, so the orchestrator stays distinct.
        assert_ne!(worker_color, green);

        // MASTER keeps identity colors so orchestrators stay apart there.
        ui.open_room(master);
        assert_eq!(
            name_colors(&mut ui, "You → orch", "orch"),
            [worker_color; 4]
        );
    }
}

#[test]
fn an_agent_waiting_on_a_dialog_reads_blocked_on_its_row_and_its_room() {
    let (ui, _, pr, _, _) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let codex = snapshot
        .state
        .create_agent(pr, "codex-dev", Provider::Codex, "/repo".into(), None)
        .unwrap();
    snapshot.state.confirm_hook_setup(codex).unwrap();
    // Codex reports Idle while its approval dialog is on screen.
    snapshot
        .state
        .observe_status(codex, RuntimeStatus::Idle, 1)
        .unwrap();
    snapshot.state.observe_dialog(codex, true).unwrap();
    let mut ui = BusUi::new(Arc::new(snapshot));
    ui.open_room(pr);
    let rows = sidebar_rows(&mut ui);

    assert_eq!(rows[5], "# pr-123        Blocked ×", "{rows:?}");
    let agent_row = rows
        .iter()
        .find(|row| row.starts_with("codex-dev"))
        .expect("agent row");
    assert!(agent_row.contains("Blocked"), "{rows:?}");
    assert!(!agent_row.contains("Idle"), "{rows:?}");

    // The dialog closes: both rows follow the same state change.
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.observe_dialog(codex, false).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let rows = sidebar_rows(&mut ui);
    assert_eq!(rows[5], "# pr-123           Idle ×", "{rows:?}");
    assert!(rows
        .iter()
        .any(|row| row.starts_with("codex-dev") && row.contains("Idle")));
}

#[test]
fn the_master_form_offers_no_codex_and_a_work_room_form_does() {
    let (mut ui, master, _, other, _) = master_fixture();
    // The offered providers are the form's clickable provider rows.
    let choices = |ui: &mut BusUi| {
        room_screen(ui, 120, 40);
        ui.view
            .hits
            .iter()
            .filter_map(|hit| match hit.action {
                render::Action::Provider(kind) => Some(kind),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    typed(&mut ui, "orch");
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(choices(&mut ui), [Provider::ClaudeCode, Provider::Cursor]);
    // Arrow keys cycle only through the offered providers.
    for code in [
        KeyCode::Down,
        KeyCode::Down,
        KeyCode::Up,
        KeyCode::Up,
        KeyCode::Up,
    ] {
        key(&mut ui, code, KeyModifiers::NONE);
        let Some(forms::Form::Agent {
            provider_cursor, ..
        }) = &ui.form
        else {
            panic!("agent form closed")
        };
        assert_ne!(*provider_cursor, Provider::Codex);
    }
    // Codex forced in anyway is refused with the explanation, and nothing is sent.
    ui.action(render::Action::Provider(Provider::Codex));
    if let Some(forms::Form::Agent { cwd, .. }) = &mut ui.form {
        *cwd = editor::Editor::new("/repo".into());
    }
    key(&mut ui, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(!ui.pending.iter().any(|p| matches!(
        p.command,
        BusCommand::AddOrchestrator(..) | BusCommand::AddAgent(..)
    )));
    assert_eq!(
        ui.error.as_deref(),
        Some(crate::bus::orchestrator::CODEX_ORCHESTRATOR_REFUSED)
    );
    assert!(
        matches!(ui.form, Some(forms::Form::Agent { .. })),
        "the form stays open"
    );

    // A work room's form still offers Codex, and a Codex worker may resume.
    ui.form = None;
    ui.error = None;
    ui.open_room(other);
    ui.action(render::Action::NewAgent);
    typed(&mut ui, "codex-worker");
    key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(
        choices(&mut ui),
        [Provider::Codex, Provider::ClaudeCode, Provider::Cursor]
    );
    ui.action(render::Action::Provider(Provider::Codex));
    if let Some(forms::Form::Agent { cwd, args, .. }) = &mut ui.form {
        *cwd = editor::Editor::new("/repo".into());
        **args = editor::Editor::new("resume 01a10f9e-71ac-79e2-81b2-56f26341e7e4".into());
    }
    key(&mut ui, KeyCode::Enter, KeyModifiers::CONTROL);
    assert!(ui.error.is_none(), "{:?}", ui.error);
    assert!(ui.pending.iter().any(|p| matches!(
        &p.command,
        BusCommand::AddAgent(input) if input.provider == Provider::Codex
    )));
}
