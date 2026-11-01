use super::*;

#[test]
fn expanded_worker_details_show_the_complete_startup_failure_and_recovery() {
    let (mut ui, _, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.confirm_hook_setup(agent).unwrap();
    snapshot
        .state
        .observe_status(agent, RuntimeStatus::Unavailable, 1)
        .unwrap();
    let error = "Startup failed: provider exited before confirmation.\nNo conversation found with session ID: missing.\nRecovery: delete-and-recreate with launch settings and system prompt; rebrief retained work.";
    snapshot
        .state
        .set_agent_error(agent, Some(error.into()))
        .unwrap();
    snapshot
        .state
        .set_agent_startup(
            agent,
            Some(crate::messaging::model::AgentStartup {
                command: "claude --resume missing".into(),
                started_at_ms: 1,
                reported: true,
                native_terminal_id: Some("failed-terminal".into()),
                recovery: None,
                failure: None,
            }),
        )
        .unwrap();
    snapshot
        .state
        .set_agent_details_disclosed(agent, true)
        .unwrap();
    let second = snapshot
        .state
        .create_agent(
            ui.room.unwrap(),
            "second",
            Provider::Cursor,
            "/other".into(),
            None,
        )
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot.clone()));
    for cols in [80, 160] {
        ui.sidebar_scroll = 0;
        ui.compute_view(cols, 24);
        let maximum = ui.view.sidebar_max_scroll;
        let mut seen = String::new();
        for scroll in 0..=maximum {
            ui.sidebar_scroll = scroll;
            ui.compute_view(cols, 24);
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, 24));
            ui.render(&mut buffer);
            for y in ui.view.sidebar_body.y..ui.view.sidebar_body.bottom() {
                let line = (1..ui.view.sidebar.right() - 1)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>();
                seen.push_str(line.trim());
            }
        }
        for text in [
            "Startup failed:",
            "No conversation found",
            "delete-and-recreate",
            "system prompt",
            "rebrief retained work.",
        ] {
            // Word wrapping removes only the row boundary, never the content.
            assert!(
                seen.replace(' ', "").contains(&text.replace(' ', "")),
                "{cols}: missing {text}: {seen}"
            );
        }
        assert!(
            ui.view
                .hits
                .iter()
                .any(|hit| hit.action == render::Action::Agent(second)),
            "following agent remains reachable after scrolling"
        );
    }
    snapshot.state.set_agent_error(agent, None).unwrap();
    snapshot.state.set_agent_startup(agent, None).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.sidebar_scroll = 0;
    assert!(
        !room_screen(&mut ui, 160, 24).contains("Startup failed"),
        "confirmed recovery removes the diagnosis rows"
    );
}

