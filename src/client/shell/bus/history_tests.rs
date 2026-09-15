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

fn saved_exchange(
    ui: &mut BusUi,
    room: RoomId,
    agent: AgentId,
    prompt: &str,
    reply: &str,
) -> RequestId {
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, prompt).unwrap();
    let request = snapshot.state.submit_draft(room, 1000).unwrap()[0];
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    let record = &mut json["requests"][request.0.to_string()];
    record["phase"] = "completed".into();
    record["completed_at_ms"] = 1500.into();
    record["pending_final"] = serde_json::json!({
        "callback_id": "final", "text": reply, "received_at_ms": 1500,
        "provider_session_id": "session", "provider_turn_id": "turn"
    });
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    request
}

#[test]
fn agent_reply_markdown_is_rendered_while_prompt_stays_literal() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};

    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "**literal prompt**",
        "# Heading\n\n- **bold** and `code`",
    );
    ui.compute_view(100, 40);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    let screen: Vec<String> = (0..40)
        .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect())
        .collect();

    assert!(
        screen
            .iter()
            .any(|line| line.contains("**literal prompt**")),
        "user-authored prompts stay literal"
    );
    assert!(!screen.iter().any(|line| line.contains("# Heading")));
    assert!(!screen.iter().any(|line| line.contains("**bold**")));
    assert!(!screen.iter().any(|line| line.contains("`code`")));

    let (heading_y, heading_x) = screen
        .iter()
        .enumerate()
        .find_map(|(y, line)| {
            line.find("Heading").map(|x| {
                (
                    y as u16,
                    unicode_width::UnicodeWidthStr::width(&line[..x]) as u16,
                )
            })
        })
        .expect("rendered heading");
    assert!(
        buffer[(heading_x, heading_y)]
            .modifier
            .contains(Modifier::BOLD),
        "headings are visually distinct"
    );
    let (bold_y, bold_x) = screen
        .iter()
        .enumerate()
        .find_map(|(y, line)| {
            line.find("bold").map(|x| {
                (
                    y as u16,
                    unicode_width::UnicodeWidthStr::width(&line[..x]) as u16,
                )
            })
        })
        .expect("rendered strong text");
    assert!(
        buffer[(bold_x, bold_y)].modifier.contains(Modifier::BOLD),
        "strong text keeps its emphasis"
    );
    assert_eq!(
        buffer[(bold_x, bold_y)].fg,
        ratatui::style::Color::Rgb(222, 222, 226),
        "Markdown styles preserve the room's ordinary foreground"
    );
    assert_eq!(
        buffer[(bold_x, bold_y)].bg,
        ratatui::style::Color::Rgb(24, 24, 28),
        "Markdown styles preserve the room background"
    );
}

#[test]
fn selecting_a_rendered_agent_reply_copies_its_raw_markdown() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};

    let (mut ui, room, agent) = fixture();
    let markdown = "**bold** and [docs](https://example.com) and `code`";
    saved_exchange(&mut ui, room, agent, "literal prompt", markdown);
    ui.compute_view(100, 40);
    let text = ui.view.history_text;
    let (index, rendered) = ui
        .history
        .cached()
        .iter()
        .enumerate()
        .find(|(_, line)| line.text.contains("bold and docs"))
        .map(|(index, line)| (index, line.text.clone()))
        .expect("rendered agent reply");
    let row = text.y + (index - ui.main_scroll) as u16;

    assert_eq!(
        drag_copy(&mut ui, (text.x, row), (text.x + 1, row)).as_deref(),
        Some(markdown)
    );

    ui.compute_view(100, 40);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    for x in text.x..text.x + rendered.len() as u16 {
        assert_eq!(
            buffer[(x, row)].bg,
            Color::Rgb(44, 88, 56),
            "the visible selection covers the whole raw Markdown reply"
        );
    }
}

