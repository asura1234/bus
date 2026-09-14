use super::*;
use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};

// Persisted request fixtures: all prompts and finals already exist in this
// format, even though the original UI only displayed the latest caches.
fn saved_history(ui: &mut BusUi, room: RoomId, agent: AgentId, count: usize) -> Vec<RequestId> {
    let mut snapshot = (*ui.snapshot).clone();
    let mut requests = Vec::new();
    for i in 0..count {
        snapshot.state.set_draft_recipients(room, [agent]).unwrap();
        snapshot
            .state
            .set_draft_text(room, &format!("prompt-{i:02}"))
            .unwrap();
        requests.push(
            snapshot
                .state
                .submit_draft(room, 1000 + i as u64 * 1000)
                .unwrap()[0],
        );
    }
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (i, id) in requests.iter().enumerate() {
        let record = &mut json["requests"][id.0.to_string()];
        record["phase"] = "completed".into();
        record["completed_at_ms"] = (1500 + i as u64 * 1000).into();
        record["pending_final"] = serde_json::json!({
            "callback_id": format!("final-{i}"), "text": format!("answer-{i:02}"),
            "received_at_ms": 1500 + i as u64 * 1000,
            "provider_session_id": "session", "provider_turn_id": format!("turn-{i}")
        });
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    requests
}

#[test]
fn saved_round_trips_scroll_oldest_to_newest_with_fixed_chrome() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 20);
    let bottom = room_screen(&mut ui, 100, 30);
    assert!(
        bottom.contains("answer-19"),
        "newest reply is visible on opening"
    );
    assert!(!bottom.contains("prompt-00"));
    let composer = composer_rect(&ui);
    let mut seen = String::new();
    for _ in 0..=ui.view.history_max_scroll {
        seen.push_str(&room_screen(&mut ui, 100, 30));
        mouse(&mut ui, ScrollUp, 40, 12);
        ui.compute_view(100, 30);
        assert_eq!(composer_rect(&ui), composer);
    }
    let top = room_screen(&mut ui, 100, 30);
    assert!(top.find("prompt-00").unwrap() < top.find("answer-00").unwrap());
    for i in 0..20 {
        assert!(seen.contains(&format!("prompt-{i:02}")));
        assert!(seen.contains(&format!("answer-{i:02}")));
    }
    // New arrivals do not yank the viewport away while reading older messages.
    saved_history(&mut ui, room, agent, 1);
    let after = room_screen(&mut ui, 100, 30);
    assert!(after.contains("prompt-00"));
    assert_eq!(ui.main_scroll, 0);
    for _ in 0..=ui.view.history_max_scroll {
        mouse(&mut ui, ScrollDown, 40, 12);
        ui.compute_view(100, 30);
    }
    assert_eq!(ui.main_scroll, ui.view.history_max_scroll);
    ui.open_room(room);
    assert!(room_screen(&mut ui, 100, 30).contains("answer-19"));
}

#[test]
fn quote_uses_the_clicked_historical_reply() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 3);
    ui.compute_view(100, 80);
    let action = ui
        .view
        .hits
        .iter()
        .find(|hit| matches!(hit.action, render::Action::Quote(_)))
        .unwrap()
        .action
        .clone();
    ui.action(action);
    assert_eq!(ui.locals[&room].text.text, "author: \"answer-00\"\n");
    assert!(!ui.locals[&room].text.text.contains("answer-02"));
}

#[test]
fn quote_after_selecting_draft_text_appends_without_replacing_the_selection() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::Down};
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 1);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("keep all of this");
    ui.compute_view(100, 80);
    let rect = composer_rect(&ui);
    assert_eq!(
        drag_copy(&mut ui, (rect.x + 5, rect.y), (rect.x + 7, rect.y)).as_deref(),
        Some("all")
    );
    let quote = ui
        .view
        .hits
        .iter()
        .find(|hit| matches!(hit.action, render::Action::Quote(_)))
        .unwrap()
        .rect;
    assert_eq!(pointer(&mut ui, Down(Left), quote.x, quote.y), None);
    assert_eq!(
        ui.locals[&room].text.text,
        "keep all of this\nauthor: \"answer-00\"\n"
    );
}

#[test]
fn selected_agent_rectangles_wrap_whole_and_keep_every_name() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut agents = vec![agent];
    let mut names = vec!["author".to_owned()];
    for i in 1..9 {
        let name = format!("reviewer-{i:02}");
        agents.push(
            snapshot
                .state
                .create_agent(room, &name, Provider::Codex, "/project".into(), None)
                .unwrap(),
        );
        names.push(name);
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().recipients = agents.into_iter().collect();
    let screen = room_screen(&mut ui, 76, 60);
    let cells: Vec<_> = screen.chars().collect();
    let right_lines: Vec<_> = cells
        .chunks(76)
        .map(|line| line.iter().skip(25).collect::<String>())
        .collect();
    for name in names {
        assert!(
            right_lines.iter().any(|line| line.contains(&name)),
            "whole selected name {name} missing from composer"
        );
    }
    assert!(
        right_lines.iter().filter(|line| line.contains('╭')).count() >= 3,
        "chips must wrap across multiple rows"
    );
    assert!(!screen.contains("9 selected"));
}