#[test]
fn selected_agent_names_keep_minimum_text_contrast_on_the_selection_tint() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};
    // WCAG 2.2 contrast ratio. 3:1 is the lowest bar WCAG sets for any text
    // (large text); every allocatable agent color met it on the pre-#73
    // selected background, RGB(46, 48, 58).
    fn contrast(a: Color, b: Color) -> f64 {
        let luminance = |color: Color| {
            let Color::Rgb(r, g, b) = color else {
                panic!("rgb expected: {color:?}")
            };
            let [r, g, b] = [r, g, b].map(|channel| {
                let channel = f64::from(channel) / 255.0;
                if channel <= 0.04045 {
                    channel / 12.92
                } else {
                    ((channel + 0.055) / 1.055).powf(2.4)
                }
            });
            0.2126 * r + 0.7152 * g + 0.0722 * b
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    let (mut ui, room, first) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut agents = vec![first];
    for index in 1..8 {
        agents.push(
            snapshot
                .state
                .create_agent(
                    room,
                    &format!("agent{index}"),
                    Provider::Codex,
                    "/project".into(),
                    None,
                )
                .unwrap(),
        );
    }
    ui.receive_snapshot(Arc::new(snapshot));
    let mut unreadable = Vec::new();
    let mut selected_background = Color::Reset;
    for color_blind in [false, true] {
        ui.settings.color_blind_mode = color_blind;
        for &agent in &agents {
            ui.open_room(room);
            ui.terminal = Some(agent);
            ui.compute_view(100, 60);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 60));
            ui.render(&mut buffer);
            let hit = ui
                .view
                .hits
                .iter()
                .find(|hit| hit.action == render::Action::Agent(agent))
                .unwrap();
            let cell = &buffer[(hit.rect.x, hit.rect.y)];
            selected_background = cell.bg;
            let ratio = contrast(cell.fg, cell.bg);
            if ratio < 3.0 {
                unreadable.push(format!(
                    "color_blind={color_blind} fg={:?} bg={:?} ratio={ratio:.2}",
                    cell.fg, cell.bg
                ));
            }
        }
    }
    // Allocation searches this entire RGB grid. Cover every possible identity
    // color, including the accessible subset, beyond the first eight agents.
    for r in (0..=255).step_by(17) {
        for g in (0..=255).step_by(17) {
            for b in (0..=255).step_by(17) {
                if crate::messaging::prefs::colors::is_agent_color([r, g, b]) {
                    let ratio = contrast(Color::Rgb(r, g, b), selected_background);
                    assert!(ratio >= 3.0, "{r},{g},{b} contrast={ratio:.2}");
                }
            }
        }
    }
    assert!(unreadable.is_empty(), "{unreadable:#?}");
}

#[test]
fn selected_sidebar_rows_have_accent_bar_bold_name_and_tinted_background() {
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Modifier},
    };
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let master = snapshot.state.ensure_master_room();
    ui.receive_snapshot(Arc::new(snapshot));
    for color_blind in [false, true] {
        ui.settings.color_blind_mode = color_blind;
        for selected in [
            render::Action::Room(master),
            render::Action::Room(room),
            render::Action::Agent(agent),
        ] {
            ui.open_room(if selected == render::Action::Room(master) {
                master
            } else {
                room
            });
            if selected == render::Action::Agent(agent) {
                ui.terminal = Some(agent);
            }
            ui.compute_view(100, 30);
            let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 30));
            ui.render(&mut buffer);
            for action in [
                render::Action::Room(master),
                render::Action::Room(room),
                render::Action::Agent(agent),
            ] {
                let Some(hit) = ui.view.hits.iter().find(|hit| hit.action == action) else {
                    continue;
                };
                let rect = hit.rect;
                assert_eq!(rect.x, 1, "names retain their column");
                let is_selected = action == selected;
                assert_eq!(
                    buffer[(0, rect.y)].symbol(),
                    if is_selected { "▌" } else { " " }
                );
                assert_eq!(
                    buffer[(rect.x, rect.y)].modifier.contains(Modifier::BOLD),
                    is_selected
                );
                assert_eq!(
                    buffer[(rect.x, rect.y)].bg,
                    if is_selected {
                        Color::Rgb(28, 52, 34)
                    } else {
                        Color::Rgb(24, 24, 28)
                    }
                );
                if is_selected {
                    assert_eq!(buffer[(0, rect.y)].fg, Color::Rgb(102, 255, 102));
                }
                if action == render::Action::Agent(agent) {
                    let agent = ui.snapshot.state.agent(agent).unwrap();
                    let [r, g, b] = if color_blind {
                        agent.accessible_color
                    } else {
                        agent.color
                    };
                    assert_eq!(buffer[(rect.x, rect.y)].fg, Color::Rgb(r, g, b));
                } else {
                    assert_eq!(buffer[(rect.x + 2, rect.y)].fg, Color::Rgb(222, 222, 226));
                }
            }
        }
    }
}