#[test]
fn selection_stopping_before_markdown_keeps_the_selected_line_end_in_both_directions() {
    let (mut ui, room, agent) = fixture();
    saved_exchange(&mut ui, room, agent, "literal prompt", "**raw reply**");
    ui.compute_view(100, 40);
    let prompt = ui
        .history
        .cached()
        .iter()
        .position(|line| line.text == "literal prompt")
        .expect("prompt row");
    let reply = ui
        .history
        .cached()
        .iter()
        .position(|line| line.raw_markdown.is_some())
        .expect("first Markdown row");
    let prompt_start = selection::Point {
        line: prompt,
        offset: 0,
    };
    let reply_start = selection::Point {
        line: reply,
        offset: 0,
    };
    let before_reply_end = selection::Point {
        line: reply - 1,
        offset: ui.history.cached()[reply - 1].text.len(),
    };
    ui.history_selection = Some((prompt_start, before_reply_end));
    let expected = format!("{}\n", ui.selected_text().expect("text before reply"));

    for selection in [(prompt_start, reply_start), (reply_start, prompt_start)] {
        ui.history_selection = Some(selection);
        assert_eq!(ui.selected_text().as_deref(), Some(expected.as_str()));
        assert!(!expected.contains("raw reply"));
    }
}

#[test]
fn quote_keeps_the_agent_reply_as_raw_markdown() {
    let (mut ui, room, agent) = fixture();
    let markdown = "**bold** and [docs](https://example.com) and `code`";
    let request = saved_exchange(&mut ui, room, agent, "literal prompt", markdown);
    ui.compute_view(100, 40);
    let action = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Quote(request))
        .expect("quote action for reply")
        .action
        .clone();

    ui.action(action);

    assert_eq!(
        ui.locals[&room].text.text,
        "author: \"**bold** and [docs](https://example.com) and `code`\"\n"
    );
}

#[test]
fn markdown_reply_rendering_is_reused_when_only_timestamp_age_changes() {
    let (mut ui, room, agent) = fixture();
    saved_exchange(&mut ui, room, agent, "literal prompt", "**cached reply**");
    let snapshot = Arc::clone(&ui.snapshot);
    let room = snapshot.state.room(room).expect("room");

    ui.history
        .lines(&snapshot.state, room, 80, snapshot.revision, 1_000);
    let first = ui
        .history
        .cached()
        .iter()
        .find_map(|line| line.raw_markdown.as_ref().map(|(_, raw)| Arc::clone(raw)))
        .expect("rendered reply source");
    ui.history
        .lines(&snapshot.state, room, 80, snapshot.revision, 61_000);
    let second = ui
        .history
        .cached()
        .iter()
        .find_map(|line| line.raw_markdown.as_ref().map(|(_, raw)| Arc::clone(raw)))
        .expect("rendered reply source after timestamp refresh");

    assert!(
        Arc::ptr_eq(&first, &second),
        "timestamp refreshes reuse the immutable reply rendering"
    );
}

#[test]
fn narrow_markdown_keeps_unicode_styles_and_table_content_within_width() {
    use ratatui::style::Modifier;

    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "literal prompt",
        "## 日本語 👩‍💻\n\n**強調 text**\n\n| A | B |\n| - | - |\n| 甲 | 🙂 |",
    );
    let snapshot = Arc::clone(&ui.snapshot);
    let room = snapshot.state.room(room).expect("room");
    let rendered: Vec<_> = ui
        .history
        .lines(&snapshot.state, room, 12, snapshot.revision, 1_000)
        .iter()
        .filter(|line| line.raw_markdown.is_some())
        .collect();
    let text = rendered
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered
        .iter()
        .all(|line| { unicode_width::UnicodeWidthStr::width(line.text.as_str()) <= 12 }));
    for expected in ["日本語", "👩‍💻", "強調", "甲", "🙂"] {
        assert!(
            text.contains(expected),
            "missing {expected:?} from {text:?}"
        );
    }
    assert!(rendered.iter().any(|line| {
        line.styles
            .iter()
            .any(|(run, style)| run.contains("強調") && style.add_modifier.contains(Modifier::BOLD))
    }));
}