#[test]
fn overflowing_recipients_preserve_history_and_every_chip_is_reachable() {
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut agents = Vec::new();
    for i in 0..24 {
        agents.push(
            snapshot
                .state
                .create_agent(
                    room,
                    &format!("reviewer-{i:02}"),
                    Provider::Codex,
                    "/project".into(),
                    None,
                )
                .unwrap(),
        );
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().recipients = agents.into_iter().collect();
    ui.compute_view(76, 30);
    assert!(
        ui.view.history.height >= 3,
        "selecting many recipients must not erase chat history"
    );
    let history = ui.view.history;
    let composer = ui.view.composer;
    let mut seen = String::new();
    for _ in 0..30 {
        let screen = room_screen(&mut ui, 76, 30);
        // Only inspect rows in the recipient bar, not the sidebar.
        let cells: Vec<_> = screen.chars().collect();
        seen.extend(
            cells
                .chunks(76)
                .skip(ui.view.recipient_bar.y.into())
                .take(ui.view.recipient_bar.height.into())
                .flat_map(|row| row.iter().skip(25)),
        );
        let pointer_y = ui.view.recipient_bar.y + 1;
        mouse(&mut ui, ScrollDown, 40, pointer_y);
        ui.compute_view(76, 30);
        assert_eq!(ui.view.history, history);
        assert_eq!(ui.view.composer, composer);
    }
    for i in 0..24 {
        assert!(seen.contains(&format!("reviewer-{i:02}")));
    }
}

#[test]
fn agent_identity_color_matches_sidebar_chip_and_reply_header() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 1);
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.compute_view(100, 40);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    let [r, g, b] = ui.snapshot.state.agent(agent).unwrap().color;
    let color = ratatui::style::Color::Rgb(r, g, b);
    let mut labels = 0;
    for y in 0..40 {
        for x in 0..94 {
            let text: String = (x..x + 6).map(|x| buffer[(x, y)].symbol()).collect();
            if text == "author" && buffer[(x, y)].fg == color {
                labels += 1;
            }
        }
    }
    assert_eq!(
        labels, 4,
        "one consistent agent identity in the sidebar, chip, recipient and reply headers"
    );
}

#[test]
fn outgoing_header_is_green_and_recent_time_is_local_24h() {
    let (mut ui, room, agent) = fixture();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, "hello").unwrap();
    snapshot.state.submit_draft(room, now).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 40);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    let mut found = false;
    for y in ui.view.history.y..ui.view.history.bottom() {
        let line: String = (30..100).map(|x| buffer[(x, y)].symbol()).collect();
        if line.contains("You → author") {
            found = true;
            assert_eq!(
                buffer[(30, y)].fg,
                ratatui::style::Color::Rgb(102, 255, 102)
            );
            let local = crate::platform::local_datetime().unwrap();
            assert!(line.contains(&format!("{:02}:{:02}", local.hour(), local.minute())));
            assert!(!line.contains("UTC"));
        }
    }
    assert!(found);
}

#[test]
fn old_prompts_and_replies_show_relative_day_label() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 1);
    let screen = room_screen(&mut ui, 100, 40);
    assert_eq!(screen.matches("> 1 day").count(), 2);
}

#[test]
fn history_recipient_colors_survive_wrapping_without_coloring_message_text() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};

    let (mut ui, room, author) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let reviewer = snapshot
        .state
        .create_agent(room, "审核Beta", Provider::Cursor, "/project".into(), None)
        .unwrap();
    snapshot
        .state
        .set_draft_recipients(room, [author, reviewer])
        .unwrap();
    snapshot
        .state
        .set_draft_text(room, "You author 审核Beta")
        .unwrap();
    snapshot.state.submit_draft(room, 1).unwrap();
    let colors: Vec<_> = [author, reviewer]
        .iter()
        .map(|id| {
            let [r, g, b] = snapshot.state.agent(*id).unwrap().color;
            Color::Rgb(r, g, b)
        })
        .collect();
    ui.receive_snapshot(Arc::new(snapshot));
    // At 29 columns the history has 16 cells: 审 ends one header line and
    // 核Beta starts the next, so the split crosses a UTF-8 name span.
    for (cols, reviewer_rows) in [(100, 1), (44, 1), (29, 2)] {
        ui.compute_view(cols, 40);
        let mut buffer = Buffer::empty(Rect::new(0, 0, cols, 40));
        ui.render(&mut buffer);
        let mut cells = Vec::new();
        let mut colored_rows = 0;
        for y in ui.view.history.y..ui.view.history.bottom() {
            let mut row = Vec::new();
            for x in ui.view.history.x + 2..ui.view.history.right() - 2 {
                let cell = &buffer[(x, y)];
                for ch in cell.symbol().chars().filter(|ch| !ch.is_whitespace()) {
                    row.push((ch, cell.fg));
                }
            }
            colored_rows += usize::from(row.iter().any(|(_, color)| *color == colors[1]));
            cells.extend(row);
        }
        assert_eq!(
            colored_rows, reviewer_rows,
            "name wrapping at {cols} columns"
        );
        let header = "You→author,审核Beta>1day";
        assert_eq!(
            cells
                .iter()
                .take(header.chars().count())
                .map(|c| c.0)
                .collect::<String>(),
            header
        );
        let expected_colors = [
            ("You", Color::Rgb(102, 255, 102)),
            ("→", Color::Rgb(145, 148, 159)),
            ("author", colors[0]),
            (",", Color::Rgb(145, 148, 159)),
            ("审核Beta", colors[1]),
            (">1day", Color::Rgb(145, 148, 159)),
            ("Youauthor审核Beta", Color::Rgb(222, 222, 226)),
        ];
        let expected: Vec<_> = expected_colors
            .into_iter()
            .flat_map(|(text, color)| text.chars().map(move |ch| (ch, color)))
            .collect();
        assert_eq!(cells, expected, "rendered history colors at {cols} columns");
    }
}
