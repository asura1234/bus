use super::*;
use ratatui::{buffer::Buffer, layout::Rect};

fn picker(master: bool, count: usize) -> (BusUi, RoomId, AgentId) {
    let mut state = BusState::default();
    let room = if master {
        state.ensure_master_room()
    } else {
        state.create_room("work").unwrap()
    };
    let mut first = None;
    for index in 0..count {
        let agent = state
            .create_agent(
                room,
                &format!("agent-{index:02}"),
                Provider::ClaudeCode,
                "/project".into(),
                None,
            )
            .unwrap();
        if master {
            let work = state.create_room(&format!("work-{index:02}")).unwrap();
            state.bind_orchestrator(agent, work).unwrap();
        }
        first.get_or_insert(agent);
    }
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    ui.open_room(room);
    (ui, room, first.unwrap())
}

fn paint(ui: &mut BusUi, cols: u16, rows: u16) -> Buffer {
    ui.compute_view(cols, rows);
    let mut buffer = Buffer::empty(Rect::new(0, 0, cols, rows));
    ui.render(&mut buffer);
    buffer
}

fn choices(ui: &BusUi) -> Vec<Rect> {
    ui.view
        .hits
        .iter()
        .filter(|hit| matches!(hit.action, render::Action::Recipient(_)))
        .map(|hit| hit.rect)
        .collect()
}

fn assert_panel(ui: &BusUi, buffer: &Buffer) {
    let rows = choices(ui);
    assert!(!rows.is_empty(), "picker must retain a visible choice");
    let first = rows.first().unwrap();
    let last = rows.last().unwrap();
    assert!(first.x > 0 && first.y > 0 && last.bottom() < buffer.area.bottom());
    assert_eq!(
        buffer[(first.x - 1, first.y - 1)].symbol(),
        "┌",
        "picker must own a framed, cleared area above its rows"
    );
    assert_eq!(buffer[(first.right(), first.y - 1)].symbol(), "┐");
    assert_eq!(buffer[(last.x - 1, last.bottom())].symbol(), "└");
    assert_eq!(buffer[(last.right(), last.bottom())].symbol(), "┘");
    for row in rows {
        assert_eq!(row.intersection(buffer.area), row);
        assert_eq!(buffer[(row.x - 1, row.y)].symbol(), "│");
        assert_eq!(buffer[(row.right(), row.y)].symbol(), "│");
        if row.width > 50 {
            assert_eq!(
                buffer[(row.right() - 1, row.y)].symbol(),
                " ",
                "history suffix must be erased"
            );
        }
    }
}

#[test]
fn recipient_menu_owns_its_area_for_master_work_short_long_and_small_terminals() {
    for master in [true, false] {
        for count in [2, 30] {
            for (cols, rows) in [(120, 40), (42, 14), (30, 8)] {
                let (mut ui, room, agent) = picker(master, count);
                saved_history(&mut ui, room, agent, 15);
                ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
                key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
                for _ in 0..=count {
                    let buffer = paint(&mut ui, cols, rows);
                    assert_panel(&ui, &buffer);
                    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
                }
                key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
                assert!(ui.locals[&room].recipients.len() >= 2);
                let buffer = paint(&mut ui, cols, rows);
                assert_panel(&ui, &buffer);
            }
        }
    }
}

#[test]
fn recipient_menu_wheel_scrolls_choices_instead_of_history() {
    use crossterm::event::MouseEventKind::ScrollDown;
    let (mut ui, room, agent) = picker(true, 30);
    saved_history(&mut ui, room, agent, 25);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    paint(&mut ui, 100, 30);
    let history = ui.main_scroll;
    let row = choices(&ui)[1];
    mouse(&mut ui, ScrollDown, row.x, row.y);
    assert_eq!(
        ui.recipient_index, 3,
        "wheel over picker must navigate its own list"
    );
    assert_eq!(
        ui.main_scroll, history,
        "picker must consume its wheel events"
    );
}

#[test]
fn recipient_menu_clamps_a_stale_highlight_before_rendering_a_shorter_list() {
    let (mut ui, master, _) = picker(true, 24);
    let mut snapshot = (*ui.snapshot).clone();
    let work = snapshot.state.create_room("busy-work").unwrap();
    for index in 0..50 {
        snapshot
            .state
            .create_agent(
                work,
                &format!("worker-{index:02}"),
                Provider::ClaudeCode,
                "/project".into(),
                None,
            )
            .unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(work);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    for _ in 0..44 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    paint(&mut ui, 100, 30);
    ui.open_room(master);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    let buffer = paint(&mut ui, 100, 30);
    assert!(
        choices(&ui).len() > 1,
        "a stale long-list index must not leave a lone row over history"
    );
    assert_panel(&ui, &buffer);
}

#[test]
fn recipient_menu_survives_history_updates_and_full_composer_and_clamps_roster_changes() {
    use crossterm::event::MouseEventKind::ScrollUp;
    let (mut ui, room, agent) = picker(true, 30);
    saved_history(&mut ui, room, agent, 20);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    for _ in 0..30 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    let buffer = paint(&mut ui, 100, 30);
    assert_panel(&ui, &buffer);
    let history = ui.view.history;
    mouse(&mut ui, ScrollUp, history.x, history.y);
    let buffer = paint(&mut ui, 100, 30);
    assert_panel(&ui, &buffer);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(room, agent, "NEW HISTORY".into(), vec![], 100_000)
        .unwrap();
    // A shrinking roster must clamp the retained viewport as well as a room switch.
    let removed: Vec<_> = snapshot
        .state
        .agents()
        .filter(|a| a.room_id == room && a.id != agent)
        .map(|a| a.id)
        .collect();
    for id in removed {
        snapshot.state.delete_agent(id, 100_001).unwrap();
    }
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let buffer = paint(&mut ui, 100, 30);
    assert_eq!(choices(&ui).len(), 2);
    assert_panel(&ui, &buffer);
    let local = ui.locals.get_mut(&room).unwrap();
    local.text = editor::Editor::new("Draft preserved\n".repeat(40));
    local.composer_size = ComposerSize::Full;
    let buffer = paint(&mut ui, 42, 14);
    assert_panel(&ui, &buffer);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].text.text, "Draft preserved\n".repeat(40));
    assert!(ui.view.thumbnails.is_empty());
}
