use super::*;

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
fn a_drag_inside_a_rendered_reply_selects_and_copies_only_those_characters() {
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};

    let (mut ui, room, agent) = fixture();
    let markdown = "**bold** and [docs](https://example.com) and `code`";
    saved_exchange(&mut ui, room, agent, "literal prompt", markdown);
    ui.compute_view(100, 40);
    let (start, row) = locate(&ui, "and docs");
    // "and docs" is 8 cells; the release column is inclusive.
    assert_eq!(
        drag_copy(&mut ui, (start, row), (start + 7, row)).as_deref(),
        Some("and docs"),
        "rendered text, not its Markdown source"
    );

    ui.compute_view(100, 40);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 40));
    ui.render(&mut buffer);
    let tint = Color::Rgb(44, 88, 56);
    for x in start..start + 8 {
        assert_eq!(buffer[(x, row)].bg, tint, "selected cell {x}");
    }
    assert_ne!(buffer[(start - 2, row)].bg, tint, "before the press point");
    assert_ne!(buffer[(start + 9, row)].bg, tint, "after the release point");
}

#[test]
fn a_drag_across_two_messages_copies_the_text_between_without_layout_indents() {
    let (mut ui, room, agent) = fixture();
    saved_exchange(&mut ui, room, agent, "literal prompt", "**bold** reply");
    ui.compute_view(100, 40);
    let (from, from_row) = locate(&ui, "prompt");
    let (to, to_row) = locate(&ui, "bold reply");
    assert!(to_row > from_row);
    // From "prompt" in the human's message to "bold" in the agent's reply:
    // the blank separator row and the reply header in between are inside the
    // selection, so they are copied, but the four-cell reply indent is layout
    // and is not.
    assert_eq!(
        drag_copy(&mut ui, (from, from_row), (to + 3, to_row)).as_deref(),
        Some("prompt\n\nauthor  Codex\nbold")
    );
}

#[test]
fn a_drag_across_a_wrapped_reply_line_rejoins_it_and_never_copies_quote() {
    let (mut ui, room, agent) = fixture();
    let words: Vec<String> = (0..30).map(|n| format!("w{n:02}")).collect();
    let reply = format!("**lead** {}", words.join(" "));
    saved_exchange(&mut ui, room, agent, "literal prompt", &reply);
    ui.compute_view(60, 40);
    let rows: Vec<_> = ui
        .history
        .cached()
        .iter()
        .filter(|line| is_reply(line))
        .map(|line| line.text.clone())
        .collect();
    assert!(rows.len() >= 2, "the reply must wrap: {rows:?}");
    let (from, from_row) = locate(&ui, "lead");
    let (to, to_row) = locate(&ui, "w29");
    assert!(to_row > from_row);
    let (_, quote_row) = locate(&ui, "Quote");
    assert!(quote_row > to_row);
    // Wrapped rows rejoin with one space, as the reply was written.
    let expected = format!("lead {}", words.join(" "));
    assert_eq!(
        drag_copy(&mut ui, (from, from_row), (to + 2, to_row)).as_deref(),
        Some(expected.as_str())
    );
    // Dragging on past the Quote button adds only the blank row above it: the
    // button is not message text.
    assert_eq!(
        drag_copy(&mut ui, (from, from_row), (from + 4, quote_row)).as_deref(),
        Some(format!("{expected}\n").as_str())
    );
}

#[test]
fn a_drag_in_a_scrolled_history_selects_the_rows_on_screen() {
    let (mut ui, room, agent) = fixture();
    saved_history(&mut ui, room, agent, 30);
    ui.history_follow_tail = false;
    ui.compute_view(100, 40);
    let target = ui
        .history
        .cached()
        .iter()
        .position(|line| line.text.trim() == "prompt-10")
        .unwrap();
    // Scroll so prompt-10 sits mid-screen, with neither end of the history shown.
    ui.main_scroll = target - 5;
    ui.compute_view(100, 40);
    assert_eq!(ui.main_scroll, target - 5);
    let (column, row) = locate(&ui, "prompt-10");
    assert_eq!(
        drag_copy(&mut ui, (column + 3, row), (column + 8, row)).as_deref(),
        Some("mpt-10")
    );
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

fn copy_markdown_body_at_width(ui: &mut BusUi, room: RoomId, width: u16, prompt: bool) -> String {
    let snapshot = Arc::clone(&ui.snapshot);
    ui.history.lines(
        &snapshot.state,
        snapshot.state.room(room).unwrap(),
        width,
        snapshot.revision,
        2_000,
        &mut Default::default(),
    );
    let body = |line: &history::Line| {
        let matches_source = match line.raw_markdown.as_ref() {
            Some((history::MarkdownSource::Prompt(_), _)) => prompt,
            Some((history::MarkdownSource::Reply(_), _)) => !prompt,
            None => false,
        };
        matches_source && !line.text.trim().is_empty()
    };
    let lines = ui.history.cached();
    let start = lines.iter().position(body).expect("first body row");
    let end = lines.iter().rposition(body).expect("last body row");
    ui.history_selection = Some((
        selection::Point {
            line: start,
            offset: lines[start].copy_from,
        },
        selection::Point {
            line: end,
            offset: lines[end].text.len(),
        },
    ));
    ui.selected_text().expect("selected body text")
}

#[test]
fn a_history_copy_of_a_wrapped_list_item_skips_its_continuation_indent() {
    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "prompt",
        "- alpha beta gamma delta epsilon zeta",
    );

    assert_eq!(
        copy_markdown_body_at_width(&mut ui, room, 20, false),
        "• alpha beta gamma delta epsilon zeta"
    );
}