fn complete_requests(
    ui: &mut BusUi,
    completions: impl IntoIterator<Item = (RequestId, &'static str, u64)>,
) {
    let mut snapshot = (*ui.snapshot).clone();
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (id, text, received_at_ms) in completions {
        let record = &mut json["requests"][id.0.to_string()];
        record["phase"] = "completed".into();
        record["completed_at_ms"] = received_at_ms.into();
        record["pending_final"] = serde_json::json!({
            "callback_id": format!("final-{}", id.0), "text": text,
            "received_at_ms": received_at_ms,
            "provider_session_id": "session", "provider_turn_id": format!("turn-{}", id.0)
        });
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

#[test]
fn pending_replies_reserve_indented_slots_in_recipient_order() {
    let (mut ui, room, author) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let claude = snapshot
        .state
        .create_agent(
            room,
            "claude1",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    let cursor = snapshot
        .state
        .create_agent(room, "cursor1", Provider::Cursor, "/project".into(), None)
        .unwrap();
    snapshot
        .state
        .set_draft_recipients(room, [cursor, author, claude])
        .unwrap();
    snapshot.state.set_draft_text(room, "review this").unwrap();
    snapshot.state.submit_draft(room, 1_000).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));

    let snapshot = ui.snapshot.clone();
    let lines = ui.history.lines(
        &snapshot.state,
        snapshot.state.room(room).unwrap(),
        100,
        snapshot.revision,
        2_000,
    );
    assert!(lines[0].text.starts_with("You → cursor1, author, claude1"));
    let slot_headers = lines
        .iter()
        .filter_map(|line| {
            let text = line.text.strip_prefix("    ")?;
            ["cursor1", "author", "claude1"]
                .into_iter()
                .find(|name| text.starts_with(name))
        })
        .collect::<Vec<_>>();
    assert_eq!(slot_headers, ["cursor1", "author", "claude1"]);
    assert_eq!(lines.iter().filter(|line| line.text == "    …").count(), 3);
}

#[test]
fn all_recipient_toggle_clears_a_full_selection_in_any_order() {
    let (mut ui, room, author) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let claude = snapshot
        .state
        .create_agent(
            room,
            "claude1",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    let cursor = snapshot
        .state
        .create_agent(room, "cursor1", Provider::Cursor, "/project".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));

    for agent in [cursor, author, claude] {
        ui.action(render::Action::Recipient(Some(agent)));
    }
    assert_eq!(
        ui.locals[&room]
            .recipients
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [cursor, author, claude]
    );

    ui.action(render::Action::Recipient(None));

    assert!(ui.locals[&room].recipients.is_empty());
}

#[test]
fn late_replies_stay_with_their_prompt_and_never_reorder_recipient_slots() {
    let (mut ui, room, author) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let claude = snapshot
        .state
        .create_agent(
            room,
            "claude1",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    let cursor = snapshot
        .state
        .create_agent(room, "cursor1", Provider::Cursor, "/project".into(), None)
        .unwrap();
    snapshot
        .state
        .set_draft_recipients(room, [cursor, author, claude])
        .unwrap();
    snapshot.state.set_draft_text(room, "first prompt").unwrap();
    let first = snapshot.state.submit_draft(room, 1_000).unwrap();
    snapshot.state.set_draft_recipients(room, [author]).unwrap();
    snapshot
        .state
        .set_draft_text(room, "second prompt")
        .unwrap();
    let second = snapshot.state.submit_draft(room, 2_000).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    complete_requests(
        &mut ui,
        [
            (second[0], "second answer", 2_500),
            (first[2], "claude answer", 3_000),
            (first[1], "author answer", 4_000),
            (first[0], "cursor answer", 5_000),
        ],
    );

    let snapshot = ui.snapshot.clone();
    let text = ui
        .history
        .lines(
            &snapshot.state,
            snapshot.state.room(room).unwrap(),
            100,
            snapshot.revision,
            6_000,
        )
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let positions = [
        "first prompt",
        "cursor answer",
        "author answer",
        "claude answer",
        "second prompt",
        "second answer",
    ]
    .map(|needle| {
        text.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}:\n{text}"))
    });
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
}

#[test]
fn late_reply_expansion_above_viewport_preserves_the_visible_anchor() {
    let (mut ui, room, author) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut requests = Vec::new();
    for index in 0..8 {
        snapshot.state.set_draft_recipients(room, [author]).unwrap();
        snapshot
            .state
            .set_draft_text(room, &format!("anchor-prompt-{index}"))
            .unwrap();
        requests.push(snapshot.state.submit_draft(room, 1_000 + index).unwrap()[0]);
    }
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 24);
    let anchor = "anchor-prompt-5";
    ui.main_scroll = ui
        .history
        .cached()
        .iter()
        .position(|line| line.text == anchor)
        .unwrap();
    ui.history_follow_tail = false;

    complete_requests(
        &mut ui,
        [(requests[0], "one\ntwo\nthree\nfour\nfive\nsix", 9_000)],
    );
    ui.compute_view(100, 24);

    assert_eq!(ui.history.cached()[ui.main_scroll].text, anchor);
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
            let row_text = (ui.view.history.x + 2..ui.view.history.right() - 2)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>();
            if row_text.starts_with("    ") {
                break;
            }
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
