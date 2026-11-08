use super::*;

fn is_prompt(line: &history::Line) -> bool {
    matches!(
        line.raw_markdown,
        Some((history::MarkdownSource::Prompt(_), _))
    )
}

#[test]
fn waiting_tags_keep_you_color_when_markdown_table_cells_wrap() {
    use ratatui::style::Color;

    let (mut ui, room, agent) = fixture();
    saved_exchange(
        &mut ui,
        room,
        agent,
        "| Status | Next |\n| --- | --- |\n| [waiting-on-you] | confirm |",
        "hidden reply",
    );
    let snapshot = Arc::clone(&ui.snapshot);
    for width in [12, 24, 32, 80] {
        let lines = ui.history.lines(
            &snapshot.state,
            snapshot.state.room(room).unwrap(),
            width,
            snapshot.revision,
            2_000,
            &mut Default::default(),
        );
        let body: Vec<_> = lines.iter().filter(|line| is_prompt(line)).collect();
        let mut tag_characters = String::new();
        let mut tag_rows = 0;
        for line in body {
            let mut has_tag = false;
            for (text, style) in &line.styles {
                if style.fg == Some(Color::Rgb(102, 255, 102)) {
                    tag_characters.push_str(text);
                    has_tag = true;
                }
            }
            tag_rows += usize::from(has_tag);
        }
        assert_eq!(
            tag_characters, "[waiting-on-you]",
            "every tag fragment must retain the You foreground at width {width}"
        );
        if width == 12 {
            assert!(tag_rows > 1, "the narrow table must actually wrap the tag");
        }
    }
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
        "hidden reply",
    );
    saved_exchange(
        &mut ui,
        room,
        agent,
        "# Heading\n\n- **bold** and `code`",
        "hidden reply",
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
    saved_exchange(&mut ui, room, agent, markdown, "hidden reply");
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
    saved_exchange(&mut ui, room, agent, "literal prompt", "hidden reply");
    saved_exchange(&mut ui, room, agent, "**bold** reply", "hidden reply");
    ui.compute_view(100, 40);
    let (from, from_row) = locate(&ui, "prompt");
    let (to, to_row) = locate(&ui, "bold reply");
    assert!(to_row > from_row);
    // From "prompt" through the next message header to "bold". The blank
    // separator is copied; there is no reply indent and no quote row.
    assert_eq!(
        drag_copy(&mut ui, (from, from_row), (to + 3, to_row)).as_deref(),
        Some("prompt\n\n\nYou → author  > 1 day\nbold")
    );
}

#[test]
fn a_drag_across_a_wrapped_reply_line_rejoins_it_and_never_copies_quote() {
    let (mut ui, room, agent) = fixture();
    let words: Vec<String> = (0..30).map(|n| format!("w{n:02}")).collect();
    let reply = format!("**lead** {}", words.join(" "));
    saved_exchange(&mut ui, room, agent, &reply, "hidden reply");
    ui.compute_view(60, 40);
    let rows: Vec<_> = ui
        .history
        .cached()
        .iter()
        .filter(|line| is_prompt(line))
        .map(|line| line.text.clone())
        .collect();
    assert!(rows.len() >= 2, "the message must wrap: {rows:?}");
    let (from, from_row) = locate(&ui, "lead");
    let (to, to_row) = locate(&ui, "w29");
    assert!(to_row > from_row);
    let gap_row = to_row + 1;
    // Wrapped rows rejoin with one space, as the message was written.
    let expected = format!("lead {}", words.join(" "));
    assert_eq!(
        drag_copy(&mut ui, (from, from_row), (to + 2, to_row)).as_deref(),
        Some(expected.as_str())
    );
    // Dragging onto the blank row under the message adds that row only.
    // There is no Quote button to copy.
    let copied = drag_copy(&mut ui, (from, from_row), (from + 4, gap_row));
    assert_eq!(copied.as_deref(), Some(format!("{expected}\n").as_str()));
    assert!(!copied.unwrap_or_default().contains("Quote"));
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
    saved_exchange(&mut ui, room, agent, "literal prompt", "hidden reply");
    saved_exchange(&mut ui, room, agent, "**raw reply**", "hidden reply");
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
        .position(|line| is_prompt(line) && line.text.contains("raw reply"))
        .expect("markdown prompt row");
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
fn clicking_right_of_quote_does_nothing() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::*};

    let (mut ui, room, agent) = fixture();
    let request = saved_exchange(&mut ui, room, agent, "prompt", "prompt");
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft stays".into());
    ui.compute_view(100, 40);
    assert!(ui
        .view
        .hits
        .iter()
        .all(|hit| hit.action != render::Action::Quote(request)));
    assert!(ui
        .history
        .cached()
        .iter()
        .all(|line| !matches!(line.action, Some(render::Action::Quote(_)))));
    let (x, y) = locate(&ui, "prompt");
    let beside = x + 6;
    assert_eq!(ui.view.hit_at(beside, y), None);
    assert_eq!(pointer(&mut ui, Down(Left), beside, y), None);
    assert_eq!(ui.locals[&room].text.text, "draft stays");
    assert!(
        ui.history_selection.is_some(),
        "cells beside the prompt are an ordinary history row"
    );
    assert!(ui.last_click.is_none(), "no quote action is dispatched");
    assert_eq!(pointer(&mut ui, Up(Left), beside, y), None);
    assert_eq!(ui.locals[&room].text.text, "draft stays");
    assert!(ui.history_selection.is_none());
    assert!(ui.last_click.is_none());
}