#[test]
fn sidebar_errors_never_add_agent_rows_or_scroll_height() {
    // Keep the content viewport identical in both build variants.
    let rows = 14 + u16::from(cfg!(feature = "dev-tools"));
    for (invalidated, deleting) in [(false, false), (true, false), (false, true)] {
        let (mut ui, room, agent) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        let second = snapshot
            .state
            .create_agent(room, "second", Provider::Cursor, "/other".into(), None)
            .unwrap();
        ui.receive_snapshot(Arc::new(snapshot.clone()));
        let before = room_screen(&mut ui, 100, rows);
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
        let after = room_screen(&mut ui, 100, rows);
        assert_eq!(sidebar(&after), sidebar(&before));
        assert_eq!(ui.view.sidebar_max_scroll, max_scroll);
        assert!(ui
            .view
            .hits
            .iter()
            .any(|hit| { hit.action == render::Action::Agent(second) && hit.rect.y == 11 }));
    }
}

#[test]
fn expanded_sidebar_omits_absent_branch_without_a_placeholder_row() {
    let rows = 16 + u16::from(cfg!(feature = "dev-tools"));
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
    ui.compute_view(100, rows);
    assert_eq!(ui.view.sidebar_max_scroll, 3);
    ui.sidebar_scroll = usize::MAX;
    ui.compute_view(100, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, rows));
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
    let divider_y = 28 - u16::from(cfg!(feature = "dev-tools"));
    for x in 1..right {
        assert_eq!(buffer[(x, divider_y)].symbol(), "─", "divider at x={x}");
        assert_eq!(buffer[(x, divider_y)].fg, green, "divider at x={x}");
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
fn sidebar_build_hash_is_dev_only_directly_above_settings() {
    let (mut ui, _, _) = fixture();
    let (buffer, y, right) = settings_row(&mut ui, 100, 30);
    let above = (1..right)
        .map(|x| buffer[(x, y - 1)].symbol())
        .collect::<String>();
    if cfg!(feature = "dev-tools") {
        let hash = option_env!("BUS_BUILD_SHORT_SHA").unwrap_or("missing-build-hash");
        assert_eq!(above.trim_end(), hash);
        for x in 1..=hash.len() as u16 {
            assert_eq!(
                buffer[(x, y - 1)].fg,
                ratatui::style::Color::Rgb(145, 148, 159)
            );
            assert_eq!(ui.view.hit_at(x, y - 1), None, "hash is a plain label");
        }
        assert_eq!(ui.view.sidebar_body.bottom(), y - 2);
    } else {
        assert!(
            above.chars().all(|ch| ch == '─'),
            "non-dev footer has no hash: {above}"
        );
        assert_eq!(ui.view.sidebar_body.bottom(), y - 1);
    }
}

fn settings_row(ui: &mut BusUi, cols: u16, rows: u16) -> (ratatui::buffer::Buffer, u16, u16) {
    ui.compute_view(cols, rows);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    // The footer rows span the sidebar minus its left pad and divider.
    let row_right = 1 + ui.view.sidebar.width - 3;
    (buffer, rows - 1, row_right)
}

#[test]
fn yolo_badge_sits_at_the_settings_row_right_edge_in_black_on_red() {
    let (mut ui, _, _) = fixture();
    ui.yolo = true;
    for cols in [100, 60] {
        let (buffer, y, right) = settings_row(&mut ui, cols, 30);
        let badge = right - 4..right;
        assert_eq!(
            badge
                .clone()
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>(),
            "YOLO",
            "cols={cols}"
        );
        for x in badge.clone() {
            assert_eq!(
                buffer[(x, y)].fg,
                ratatui::style::Color::Black,
                "cols={cols} x={x}"
            );
            assert_eq!(
                buffer[(x, y)].bg,
                ratatui::style::Color::Rgb(255, 0, 0),
                "cols={cols} x={x}"
            );
            assert_eq!(ui.view.hit_at(x, y), None, "badge is a plain label");
        }
        assert_eq!(
            buffer[(badge.start - 1, y)].symbol(),
            " ",
            "gap before the badge"
        );
        let settings = ui
            .view
            .hits
            .iter()
            .find(|hit| hit.action == render::Action::Settings)
            .expect("settings button")
            .rect;
        assert_eq!((settings.x, settings.y), (1, y));
        assert_eq!(
            (settings.x..settings.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>(),
            "Settings"
        );
    }
    assert!(ui.form.is_none());
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        2,
        29,
    );
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        2,
        29,
    );
    assert!(matches!(ui.form, Some(forms::Form::Settings)));
}

