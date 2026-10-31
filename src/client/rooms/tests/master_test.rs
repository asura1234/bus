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

#[test]
fn unread_dot_is_green_clickable_and_survives_narrow_room_labels() {
    let (ui, master, room, other, author) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .rename_room(room, "非常长的房间名称-with-a-long-suffix")
        .unwrap();
    for target in [master, room] {
        snapshot
            .state
            .post_to_developer(target, author, "decision".into(), vec![], 1)
            .unwrap();
    }
    let worker = snapshot
        .state
        .create_agent(room, "working", Provider::Codex, "/repo".into(), None)
        .unwrap();
    snapshot.state.confirm_hook_setup(worker).unwrap();
    snapshot
        .state
        .observe_status(worker, RuntimeStatus::Working, 1)
        .unwrap();
    let mut ui = BusUi::new(Arc::new(snapshot));
    ui.open_room(other);
    for (cols, width) in [(40, 20), (60, 20), (100, 28), (180, 60)] {
        ui.settings.sidebar_width = width;
        ui.compute_view(cols, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, 30));
        ui.render(&mut buffer);
        for target in [master, room] {
            let row = render::sidebar_room_row(&ui.snapshot.state, target, ui.room).unwrap() as u16;
            let dots: Vec<_> = (0..ui.view.sidebar.width)
                .filter(|&x| buffer[(x, row)].symbol() == "●")
                .collect();
            assert_eq!(dots.len(), 1, "room {target:?}, width {width}");
            let x = dots[0];
            assert_eq!(buffer[(x, row)].fg, render::ACCENT);
            assert_eq!(ui.view.hit_at(x, row), Some(&render::Action::Room(target)));
        }
    }
}

#[test]
fn unread_dot_stays_visible_beside_long_room_statuses_in_narrow_sidebars() {
    for (status, cols) in [
        (RuntimeStatus::Working, 36),
        (RuntimeStatus::Unavailable, 40),
    ] {
        let (ui, _master, room, other, author) = master_fixture();
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .post_to_developer(room, author, "decision".into(), vec![], 1)
            .unwrap();
        let worker = snapshot
            .state
            .create_agent(room, "status", Provider::Codex, "/repo".into(), None)
            .unwrap();
        snapshot.state.confirm_hook_setup(worker).unwrap();
        snapshot.state.observe_status(worker, status, 1).unwrap();
        let mut ui = BusUi::new(Arc::new(snapshot));
        ui.open_room(other);
        ui.settings.sidebar_width = 20;
        ui.compute_view(cols, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, 30));
        ui.render(&mut buffer);
        let row = render::sidebar_room_row(&ui.snapshot.state, room, ui.room).unwrap() as u16;
        let line: String = (0..ui.view.sidebar.width)
            .map(|x| buffer[(x, row)].symbol().to_string())
            .collect();
        assert!(
            line.contains('●'),
            "{status:?} at {cols} columns hid the dot: {line:?}"
        );
        let dot = (0..ui.view.sidebar.width)
            .find(|&x| buffer[(x, row)].symbol() == "●")
            .unwrap();
        assert_eq!(buffer[(dot, row)].fg, render::ACCENT);
        assert_eq!(ui.view.hit_at(dot, row), Some(&render::Action::Room(room)));
    }
}

