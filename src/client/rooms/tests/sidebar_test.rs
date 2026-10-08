use super::*;

#[test]
fn sidebar_errors_never_add_agent_rows_or_scroll_height() {
    for (invalidated, deleting) in [(false, false), (true, false), (false, true)] {
        let (mut ui, room, agent) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        let second = snapshot
            .state
            .create_agent(room, "second", Provider::Cursor, "/other".into(), None)
            .unwrap();
        ui.receive_snapshot(Arc::new(snapshot.clone()));
        let before = room_screen(&mut ui, 100, 14);
        let sidebar_width = usize::from(ui.view.sidebar.width);
        let sidebar = |screen: &str| {
            screen
                .chars()
                .collect::<Vec<_>>()
                .chunks(100)
                .map(|line| line.iter().take(sidebar_width).collect::<String>())
                .collect::<Vec<_>>()
        };
        let max_scroll = ui.view.sidebar_max_scroll;
        snapshot
            .state
            .set_agent_error(agent, Some("Setup required".into()))
            .unwrap();
        if invalidated {
            snapshot.state.invalidate_agent_session(agent).unwrap();
        }
        if deleting {
            snapshot.state.prepare_delete_agent(agent).unwrap();
        }
        ui.receive_snapshot(Arc::new(snapshot));
        let after = room_screen(&mut ui, 100, 14);
        assert_eq!(sidebar(&after), sidebar(&before));
        assert_eq!(ui.view.sidebar_max_scroll, max_scroll);
        assert!(ui
            .view
            .hits
            .iter()
            .any(|hit| { hit.action == render::Action::Agent(second) && hit.rect.y == 10 }));
    }
}

#[test]
fn expanded_sidebar_omits_absent_branch_without_a_placeholder_row() {
    let (mut ui, room, first) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot
        .state
        .create_agent(room, "second", Provider::Cursor, "/other".into(), None)
        .unwrap();
    for agent in [first, second] {
        snapshot
            .state
            .set_agent_details_disclosed(agent, true)
            .unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 16);
    assert_eq!(ui.view.sidebar_max_scroll, 2);
    ui.sidebar_scroll = usize::MAX;
    ui.compute_view(100, 16);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 16));
    ui.render(&mut buffer);
    let row_text = |y| {
        (1..ui.view.sidebar.right() - 1)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    };
    for (agent, branch, pwd) in [(first, Some("main"), "/project"), (second, None, "/other")] {
        let details = ui
            .view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::Details(agent))
            .unwrap();
        let mut y = details.rect.y + 1;
        if let Some(branch) = branch {
            assert!(row_text(y).starts_with(branch));
            y += 1;
        }
        assert!(row_text(y).starts_with(pwd));
        assert!(row_text(y + 1).trim().is_empty());
    }
}

#[test]
fn sidebar_pins_green_divider_and_settings_button_to_lower_left() {
    let (mut ui, _, _) = fixture();
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let green = ratatui::style::Color::Rgb(102, 255, 102);
    let right = ui.view.sidebar.right() - 2;
    for x in 1..right {
        assert_eq!(buffer[(x, 28)].symbol(), "─", "divider at x={x}");
        assert_eq!(buffer[(x, 28)].fg, green, "divider at x={x}");
    }
    let settings = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Settings)
        .expect("settings button")
        .rect;
    assert_eq!((settings.x, settings.y), (1, 29));
    assert_eq!(
        (settings.x..settings.right())
            .map(|x| buffer[(x, 29)].symbol())
            .collect::<String>(),
        "Settings"
    );
}

