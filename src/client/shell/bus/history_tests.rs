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

fn is_reply(line: &history::Line) -> bool {
    matches!(
        line.raw_markdown,
        Some((history::MarkdownSource::Reply(_), _))
    )
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
fn prompt_and_agent_reply_markdown_are_rendered() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};

    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "**rendered prompt**\nsecond `line`",
        "# Heading\n\n- **bold** and `code`",
    );
    ui.compute_view(100, 40);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    let screen: Vec<String> = (0..40)
        .map(|y| (0..100).map(|x| buffer[(x, y)].symbol()).collect())
        .collect();

    assert!(!screen
        .iter()
        .any(|line| line.contains("**rendered prompt**")));
    let prompt_row = screen
        .iter()
        .position(|line| line.contains("rendered prompt"))
        .expect("rendered prompt");
    assert!(
        screen[prompt_row + 1].contains("second line"),
        "a typed newline stays a line break: {:?}",
        &screen[prompt_row..prompt_row + 2]
    );
    let prompt_x = unicode_width::UnicodeWidthStr::width(
        &screen[prompt_row][..screen[prompt_row].find("rendered prompt").unwrap()],
    ) as u16;
    assert!(buffer[(prompt_x, prompt_row as u16)]
        .modifier
        .contains(Modifier::BOLD));
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
        .position(is_reply)
        .expect("first Markdown reply row");
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

    ui.history.lines(
        &snapshot.state,
        room,
        80,
        snapshot.revision,
        1_000,
        &mut Default::default(),
    );
    let first = ui
        .history
        .cached()
        .iter()
        .filter(|line| is_reply(line))
        .find_map(|line| line.raw_markdown.as_ref().map(|(_, raw)| Arc::clone(raw)))
        .expect("rendered reply source");
    ui.history.lines(
        &snapshot.state,
        room,
        80,
        snapshot.revision,
        61_000,
        &mut Default::default(),
    );
    let second = ui
        .history
        .cached()
        .iter()
        .filter(|line| is_reply(line))
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
        .lines(
            &snapshot.state,
            room,
            12,
            snapshot.revision,
            1_000,
            &mut Default::default(),
        )
        .iter()
        .filter(|line| is_reply(line))
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
        &mut Default::default(),
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
            &mut Default::default(),
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
fn old_prompts_show_relative_day_label_but_replies_do_not() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 1);
    let screen = room_screen(&mut ui, 100, 40);
    assert_eq!(screen.matches("> 1 day").count(), 1);
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