#[test]
fn worker_to_orchestrator_picker_lists_and_selects_only_the_bound_orchestrator() {
    let (mut ui, master, room, other, orchestrator) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let outsider = snapshot
        .state
        .create_agent(
            master,
            "other-orch",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    snapshot.state.bind_orchestrator(outsider, other).unwrap();
    let worker = snapshot
        .state
        .create_agent(room, "worker", Provider::Codex, "/repo".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    ui.action(render::Action::Recipients);
    ui.compute_view(100, 30);
    assert!(!hit_rows(&ui, render::Action::Recipient(Some(orchestrator))).is_empty());
    assert!(hit_rows(&ui, render::Action::Recipient(Some(outsider))).is_empty());
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.locals[&room].recipients.contains(&orchestrator));
    assert!(!ui.locals[&room].recipients.contains(&worker));
    ui.action(render::Action::Recipient(None));
    assert_eq!(ui.locals[&room].recipients, [orchestrator, worker].into());
    let mut state = ui.snapshot.state.clone();
    state.set_draft_recipients(room, [orchestrator]).unwrap();
    state.set_draft_text(room, "verify this").unwrap();
    let requests = state.submit_draft(room, 1).unwrap();
    assert_eq!(state.request(requests[0]).unwrap().agent_id, orchestrator);
}

#[test]
fn work_room_picker_all_selects_members_like_cli_to_all() {
    let (mut ui, _, room, _, orchestrator) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let worker = snapshot
        .state
        .create_agent(room, "worker", Provider::Codex, "/repo".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    ui.action(render::Action::Recipient(None));
    assert_eq!(
        ui.locals[&room].recipients,
        [worker].into(),
        "All also selected the MASTER orchestrator {orchestrator:?}"
    );
    ui.action(render::Action::Recipients);
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let all_row = hit_rows(&ui, render::Action::Recipient(None))[0];
    let text: String = (0..100).map(|x| buffer[(x, all_row)].symbol()).collect();
    assert!(text.contains("[x] All"), "{text}");
    ui.action(render::Action::Recipient(None));
    assert!(ui.locals[&room].recipients.is_empty());
    ui.action(render::Action::Recipient(Some(orchestrator)));
    ui.action(render::Action::Recipient(None));
    assert_eq!(ui.locals[&room].recipients, [orchestrator, worker].into());
    ui.action(render::Action::Recipient(None));
    assert_eq!(ui.locals[&room].recipients, [orchestrator].into());
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
    // The agents header names the open room, `# pr-123`, in place of a
    // generic AGENTS heading plus a separate room-name row.
    assert!(
        rows[8].starts_with("# pr-123") && rows[8].ends_with('+'),
        "{rows:?}"
    );
    assert_eq!(rows[9], "", "{rows:?}");
    assert!(rows[10].starts_with("worker"), "{rows:?}");
    assert!(
        !rows
            .iter()
            .any(|row| row.trim() == "MASTER" || row.starts_with("AGENTS")),
        "no MASTER or AGENTS section header: {rows:?}"
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
fn master_sidebar_lists_master_then_rooms_then_its_agents() {
    let (mut ui, master, pr, other, orchestrator) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .create_agent(pr, "worker", Provider::Codex, "/repo".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(master);
    let rows = sidebar_rows(&mut ui);
    assert_eq!(rows[1], "# MASTER", "{rows:?}");
    assert!(
        rows[3].starts_with("ROOMS") && rows[3].ends_with('+'),
        "{rows:?}"
    );
    assert_eq!(rows[5], "# pr-123           Idle", "{rows:?}");
    assert_eq!(rows[6], "# pr-456           Idle", "{rows:?}");
    assert!(rows[7].starts_with('─'), "{rows:?}");
    assert!(
        rows[8].starts_with("# MASTER") && rows[8].ends_with('+'),
        "{rows:?}"
    );
    assert_eq!(rows[9], "", "{rows:?}");
    assert!(rows[10].starts_with("claude-orch"), "{rows:?}");
    assert_eq!(rows[11], "Claude Code", "{rows:?}");
    assert_eq!(rows[12], "# pr-123", "{rows:?}");
    assert!(
        !rows.iter().any(|row| row.starts_with("worker")),
        "{rows:?}"
    );
    assert!(deletable_rooms(&ui).is_empty());
    assert_eq!(hit_rows(&ui, render::Action::Room(master)), [1]);
    assert_eq!(hit_rows(&ui, render::Action::NewRoom), [3]);
    assert_eq!(hit_rows(&ui, render::Action::NewAgent), [8]);
    assert_eq!(
        hit_rows(&ui, render::Action::Agent(orchestrator)),
        [10, 10, 11]
    );
    assert_eq!(hit_rows(&ui, render::Action::Room(pr)), [5, 5, 12]);
    assert_eq!(hit_rows(&ui, render::Action::Room(other)), [6, 6]);
    assert_eq!(
        render::sidebar_room_row(&ui.snapshot.state, other, Some(master)),
        Some(6)
    );
    click(&mut ui, 5);
    assert_eq!(ui.room, Some(pr));
    let rows = sidebar_rows(&mut ui);
    assert!(rows[3].starts_with("ROOMS"), "{rows:?}");
    assert!(rows[8].starts_with("# pr-123"), "{rows:?}");
    assert!(rows[10].starts_with("worker"), "{rows:?}");
    assert!(
        !rows.iter().any(|row| row.starts_with("claude-orch")),
        "{rows:?}"
    );
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
    assert!(rows[10].starts_with("claude-orch"), "{rows:?}");
    assert_eq!(rows[11], "Claude Code", "{rows:?}");
    assert_eq!(rows[12], "# pr-123", "{rows:?}");
    // Even disclosed, no branch or cwd lines follow the orchestrated room.
    assert_eq!(rows[13], "", "{rows:?}");
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
    assert!(hit_row(render::Action::Room(pr)).contains(&12));
    assert!(hit_row(render::Action::Agent(orchestrator)).contains(&11));
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
    snapshot
        .state
        .observe_dialog(
            worker,
            true,
            snapshot.state.agent(worker).unwrap().observed_at_ms,
        )
        .unwrap();
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
    assert!(ui.deletion.is_some());
    assert!(room_screen(&mut ui, 100, 30).contains("The MASTER room cannot be renamed or deleted"));
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::DeleteRoom(_))));
}

#[test]
fn master_room_rename_error_stays_beside_the_name_field() {
    let (mut ui, master, _, _, _) = master_fixture();
    ui.open_room(master);
    key(&mut ui, KeyCode::F(2), KeyModifiers::NONE);
    assert!(ui.rename.is_some());
    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("The MASTER room cannot be"), "{screen}");
    assert!(screen.contains("renamed or deleted"), "{screen}");
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::RenameRoom(_, _))));
}