#[test]
fn without_yolo_the_settings_row_shows_no_badge() {
    let (mut ui, _, _) = fixture();
    let (buffer, y, right) = settings_row(&mut ui, 100, 30);
    let row = (1..right)
        .map(|x| buffer[(x, y)].symbol())
        .collect::<String>();
    assert_eq!(row.trim_end(), "Settings");
    for x in right - 4..right {
        assert_ne!(buffer[(x, y)].bg, ratatui::style::Color::Rgb(255, 0, 0));
    }
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
    assert!(
        crate::messaging::prefs::settings::load(&path)
            .unwrap()
            .color_blind_mode
    );
    assert_eq!(name_color(&mut ui), accessible);

    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert_eq!(ui.room, Some(room));
    assert_eq!(name_color(&mut ui), accessible);

    ui.action(render::Action::Settings);
    ui.action(render::Action::ToggleColorBlindMode);
    assert!(
        !crate::messaging::prefs::settings::load(&path)
            .unwrap()
            .color_blind_mode
    );
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
        // The agents header names the open room in the ROOMS `#` style.
        assert!(
            row_text(4 + count * 2).starts_with("# room"),
            "{}",
            row_text(4 + count * 2)
        );
        let color = buffer[(27, 0)].fg;
        for x in 1..26 {
            assert_eq!(buffer[(x, 3 + count * 2)].symbol(), "─");
            assert_eq!(buffer[(x, 3 + count * 2)].fg, color);
        }
        assert_eq!(buffer[(25, 1)].symbol(), "+");
        assert_eq!(buffer[(25, 4 + count * 2)].symbol(), "+");
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
        (1, 5, "─"),                       // Rooms/agents separator.
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
        3,
        "room row, agents header and room heading"
    );
    for required in ["ROOMS", "# room", "author", "Codex", "Quote"] {
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

#[test]
fn sidebar_build_hash_keeps_save_failure_notices_visible() {
    let (mut ui, _, _) = fixture();
    ui.force_exit_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    let (buffer, y, right) = settings_row(&mut ui, 100, 30);
    let extra = u16::from(cfg!(feature = "dev-tools"));
    let text = |row| {
        (1..right)
            .map(|x| buffer[(x, row)].symbol())
            .collect::<String>()
    };
    assert!(text(y - 3 - extra).starts_with("Ctrl+Q again within 5s"));
    assert!(text(y - 2 - extra).starts_with("May lose unsaved edits"));
    assert_eq!(ui.view.sidebar_body.bottom(), y - 3 - extra);
}

#[test]
fn each_room_has_a_right_aligned_working_agent_count() {
    let (mut ui, room, first) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.confirm_hook_setup(first).unwrap();
    snapshot
        .state
        .observe_status(first, RuntimeStatus::Working, 1)
        .unwrap();
    for (name, status) in [
        ("blocked", RuntimeStatus::Blocked),
        ("idle", RuntimeStatus::Idle),
        ("unavailable", RuntimeStatus::Unavailable),
    ] {
        let id = snapshot
            .state
            .create_agent(room, name, Provider::Codex, "/project".into(), None)
            .unwrap();
        snapshot.state.confirm_hook_setup(id).unwrap();
        snapshot.state.observe_status(id, status, 1).unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    let row = render::sidebar_room_row(&ui.snapshot.state, room, ui.room).unwrap() as u16 + 1;
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let line: String = (0..ui.view.sidebar.width)
        .map(|x| buffer[(x, row)].symbol())
        .collect();
    assert!(line.starts_with(" Agents"), "{line}");
    let count = line.find("1/4").expect("active/total count");
    assert_eq!(count + 3, ui.view.sidebar.width as usize - 2, "{line}");
    assert!(!line.contains("stalled"), "{line}");
}