fn exchange_with_files(ui: &mut BusUi, room: RoomId, agent: AgentId, files: &[std::path::PathBuf]) {
    saved_exchange(ui, room, agent, "see attached", "ok");
    let mut snapshot = (*ui.snapshot).clone();
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for (_, record) in json["requests"].as_object_mut().unwrap() {
        record["prompt"]["files"] = serde_json::json!(files);
    }
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

fn thumbnail_dir(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "bus-history-thumbnails-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn png(dir: &std::path::Path, name: &str, size: (u32, u32)) -> std::path::PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    image::RgbaImage::new(size.0, size.1).save(&path).unwrap();
    path
}

const CELL: crate::kitty_graphics::HostCellSize = crate::kitty_graphics::HostCellSize {
    width_px: 10,
    height_px: 20,
};

#[test]
fn image_attachments_draw_as_thumbnails_instead_of_their_name() {
    let dir = thumbnail_dir("rows");
    let image = png(&dir, "shot.png", (200, 80));
    let notes = dir.join("notes.md");
    std::fs::write(&notes, "# notes").unwrap();
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, &[image.clone(), notes]);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);

    let lines = ui.history.cached();
    assert!(
        !lines.iter().any(|line| line.text == "[shot.png]"),
        "the picture replaces the name placeholder"
    );
    let first = lines
        .iter()
        .position(|line| line.thumbnail.is_some())
        .expect("thumbnail rows");
    let thumbnail: Vec<_> = lines[first..first + 4]
        .iter()
        .filter_map(|line| line.thumbnail.as_ref())
        .collect();
    assert_eq!(thumbnail.len(), 4, "80 px tall in 20 px cells");
    assert!(thumbnail
        .iter()
        .enumerate()
        .all(|(row, slot)| slot.row == row as u16 && (slot.cols, slot.rows) == (20, 4)));
    assert!(lines[first..first + 4]
        .iter()
        .all(|line| line.text.is_empty()
            && line.action == Some(render::Action::FileDetail(image.clone()))));
    assert!(lines.iter().any(|line| line.text == "[notes.md]"));
    assert_eq!(
        lines.iter().filter(|line| line.thumbnail.is_some()).count(),
        4,
        "non-image files get no thumbnail"
    );

    let text = ui.view.history_text;
    assert_eq!(ui.view.thumbnails.len(), 1);
    let placement = &ui.view.thumbnails[0];
    assert_eq!(
        (placement.x, placement.cols, placement.rows),
        (text.x, 20, 4)
    );
    assert_eq!(placement.y, text.y + (first - ui.main_scroll) as u16);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn image_attachments_fall_back_to_their_name_without_kitty_or_the_file() {
    let dir = thumbnail_dir("fallback");
    let image = png(&dir, "shot.png", (200, 80));
    let missing = dir.join("gone.jpg");
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, &[image, missing]);

    ui.compute_view(100, 40);
    assert!(ui
        .history
        .cached()
        .iter()
        .all(|line| line.thumbnail.is_none()));
    assert!(ui.view.thumbnails.is_empty());
    assert!(ui
        .history
        .cached()
        .iter()
        .any(|line| line.text == "[shot.png]"));

    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);
    let lines = ui.history.cached();
    assert!(lines.iter().any(|line| line.text == "[gone.jpg]"));
    assert_eq!(
        lines.iter().filter(|line| line.thumbnail.is_some()).count(),
        4,
        "only the readable image gets rows"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn thumbnails_show_only_when_wholly_visible_and_uncovered() {
    let dir = thumbnail_dir("visible");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, &[image]);
    saved_history(&mut ui, room, agent, 30);
    ui.thumbnails.set_cell(Some(CELL));
    ui.history_follow_tail = false;
    ui.compute_view(100, 40);
    assert_eq!(ui.view.thumbnails.len(), 1);

    let first = ui
        .history
        .cached()
        .iter()
        .position(|line| line.thumbnail.is_some())
        .unwrap();
    // Scroll so the thumbnail's top row is just above the viewport.
    ui.main_scroll = first + 1;
    ui.compute_view(100, 40);
    assert_eq!(ui.main_scroll, first + 1);
    assert!(
        ui.view.thumbnails.is_empty(),
        "a partly scrolled thumbnail is not drawn"
    );

    ui.main_scroll = 0;
    ui.form = Some(forms::Form::Help { scroll: 0 });
    ui.compute_view(100, 40);
    assert!(ui.view.thumbnails.is_empty(), "forms cover the history");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recipient_picker_hides_history_thumbnails_that_cover_its_rows() {
    let dir = thumbnail_dir("recipient-picker");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    for index in 0..10 {
        snapshot
            .state
            .create_agent(
                room,
                &format!("reviewer-{index}"),
                Provider::Codex,
                "/project".into(),
                None,
            )
            .unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    exchange_with_files(&mut ui, room, agent, &[image]);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 30);
    assert_eq!(ui.view.thumbnails.len(), 1);
    let original = ui.view.thumbnails[0].clone();
    assert!(!ui.thumbnail_graphics().is_empty());

    ui.action(render::Action::Recipients);
    ui.compute_view(100, 30);
    let original_rect =
        ratatui::layout::Rect::new(original.x, original.y, original.cols, original.rows);
    assert!(
        ui.view
            .hits
            .iter()
            .any(|hit| matches!(hit.action, render::Action::Recipient(_))
                && hit.rect.intersects(original_rect)),
        "the recipient picker must cover the prior thumbnail in this fixture"
    );
    let retained = ui.view.thumbnails.iter().any(|thumbnail| {
        let rect =
            ratatui::layout::Rect::new(thumbnail.x, thumbnail.y, thumbnail.cols, thumbnail.rows);
        ui.view.hits.iter().any(|hit| {
            matches!(hit.action, render::Action::Recipient(_)) && hit.rect.intersects(rect)
        })
    });
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        !retained,
        "Kitty images draw over text and must not obscure recipient choices"
    );
}