#[test]
fn repeated_master_navigation_then_opening_agent_never_raises_rename_error() {
    let (mut ui, master, _, _, agent) = master_fixture();
    ui.compute_view(100, 30);
    click(&mut ui, 1);
    ui.compute_view(100, 30);
    click(&mut ui, 1);
    assert_eq!(ui.room, Some(master));
    ui.compute_view(100, 30);
    let agent_row = hit_rows(&ui, render::Action::Agent(agent))[0];
    click(&mut ui, agent_row);
    let screen = room_screen(&mut ui, 100, 30);
    assert_eq!(ui.terminal, Some(agent));
    assert!(ui.rename.is_none());
    assert!(
        ui.visible_error().is_none(),
        "navigation must not try to rename MASTER"
    );
    assert!(!screen.contains("The MASTER room cannot"));
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
    assert!(room_screen(&mut ui, 100, 30).contains("Orchestrates room is required"));
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
        |data| crate::messaging::orchestration::docs_dir(&data),
    );
    crate::messaging::orchestration::fill(
        crate::messaging::orchestration::DEFAULT_PROMPT,
        &crate::messaging::orchestration::PromptValues {
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
    let spec = crate::messaging::orchestration::OrchestratorSpec {
        room: other,
        system_prompt: Some("Run pr-456.".into()),
    };
    ui.receive_event(BusEvent::SetupRequired {
        input: crate::messaging::coordinator::agents::AddAgent {
            room: master,
            name: "codex-orch".into(),
            provider: Provider::Codex,
            cwd: "/repo".into(),
            extra_args: String::new(),
            consent_project_hooks: false,
        },
        orchestrator: Some(spec.clone()),
        notice: crate::agents::providers::launch::SetupNotice {
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
        .any(|hit| hit.action == render::Action::Room(pr) && hit.rect.y == 12));
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
    for expected in ["Name is required.", "Model is required."] {
        assert!(
            screen.contains(expected),
            "validation must explain why Add did not submit in a 24-row terminal: {screen}"
        );
    }
}

#[test]
fn system_prompt_validation_is_inline_with_the_prompt_field() {
    let (mut ui, master, _, _, _) = master_fixture();
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::ClaudeCode));
    if let Some(forms::Form::Agent {
        name, cwd, prompt, ..
    }) = &mut ui.form
    {
        name.insert("orch");
        *cwd = editor::Editor::new("/repo".into());
        prompt.as_mut().unwrap().editor.text.clear();
    }
    ui.action(render::Action::Add);
    ui.compute_view(100, 40);
    let prompt_bottom = ui
        .view
        .hits
        .iter()
        .filter(|hit| hit.action == render::Action::Field(forms::PROMPT_FIELD))
        .map(|hit| hit.rect.bottom())
        .max()
        .unwrap();
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    let error_row = (0..40)
        .find(|&y| {
            let row: String = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
            row.contains("System prompt is required.")
        })
        .expect("inline system prompt error");
    assert!(error_row >= prompt_bottom && error_row <= prompt_bottom + 1);
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
        .submit_message_from(master, draft(orchestrator), Author::Developer, 5)
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
fn bus_signs_its_notices_bus_in_grey_in_work_rooms_and_master() {
    let grey = ratatui::style::Color::Rgb(145, 148, 159);
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    state
        .submit_message_from(
            work,
            Draft {
                text: "builder (agent 3). Blocked, needs help to continue.".into(),
                files: Vec::new(),
                recipient_ids: AgentRecipients::from([orchestrator]),
            },
            Author::Bus,
            5,
        )
        .unwrap();
    state
        .post_developer_attention(orchestrator, "Blocked, needs help to continue.", 6)
        .unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    for color_blind in [false, true] {
        ui.settings.color_blind_mode = color_blind;
        ui.open_room(work);
        assert_eq!(name_colors(&mut ui, "bus → orch", "bus"), [grey; 3]);
        ui.open_room(master);
        assert_eq!(name_colors(&mut ui, "bus → You", "bus"), [grey; 3]);
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
    snapshot
        .state
        .observe_dialog(
            codex,
            true,
            snapshot.state.agent(codex).unwrap().observed_at_ms,
        )
        .unwrap();
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
    snapshot
        .state
        .observe_dialog(
            codex,
            false,
            snapshot.state.agent(codex).unwrap().observed_at_ms,
        )
        .unwrap();
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
    assert_eq!(
        choices(&mut ui),
        [
            Provider::ClaudeCode,
            Provider::Cursor,
            Provider::Grok,
            Provider::Kimi
        ]
    );
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
        ui.form_errors.get(&1).map(String::as_str),
        Some(crate::messaging::orchestration::CODEX_ORCHESTRATOR_REFUSED)
    );
    let screen = room_screen(&mut ui, 120, 40);
    assert!(screen.contains("does not start"), "{screen}");
    assert!(
        screen.contains("a new turn when a background command exits"),
        "{screen}"
    );
    assert!(
        screen.contains("Claude Code or Cursor as the orchestrator"),
        "{screen}"
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
        [
            Provider::Codex,
            Provider::ClaudeCode,
            Provider::Cursor,
            Provider::Grok,
            Provider::Kimi
        ]
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

#[test]
fn agents_header_names_the_selected_room_in_one_spaced_room_styled_truncated_row() {
    let (mut ui, master, pr, _, _) = master_fixture();
    for (room, name) in [(master, "# MASTER"), (pr, "# pr-123")] {
        ui.open_room(room);
        let rows = sidebar_rows(&mut ui);
        // One header row: `# room` at the left, `+` at the right edge, a gap
        // below, and no separate AGENTS or room-name row.
        assert!(
            rows[8].starts_with(name) && rows[8].ends_with('+'),
            "{rows:?}"
        );
        assert_eq!(rows[8].trim_end_matches('+').trim(), name, "{rows:?}");
        assert_eq!(rows[9], "", "{rows:?}");
        assert!(
            !rows.iter().any(|row| row.starts_with("AGENTS")),
            "{rows:?}"
        );
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        // The `#` and name use the same foreground as the ROOMS rows.
        assert_eq!(buffer[(1, 8)].fg, ratatui::style::Color::Rgb(102, 255, 102));
        assert_eq!(buffer[(3, 8)].fg, buffer[(3, 5)].fg);
        // Only the `+` is clickable on the header row.
        let header_hits: Vec<_> = ui.view.hits.iter().filter(|hit| hit.rect.y == 8).collect();
        assert_eq!(header_hits.len(), 1, "{header_hits:?}");
        assert_eq!(header_hits[0].action, render::Action::NewAgent);
        assert_eq!(header_hits[0].rect.width, 1);
        assert!(!ui.view.hits.iter().any(|hit| hit.rect.y == 9));
    }
    let mut snapshot = (*ui.snapshot).clone();
    let mut value = serde_json::to_value(&snapshot.state).unwrap();
    value["rooms"][pr.0.to_string()]["name"] =
        serde_json::json!("a-very-long-room-name-that-overflows");
    snapshot.state = serde_json::from_value(value).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let rows = sidebar_rows(&mut ui);
    // The name truncates short of the `+` so the two never collide.
    assert!(
        rows[8].starts_with("# a-very-long-room-nam…") && rows[8].ends_with(" +"),
        "{rows:?}"
    );
}

#[test]
fn agents_header_respects_configured_and_terminal_capped_sidebar_widths() {
    let (mut ui, _, pr, _, _) = master_fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut value = serde_json::to_value(&snapshot.state).unwrap();
    value["rooms"][pr.0.to_string()]["name"] = serde_json::json!("界".repeat(40));
    snapshot.state = serde_json::from_value(value).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(pr);
    for (configured, cols, width) in [(20, 120, 20), (28, 120, 28), (60, 180, 60), (60, 60, 20)] {
        ui.settings.sidebar_width = configured;
        ui.compute_view(cols, 30);
        assert_eq!(ui.view.sidebar.width, width);
        let plus = ui
            .view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::NewAgent)
            .unwrap();
        assert_eq!(plus.rect, ratatui::layout::Rect::new(width - 3, 8, 1, 1));
        assert_eq!(
            ui.view.hit_at(width - 3, 8),
            Some(&render::Action::NewAgent)
        );
        assert!(ui.view.hit_at(width - 4, 8).is_none());
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, 30));
        ui.render(&mut buffer);
        let header = (1..width - 3)
            .map(|x| buffer[(x, 8)].symbol())
            .collect::<String>();
        assert!(header.starts_with("# 界"), "{header}");
        assert!(header.trim_end().ends_with('…'), "{header}");
        assert_eq!(buffer[(3, 8)].fg, buffer[(3, 5)].fg);

        assert_eq!(buffer[(width - 3, 8)].symbol(), "+");
        assert_eq!(buffer[(width - 4, 8)].symbol(), " ");
        assert_eq!(buffer[(width - 1, 8)].symbol(), "│");
    }
}

#[test]
fn scrolled_master_sidebar_reaches_its_last_orchestrator_below_the_room_name() {
    let (mut ui, master, pr, _, orchestrator) = master_fixture();
    ui.open_room(master);
    // Keep ten content rows when the dev build adds its hash to the footer.
    let rows = 12 + u16::from(cfg!(feature = "dev-tools"));
    ui.compute_view(100, rows);
    // MASTER, gap, ROOMS, gap, two rooms, divider, `# MASTER` header, gap,
    // then a four-row orchestrator card below the leading blank row.
    assert_eq!(ui.view.sidebar_max_scroll, 14 - 10);
    ui.sidebar_scroll = usize::MAX;
    ui.compute_view(100, rows);
    let rows_of = |action| hit_rows(&ui, action);
    assert_eq!(rows_of(render::Action::Agent(orchestrator)), [6, 6, 7]);
    assert_eq!(rows_of(render::Action::Room(pr)), [1, 1, 8]);
    assert_eq!(rows_of(render::Action::NewAgent), [4]);
}

#[test]
fn master_pwd_enter_completes_only_a_visible_highlight_when_more_than_three_suggestions_scroll() {
    let (mut ui, master, _, _, _) = master_fixture();
    ui.open_room(master);
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::ClaudeCode));
    ui.action(render::Action::Field(2));
    if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
        *name = editor::Editor::new("scroll-orch".into());
        *cwd = editor::Editor::new("/projects/f".into());
    }
    let paths =
        ["alpha", "bravo", "charlie", "delta", "echo"].map(|leaf| format!("/projects/f-{leaf}"));
    ui.receive_event(BusEvent::Suggestions {
        query_id: ui.suggestions.query_id,
        result: Ok(paths
            .iter()
            .map(|path| crate::agents::providers::suggest::PathSuggestion {
                path: path.into(),
                is_directory: true,
            })
            .collect()),
    });
    for step in 0..paths.len() {
        let screen = room_screen(&mut ui, 120, 40);
        assert_eq!(ui.suggestions.selected, step);
        assert!(
            screen.contains(&format!("{}/", paths[step])),
            "step {step}:\n{screen}"
        );
        if step + 1 < paths.len() {
            key(&mut ui, KeyCode::Tab, KeyModifiers::NONE);
        }
    }
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(
        matches!(&ui.form, Some(forms::Form::Agent { cwd, field: 2, .. })
        if cwd.text == "/projects/f-echo/")
    );
    assert!(!ui.pending.iter().any(|p| matches!(
        p.command,
        BusCommand::AddAgent(_) | BusCommand::AddOrchestrator(..)
    )));
}

#[test]
fn master_directory_warning_is_visible_and_clears_after_recovery() {
    let (mut ui, master, pr, _, agent) = master_fixture();
    let identity = ui
        .snapshot
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .clone();
    let warning = "Working directory unavailable: /gone. Restore worktree or use --pwd.";
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .observe_working_directory_error(agent, Some(warning.into()))
        .unwrap();
    ui.snapshot = Arc::new(snapshot);
    ui.open_room(master);
    let text = sidebar_rows(&mut ui).join(" ");
    for expected in ["/gone", "Restore worktree", "--pwd"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!hit_rows(&ui, render::Action::Room(pr)).is_empty());
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .observe_working_directory_error(agent, None)
        .unwrap();
    ui.snapshot = Arc::new(snapshot);
    assert!(!sidebar_rows(&mut ui).join(" ").contains("/gone"));
    assert_eq!(
        ui.snapshot.state.agent(agent).unwrap().runtime_identity,
        identity
    );
}