#[test]
fn color_blind_mode_defaults_off_and_switches_agent_identity_colors() {
    let (mut ui, room, agent) = fixture();
    let rgb = |[r, g, b]: [u8; 3]| ratatui::style::Color::Rgb(r, g, b);
    let (standard, accessible) = {
        let agent = ui.snapshot.state.agent(agent).unwrap();
        (rgb(agent.color), rgb(agent.accessible_color))
    };
    assert_ne!(standard, accessible);
    let name_color = |ui: &mut BusUi| {
        ui.compute_view(100, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let name = ui
            .view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::Agent(agent))
            .expect("sidebar agent name")
            .rect;
        buffer[(name.x, name.y)].fg
    };
    let path = std::env::temp_dir().join(format!(
        "bus-ui-settings-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    ui.settings_path = Some(path.clone());
    assert!(!ui.settings.color_blind_mode);
    assert_eq!(name_color(&mut ui), standard);

    ui.action(render::Action::Settings);
    assert!(matches!(ui.form, Some(forms::Form::Settings)));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.settings.color_blind_mode);
    assert!(crate::bus::settings::load(&path).unwrap().color_blind_mode);
    assert_eq!(name_color(&mut ui), accessible);

    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert_eq!(ui.room, Some(room));
    assert_eq!(name_color(&mut ui), accessible);

    ui.action(render::Action::Settings);
    ui.action(render::Action::ToggleColorBlindMode);
    assert!(!crate::bus::settings::load(&path).unwrap().color_blind_mode);
    assert_eq!(name_color(&mut ui), standard);
    let _ = std::fs::remove_file(path);
}

#[test]
fn bus_sidebar_working_adds_frames_without_speeding_up() {
    let (mut ui, _, agent) = fixture();
    set_sidebar_agent_status(&mut ui, agent, RuntimeStatus::Working);
    assert!(!ui.tick(), "first visible frame starts the animation clock");

    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Working"),
        vec![
            ratatui::style::Color::Rgb(102, 255, 102),
            ratatui::style::Color::Rgb(68, 190, 84),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
        ]
    );

    ui.status_animation_last_tick =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(110));
    assert!(ui.tick(), "100 ms should render an intermediate frame");
    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Working"),
        vec![
            ratatui::style::Color::Rgb(85, 223, 93),
            ratatui::style::Color::Rgb(85, 223, 93),
            ratatui::style::Color::Rgb(52, 151, 68),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
        ]
    );

    ui.status_animation_last_tick =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(110));
    assert!(ui.tick(), "200 ms should reach the next original keyframe");
    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Working"),
        vec![
            ratatui::style::Color::Rgb(68, 190, 84),
            ratatui::style::Color::Rgb(102, 255, 102),
            ratatui::style::Color::Rgb(68, 190, 84),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
            ratatui::style::Color::Rgb(36, 112, 52),
        ]
    );
}

#[test]
fn bus_sidebar_blocked_adds_frames_without_speeding_up() {
    let (mut ui, _, agent) = fixture();
    set_sidebar_agent_status(&mut ui, agent, RuntimeStatus::Blocked);
    assert!(!ui.tick(), "first visible frame starts the animation clock");

    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Blocked"),
        vec![ratatui::style::Color::Rgb(128, 44, 52); 7]
    );

    ui.status_animation_last_tick =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(110));
    assert!(ui.tick(), "100 ms should render an intermediate frame");
    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Blocked"),
        vec![ratatui::style::Color::Rgb(167, 54, 62); 7]
    );

    ui.status_animation_last_tick =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(110));
    assert!(ui.tick(), "200 ms should reach the next original keyframe");
    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Blocked"),
        vec![ratatui::style::Color::Rgb(205, 64, 72); 7]
    );
}

#[test]
fn bus_sidebar_blocked_does_not_jump_when_the_shared_phase_wraps() {
    let (mut ui, _, agent) = fixture();
    set_sidebar_agent_status(&mut ui, agent, RuntimeStatus::Blocked);
    assert!(!ui.tick(), "first visible frame starts the animation clock");

    ui.status_animation_phase = 41;
    ui.status_animation_last_tick =
        Some(std::time::Instant::now() - std::time::Duration::from_millis(110));
    assert!(ui.tick(), "the next 100 ms frame should repaint");
    assert_eq!(
        rendered_agent_status_colors(&mut ui, agent, "Blocked"),
        vec![ratatui::style::Color::Rgb(255, 92, 102); 7]
    );
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
fn sidebar_calls_an_unconfirmed_idle_agent_not_ready() {
    let (mut ui, _, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut value = serde_json::to_value(&snapshot.state).unwrap();
    value["agents"][agent.0.to_string()]["hook_setup_confirmed"] = serde_json::json!(false);
    snapshot.state = serde_json::from_value(value).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let sidebar: Vec<String> = (0..ui.view.sidebar.height)
        .map(|y| {
            (0..ui.view.sidebar.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect()
        })
        .collect();
    // The room row shows its own status; the agent row never says Idle.
    let agent_row = sidebar
        .iter()
        .find(|row| row.contains("author"))
        .expect("agent row");
    assert!(agent_row.contains("Not ready"), "{sidebar:?}");
    assert!(!agent_row.contains("Idle"), "{sidebar:?}");
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
        (29, 5, "─"),                      // History separator under one-row notes.
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