#[test]
fn file_detail_popup_hides_history_thumbnails_it_covers() {
    let dir = thumbnail_dir("detail-popup");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, &[image]);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 30);
    assert_eq!(ui.view.thumbnails.len(), 1);

    // A path long enough to wrap over every history row above the composer.
    ui.detail_path = Some(format!("/{}", "deep/".repeat(200)));
    ui.compute_view(100, 30);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        ui.view.thumbnails.is_empty(),
        "the file detail popup covers the thumbnail rows"
    );
}

#[test]
fn a_steered_group_shows_its_messages_stacked_with_one_reply() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let mut ids = Vec::new();
    for (text, at) in [
        ("write the parser", 1_000),
        ("use streaming instead", 2_000),
    ] {
        snapshot.state.set_draft_recipients(room, [agent]).unwrap();
        snapshot.state.set_draft_text(room, text).unwrap();
        ids.push(snapshot.state.submit_draft(room, at).unwrap()[0]);
    }
    let mut json = serde_json::to_value(&snapshot.state).unwrap();
    for id in &ids {
        let record = &mut json["requests"][id.0.to_string()];
        record["phase"] = "completed".into();
        record["completed_at_ms"] = 3_000.into();
        record["pending_final"] = serde_json::json!({
            "callback_id": "final", "text": "streaming parser written",
            "received_at_ms": 3_000, "provider_session_id": "s", "provider_turn_id": "t"
        });
    }
    json["requests"][ids[1].0.to_string()]["group"] = ids[0].0.into();
    json["requests"][ids[1].0.to_string()]["steered"] = true.into();
    snapshot.state = serde_json::from_value(json).unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));

    let snapshot = ui.snapshot.clone();
    let lines = ui.history.lines(
        &snapshot.state,
        snapshot.state.room(room).unwrap(),
        100,
        snapshot.revision,
        4_000,
        &mut Default::default(),
    );
    let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    let position = |needle: &str| texts.iter().position(|text| text.contains(needle));
    assert_eq!(
        texts
            .iter()
            .filter(|text| text.contains("streaming parser written"))
            .count(),
        1
    );
    assert!(position("write the parser") < position("use streaming instead"));
    assert!(position("use streaming instead") < position("streaming parser written"));
}

#[test]
fn option_enter_sends_the_draft_to_wait_for_each_agents_own_turn() {
    let (mut ui, room, agent) = fixture();
    ui.room = Some(room);
    ui.receive_snapshot(ui.snapshot.clone());
    let local = ui.locals.get_mut(&room).unwrap();
    local.recipients.insert(agent);
    local.text.insert("after you finish");
    key(&mut ui, KeyCode::Enter, KeyModifiers::ALT);
    ui.settle();
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::SubmitQueued(id) if id == room)));
    assert!(!ui.send_queued);

    let local = ui.locals.get_mut(&room).unwrap();
    local.text.insert("now");
    ui.pending.clear();
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    ui.settle();
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::Submit(id) if id == room)));
}

