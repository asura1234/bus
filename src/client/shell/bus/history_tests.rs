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
    for _ in 0..70 {
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
    for _ in 0..100 {
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
        labels, 3,
        "one consistent agent identity across the three surfaces"
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