#[test]
fn quote_padding_preserves_selection_and_draft() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::*};

    let (mut ui, room, agent) = fixture();
    saved_exchange(&mut ui, room, agent, "prompt", "prompt");
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft stays".into());
    ui.compute_view(100, 40);
    assert!(ui
        .view
        .hits
        .iter()
        .all(|hit| !matches!(hit.action, render::Action::Quote(_))));
    let (x, y) = locate(&ui, "prompt");
    let point = selection::Point { line: 0, offset: 0 };
    let selection = Some((point, point));
    ui.history_selection = selection;
    assert_eq!(pointer(&mut ui, Down(Left), x, y), None);
    assert_eq!(ui.locals[&room].text.text, "draft stays");
    assert_ne!(
        ui.history_selection, selection,
        "the prompt row selects; it is not inert quote padding"
    );
    assert!(ui.history_selection.is_some());
    assert!(ui.last_click.is_none());
}

#[test]
fn quote_hit_tracks_visible_label_cells_after_resize() {
    let (mut ui, room, agent) = fixture();
    let request = saved_exchange(&mut ui, room, agent, "reply", "reply");
    for cols in [36, 38, 80, 100] {
        ui.compute_view(cols, 40);
        assert!(
            ui.view
                .hits
                .iter()
                .all(|hit| !matches!(hit.action, render::Action::Quote(_))),
            "width {cols} has no quote row"
        );
        let (x, y) = locate(&ui, "reply");
        assert_eq!(
            ui.view.hit_at(x, y),
            None,
            "the prompt text is not a button"
        );
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::default();
        ui.quote(request);
        assert_eq!(ui.locals[&room].text.text, "author: \"reply\"\n");
    }
}

#[test]
fn quote_keeps_the_agent_reply_as_raw_markdown() {
    let (mut ui, room, agent) = fixture();
    let markdown = "**bold** and [docs](https://example.com) and `code`";
    let request = saved_exchange(&mut ui, room, agent, markdown, markdown);
    ui.compute_view(100, 40);
    let stored = ui
        .history
        .cached()
        .iter()
        .find_map(|line| match &line.raw_markdown {
            Some((history::MarkdownSource::Prompt(_), raw)) => Some(raw.to_string()),
            _ => None,
        })
        .expect("prompt line stores its markdown");
    assert_eq!(stored, markdown);
    assert!(ui
        .view
        .hits
        .iter()
        .all(|hit| hit.action != render::Action::Quote(request)));

    ui.quote(request);

    assert_eq!(
        ui.locals[&room].text.text,
        "author: \"**bold** and [docs](https://example.com) and `code`\"\n"
    );
}

#[test]
fn markdown_reply_rendering_is_reused_when_only_timestamp_age_changes() {
    let (mut ui, room, agent) = fixture();
    saved_exchange(&mut ui, room, agent, "**cached reply**", "hidden reply");
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
        .filter(|line| is_prompt(line))
        .find_map(|line| line.raw_markdown.as_ref().map(|(_, raw)| Arc::clone(raw)))
        .expect("rendered prompt source");
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
        .filter(|line| is_prompt(line))
        .find_map(|line| line.raw_markdown.as_ref().map(|(_, raw)| Arc::clone(raw)))
        .expect("rendered prompt source after timestamp refresh");

    assert!(
        Arc::ptr_eq(&first, &second),
        "timestamp refreshes reuse the immutable prompt rendering"
    );
}