#[test]
fn iterm2_draws_the_picture_at_its_reserved_rows() {
    let dir = thumbnail_dir("iterm2");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&image));
    ui.graphics = Some(super::thumbnails::Protocol::Iterm2);
    ui.thumbnails
        .set_protocol(super::thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);

    // The picture's rows open the file; no name placeholder is drawn.
    let lines = ui.history.cached();
    assert!(!lines.iter().any(|line| line.text == "[shot.png]"));
    let rows: Vec<_> = lines
        .iter()
        .filter(|line| line.thumbnail.is_some())
        .collect();
    assert_eq!(rows.len(), 4);
    assert!(rows
        .iter()
        .all(|line| line.action == Some(render::Action::FileDetail(image.clone()))));

    let placement = ui.view.thumbnails[0].clone();
    ui.full_repaint = false;
    let graphics = String::from_utf8(ui.thumbnail_graphics()).unwrap();
    assert!(
        graphics.contains(&format!(
            "\x1b[{};{}H\x1b]1337;File=inline=1;",
            placement.y + 1,
            placement.x + 1
        )),
        "{graphics:?}"
    );
    assert!(graphics.contains(";width=20;height=4;"));
    assert!(!ui.full_repaint, "first draw needs no repaint");
    ui.graphics_presented(false);
    assert!(ui.thumbnail_graphics().is_empty(), "unchanged frame");

    // Switching screens repaints every cell, so the image is drawn again.
    ui.full_repaint = true;
    assert!(!ui.thumbnail_graphics().is_empty());
    ui.graphics_presented(false);

    // A cover hides it: the client repaints to erase the image cells.
    ui.full_repaint = false;
    ui.view.thumbnails.clear();
    assert!(ui.thumbnail_graphics().is_empty());
    assert!(ui.full_repaint);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn without_an_image_protocol_the_name_row_is_still_clickable() {
    // Terminal.app: no protocol, so no cell size and no reserved rows.
    let dir = thumbnail_dir("terminal-app");
    let image = png(&dir, "shot.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&image));
    ui.compute_view(100, 40);
    let lines = ui.history.cached();
    let caption = lines
        .iter()
        .find(|line| line.text == "[shot.png]")
        .expect("image name row");
    assert_eq!(caption.action, Some(render::Action::FileDetail(image)));
    assert!(lines.iter().all(|line| line.thumbnail.is_none()));
    assert!(ui.thumbnail_graphics().is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn images_saved_before_a_restart_draw_after_dropped_or_cleared_frames() {
    // A fresh client (as after `resume`) opens history that already holds an
    // image; its first frames can be dropped while presentation is frozen.
    let dir = thumbnail_dir("resume");
    let image = png(&dir, "earlier.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&image));
    ui.graphics = Some(super::thumbnails::Protocol::Kitty);
    ui.thumbnails
        .set_protocol(super::thumbnails::Protocol::Kitty);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);
    let draws = |graphics: &[u8]| {
        let text = String::from_utf8_lossy(graphics);
        (text.contains("a=t,"), text.contains("a=p,"))
    };

    // The first frame is composed but never written.
    assert_eq!(draws(&ui.thumbnail_graphics()), (true, true));
    ui.compute_view(100, 40);
    assert_eq!(
        draws(&ui.thumbnail_graphics()),
        (true, true),
        "a dropped frame is sent again: upload and placement"
    );

    // Once the client confirms the write, nothing is resent.
    ui.graphics_presented(false);
    ui.compute_view(100, 40);
    assert!(ui.thumbnail_graphics().is_empty());
    ui.graphics_presented(false);

    // A screen clear (first frame, resize) wipes placements: draw again.
    ui.graphics_presented(true);
    assert!(ui.thumbnails.stale(), "the next tick recomposes");
    ui.compute_view(100, 40);
    assert_eq!(draws(&ui.thumbnail_graphics()), (true, true));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_notice_on_the_status_line_does_not_hide_history_images() {
    // Resuming against a running server shows a dev-log notice on the status
    // line until it is cleared; images above it must still draw.
    let dir = thumbnail_dir("notice");
    let image = png(&dir, "earlier.png", (200, 80));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, std::slice::from_ref(&image));
    ui.graphics = Some(super::thumbnails::Protocol::Kitty);
    ui.thumbnails
        .set_protocol(super::thumbnails::Protocol::Kitty);
    ui.thumbnails.set_cell(Some(CELL));
    ui.error = Some(
        "Dev logs enabled for this client. An existing server keeps its original log level; \
         restart it when safe for full server logs. Agents were not restarted."
            .into(),
    );
    ui.compute_view(100, 40);

    assert_eq!(ui.view.thumbnails.len(), 1, "the image stays placed");
    let placement = ui.view.thumbnails[0].clone();
    assert!(
        placement.y + placement.rows <= ui.view.history.y + ui.view.history.height,
        "the image sits inside the history, clear of the status line"
    );
    let graphics = String::from_utf8(ui.thumbnail_graphics()).unwrap();
    assert!(
        graphics.contains("a=t,") && graphics.contains("a=p,"),
        "{graphics}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dialog_notices_to_an_agent_never_show_in_room_history() {
    let (mut ui, room, agent) = fixture();
    ui.open_room(room);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .submit_message_from(
            room,
            Draft {
                text: "Codex wants to run: rm -rf build".into(),
                files: Vec::new(),
                recipient_ids: [agent].into(),
            },
            Author::Bus,
            1000,
        )
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let screen = room_screen(&mut ui, 100, 30);
    assert!(!screen.contains("wants to run"), "{screen}");
    assert!(!screen.contains("Bus"), "{screen}");
}
