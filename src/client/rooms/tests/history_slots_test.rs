use super::*;
use crossterm::event::MouseEventKind::ScrollDown;

#[test]
fn compaction_notice_history_is_one_plain_line_without_a_reply_slot() {
    let (mut ui, room, author) = fixture();
    let mut state = ui.snapshot.state.clone();
    let text = "author -> orchestrator: reached 5 compactions; get a handover note and replace it.";
    state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [author].into(),
            },
            Author::Agent(author),
            1_000,
        )
        .unwrap();
    // A saved notice keeps its presentation even when loaded into a new UI.
    let mut saved = serde_json::to_value(&state).unwrap();
    for request in saved["requests"].as_object_mut().unwrap().values_mut() {
        request["prompt"]["compaction_limit_notice"] = true.into();
    }
    let state = serde_json::from_value::<BusState>(saved).unwrap();
    let lines = ui.history.lines(
        &state,
        state.room(room).unwrap(),
        120,
        1,
        2_000,
        &mut Default::default(),
    );
    let text_lines: Vec<_> = lines
        .iter()
        .filter(|line| !line.text.is_empty())
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(text_lines, [text]);
    assert!(lines
        .iter()
        .all(|line| line.styles.is_empty() && line.raw_markdown.is_none()));
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
        !lines.iter().any(|line| line.text == "shot.png"),
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
            && line.action == Some(render::Action::OpenFile(image.clone()))));
    assert!(lines.iter().any(|line| line.text == "notes.md"));
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
        .any(|line| line.text == "shot.png"));

    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);
    let lines = ui.history.cached();
    assert!(lines.iter().any(|line| line.text == "gone.jpg"));
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
    ui.graphics = Some(thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_protocol(thumbnails::Protocol::Iterm2);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);

    // The picture's rows open the file; no name placeholder is drawn.
    let lines = ui.history.cached();
    assert!(!lines.iter().any(|line| line.text == "shot.png"));
    let rows: Vec<_> = lines
        .iter()
        .filter(|line| line.thumbnail.is_some())
        .collect();
    assert_eq!(rows.len(), 4);
    assert!(rows
        .iter()
        .all(|line| line.action == Some(render::Action::OpenFile(image.clone()))));

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
        .find(|line| line.text == "shot.png")
        .expect("image name row");
    assert_eq!(caption.action, Some(render::Action::OpenFile(image)));
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
    ui.graphics = Some(thumbnails::Protocol::Kitty);
    ui.thumbnails.set_protocol(thumbnails::Protocol::Kitty);
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
    ui.graphics = Some(thumbnails::Protocol::Kitty);
    ui.thumbnails.set_protocol(thumbnails::Protocol::Kitty);
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

#[test]
fn two_images_in_one_message_render_in_order_with_distinct_placements() {
    let dir = thumbnail_dir("two");
    let first = png(&dir, "first.png", (200, 80));
    let second = png(&dir, "second.png", (100, 60));
    let (mut ui, room, agent) = fixture();
    exchange_with_files(&mut ui, room, agent, &[first.clone(), second.clone()]);
    ui.thumbnails.set_cell(Some(CELL));
    ui.compute_view(100, 40);

    // Both images get their own rows, in message order, before the reply.
    let lines = ui.history.cached();
    let owner = |line: &history::Line| line.thumbnail.as_ref().map(|t| t.path.to_path_buf());
    let order: Vec<_> = lines.iter().filter_map(owner).collect();
    assert_eq!(
        order,
        [vec![first.clone(); 4], vec![second.clone(); 3]].concat()
    );
    let reply = lines
        .iter()
        .position(|line| line.text.starts_with("    author"))
        .unwrap();
    assert!(
        lines
            .iter()
            .rposition(|line| line.thumbnail.is_some())
            .unwrap()
            < reply
    );

    // Each one is placed, with a placement id no other thumbnail shares.
    let placed: Vec<_> = ui
        .view
        .thumbnails
        .iter()
        .map(|p| p.path.to_path_buf())
        .collect();
    assert_eq!(placed, [first, second]);
    let graphics = String::from_utf8(ui.thumbnail_graphics()).unwrap();
    let ids: Vec<&str> = graphics
        .split("\x1b_G")
        .filter(|command| command.starts_with("a=p,"))
        .map(|command| {
            command
                .split(',')
                .find(|key| key.starts_with("p="))
                .unwrap()
        })
        .collect();
    assert_eq!(ids.len(), 2, "{graphics:?}");
    assert_ne!(
        ids[0], ids[1],
        "a shared placement id lets one image replace the other"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