#[test]
fn markdown_rendering_survives_a_visit_to_another_room() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    let other_agent = snapshot
        .state
        .create_agent(other, "helper", Provider::Codex, "/project".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    saved_exchange(&mut ui, room, agent, "**first reply**", "hidden reply");
    saved_exchange(
        &mut ui,
        other,
        other_agent,
        "**other reply**",
        "hidden reply",
    );
    let snapshot = Arc::clone(&ui.snapshot);
    let rendered_reply = |ui: &mut BusUi, id| {
        ui.history.lines(
            &snapshot.state,
            snapshot.state.room(id).expect("room"),
            80,
            snapshot.revision,
            1_000,
            &mut Default::default(),
        );
        ui.history
            .cached()
            .iter()
            .filter(|line| is_prompt(line))
            .find_map(|line| line.raw_markdown.as_ref().map(|(_, raw)| Arc::clone(raw)))
            .expect("rendered prompt source")
    };

    let first = rendered_reply(&mut ui, room);
    rendered_reply(&mut ui, other);
    let back = rendered_reply(&mut ui, room);

    assert!(
        Arc::ptr_eq(&first, &back),
        "returning to a room reuses its rendered Markdown instead of re-rendering the whole room"
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
        "## 日本語 👩‍💻\n\n**強調 text**\n\n| A | B |\n| - | - |\n| 甲 | 🙂 |",
        "hidden reply",
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
        .filter(|line| is_prompt(line))
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

fn copy_markdown_body_at_width(ui: &mut BusUi, room: RoomId, width: u16) -> String {
    let snapshot = Arc::clone(&ui.snapshot);
    ui.history.lines(
        &snapshot.state,
        snapshot.state.room(room).unwrap(),
        width,
        snapshot.revision,
        2_000,
        &mut Default::default(),
    );
    let body = |line: &history::Line| is_prompt(line) && !line.text.trim().is_empty();
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
        "- alpha beta gamma delta epsilon zeta",
        "hidden reply",
    );

    assert_eq!(
        copy_markdown_body_at_width(&mut ui, room, 20),
        "• alpha beta gamma delta epsilon zeta"
    );
}

#[test]
fn markdown_returning_to_a_room_after_a_resize_elsewhere_lays_out_at_the_new_width() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    let other_agent = snapshot
        .state
        .create_agent(other, "helper", Provider::Codex, "/project".into(), None)
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau";
    saved_exchange(&mut ui, room, agent, long, "hidden reply");
    saved_exchange(
        &mut ui,
        other,
        other_agent,
        "**other reply**",
        "hidden reply",
    );
    let snapshot = Arc::clone(&ui.snapshot);
    let reply_rows = |ui: &mut BusUi, id, width| -> Vec<String> {
        ui.history.lines(
            &snapshot.state,
            snapshot.state.room(id).expect("room"),
            width,
            snapshot.revision,
            1_000,
            &mut Default::default(),
        );
        ui.history
            .cached()
            .iter()
            .filter(|line| is_prompt(line))
            .map(|line| line.text.clone())
            .collect()
    };

    let wide = reply_rows(&mut ui, room, 120);
    reply_rows(&mut ui, other, 30);
    let narrow = reply_rows(&mut ui, room, 30);

    assert!(narrow.len() > wide.len(), "wide {wide:?} narrow {narrow:?}");
    assert!(
        narrow
            .iter()
            .all(|row| unicode_width::UnicodeWidthStr::width(row.as_str()) <= 30),
        "rows fit the new width: {narrow:?}"
    );
}

#[test]
fn quote_padding_in_a_scrolled_history_is_inert_while_neighbor_rows_still_select() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::*};

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
    ui.main_scroll = target - 5;
    ui.compute_view(100, 40);
    assert_eq!(ui.main_scroll, target - 5);
    let text = ui.view.history_text;
    let edge = text.right() - 1;
    let mut selectable = 0;
    for row in text.y..text.bottom() {
        let line = ui.history.cached()[ui.main_scroll + usize::from(row - text.y)].clone();
        assert!(
            !matches!(line.action, Some(render::Action::Quote(_))),
            "row {row} is not a quote button: {:?}",
            line.text
        );
        ui.history_selection = None;
        assert_eq!(pointer(&mut ui, Down(Left), edge, row), None);
        let selects =
            line.action.is_none() || matches!(line.action, Some(render::Action::OpenFile(_)));
        assert_eq!(
            ui.history_selection.is_some(),
            selects,
            "row {row} ({:?}) padding press",
            line.text
        );
        selectable += usize::from(selects);
        pointer(&mut ui, Up(Left), edge, row);
    }
    assert!(
        selectable > 1,
        "prompt rows on the scrolled screen still select"
    );
}
