use super::*;
use crate::{
    bus::{model::*, runtime::*},
    input::TerminalKey,
    raw_input::RawInputEvent,
};
use crossterm::event::{KeyCode, KeyModifiers};
use std::sync::Arc;

#[path = "support_test.rs"]
mod support;
use support::*;

#[path = "deletion_test.rs"]
mod deletion_tests;

#[path = "sidebar_test.rs"]
mod sidebar_tests;

#[path = "toasts_test.rs"]
mod toasts_tests;

#[path = "native_shell_test.rs"]
mod native_shell_tests;

#[path = "chat_search_test.rs"]
mod chat_search_tests;

#[path = "history_markdown_test.rs"]
mod history_markdown_tests;

#[path = "history_slots_test.rs"]
mod history_slots_tests;

#[path = "history_scroll_test.rs"]
mod history_scroll_tests;

#[path = "keys_test.rs"]
mod keys_tests;

#[path = "master_test.rs"]
mod master_tests;

#[path = "sound_test.rs"]
mod sound_tests;

#[test]
fn creation_events_survive_snapshots_from_before_the_created_target() {
    let (mut ui, old_room, _) = fixture();
    let mut created = (*ui.snapshot).clone();
    let room = created.state.create_room("new room").unwrap();
    let mut intermediate = (*ui.snapshot).clone();
    intermediate.revision += 1;
    ui.receive_event(BusEvent::RoomCreated(room));
    ui.receive_snapshot(Arc::new(intermediate));
    assert_eq!(ui.room, Some(room), "intermediate snapshot is not deletion");
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::SelectRoom(id) if id == room)));
    created.revision += 2;
    ui.receive_snapshot(Arc::new(created.clone()));
    assert!(ui.locals.contains_key(&room));
    assert!(ui.locals.contains_key(&old_room));

    let mut added = created.clone();
    let agent = added
        .state
        .create_agent(room, "new agent", Provider::Codex, "/project".into(), None)
        .unwrap();
    created.revision += 1;
    ui.receive_event(BusEvent::AgentAdded(agent));
    ui.receive_snapshot(Arc::new(created));
    assert_eq!(ui.terminal, Some(agent));
    assert!(ui
        .pending
        .iter()
        .any(|pending| matches!(pending.command, BusCommand::FocusTerminal(id) if id == agent)));
    added.revision += 2;
    ui.receive_snapshot(Arc::new(added));
    assert_eq!(ui.terminal, Some(agent));
}

#[test]
fn typing_plus_in_the_composer_inserts_plus() {
    for input in shifted_symbol_encodings('+', '=') {
        let (mut ui, room, _) = fixture();
        ui.input(
            &RawInputEvent::Key(input.clone()),
            false,
            &mut Default::default(),
        );
        assert_eq!(ui.locals[&room].text.text, "+", "for {input:?}");
        assert!(ui.form.is_none() && !ui.recipient_menu, "for {input:?}");
    }
}

#[test]
fn every_shifted_symbol_types_into_the_composer() {
    let shifted = [
        ('~', '`'),
        ('!', '1'),
        ('@', '2'),
        ('#', '3'),
        ('$', '4'),
        ('%', '5'),
        ('^', '6'),
        ('&', '7'),
        ('*', '8'),
        ('(', '9'),
        (')', '0'),
        ('_', '-'),
        ('+', '='),
        ('{', '['),
        ('}', ']'),
        ('|', '\\'),
        (':', ';'),
        ('"', '\''),
        ('<', ','),
        ('>', '.'),
        ('?', '/'),
    ];
    for (symbol, base) in shifted {
        for input in shifted_symbol_encodings(symbol, base) {
            let (mut ui, room, agent) = fixture();
            ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
            ui.input(
                &RawInputEvent::Key(input.clone()),
                false,
                &mut Default::default(),
            );
            assert_eq!(
                ui.locals[&room].text.text,
                symbol.to_string(),
                "{symbol} was not typed for {input:?}"
            );
            assert!(ui.form.is_none() && !ui.recipient_menu);
            assert!(ui.send_intent.is_none());
        }
    }
}

#[test]
fn ctrl_chords_open_the_agent_and_file_pickers() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("keep draft");
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(ui.recipient_menu && ui.form.is_none());
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("keep draft");
    key(&mut ui, KeyCode::Char('o'), KeyModifiers::CONTROL);
    assert!(matches!(ui.form, Some(forms::Form::Files(_))));
    assert_eq!(ui.locals[&room].text.text, "keep draft");
}

#[test]
fn symbols_stay_literal_in_paste_notes_and_forms() {
    let (mut ui, room, _) = fixture();
    for event in [
        RawInputEvent::Paste("@author + file.md".into()),
        RawInputEvent::Text(crate::input::TextCommit::new("@+")),
    ] {
        ui.input(&event, false, &mut Default::default());
    }
    assert_eq!(ui.locals[&room].text.text, "@author + file.md@+");
    assert!(!ui.recipient_menu && ui.form.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    for symbol in ['@', '+'] {
        key(&mut ui, KeyCode::Char(symbol), KeyModifiers::SHIFT);
    }
    assert_eq!(ui.locals[&room].notes.text, "@+");
    assert!(!ui.recipient_menu && ui.form.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('o'), KeyModifiers::CONTROL);
    for symbol in ['@', '+'] {
        key(&mut ui, KeyCode::Char(symbol), KeyModifiers::SHIFT);
    }
    let Some(forms::Form::Files(editor)) = &ui.form else {
        panic!("file form lost")
    };
    assert_eq!(editor.text, "~/@+");
    assert!(ui.send_intent.is_none());
}

#[test]
fn settings_space_does_not_toggle_color_blind_mode() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);

    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);

    assert!(!ui.settings.color_blind_mode);
}

#[test]
fn settings_footer_advertises_enter_as_the_only_toggle_key() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::Settings);

    let screen = room_screen(&mut ui, 100, 30);

    assert!(screen.contains("Enter toggles"), "{screen}");
    assert!(!screen.contains("Space"), "{screen}");
}

#[test]
fn composer_box_encloses_controls_and_grows_with_matching_column_divider() {
    let (mut ui, room, _) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        ui.compute_view(cols, rows);
        let mut buffer =
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        ui.render(&mut buffer);
        let editor = composer_rect(&ui);
        let top = editor.y - 3;
        let left = editor.x - 1;
        let right = editor.right();
        let bottom = editor.bottom() + 1;
        let divider = ui.view.sidebar.right() - 1;
        let color = buffer[(divider, 0)].fg;
        assert_eq!(buffer[(divider, 0)].symbol(), "│");
        assert_ne!(color, buffer[(editor.x, editor.y)].fg);
        for y in 0..rows {
            assert_eq!(buffer[(divider, y)].symbol(), "│");
            assert_eq!(buffer[(divider, y)].fg, color);
        }
        for (x, y, symbol) in [
            (left, top, "┌"),
            (right, top, "┐"),
            (left, bottom, "└"),
            (right, bottom, "┘"),
        ] {
            assert_eq!(buffer[(x, y)].symbol(), symbol);
            assert_eq!(buffer[(x, y)].fg, color);
        }
        for x in left + 1..right {
            for y in [top, bottom] {
                assert_eq!(buffer[(x, y)].symbol(), "─");
                assert_eq!(buffer[(x, y)].fg, color);
            }
        }
        for y in top + 1..bottom {
            for x in [left, right] {
                let symbol = if y == editor.y - 1 {
                    if x == left {
                        "├"
                    } else {
                        "┤"
                    }
                } else {
                    "│"
                };
                assert_eq!(buffer[(x, y)].symbol(), symbol);
                assert_eq!(buffer[(x, y)].fg, color);
            }
        }
        assert_eq!(buffer[(editor.x, top + 1)].symbol(), "+");
        assert_eq!(buffer[(editor.x + 3, top + 1)].symbol(), "@");
    }
}

#[test]
fn routine_footer_hints_are_hidden_in_rooms_and_forms() {
    let (mut ui, _, _) = fixture();
    for form in [false, true] {
        if form {
            ui.action(render::Action::NewRoom);
        }
        let screen = room_screen(&mut ui, 120, 40);
        for hint in ["Ctrl+Q quit", "F2 rename", "Enter send", "Tab fields"] {
            assert!(!screen.contains(hint), "persistent shortcut: {hint}");
        }
    }
}

#[test]
fn composer_toolbar_has_a_matching_horizontal_line_above_editable_text() {
    let (mut ui, room, _) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        ui.compute_view(cols, rows);
        let mut buffer =
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        ui.render(&mut buffer);
        let editor = composer_rect(&ui);
        let color = buffer[(ui.view.sidebar.right() - 1, 0)].fg;
        for x in editor.x..editor.right() {
            assert_eq!(buffer[(x, editor.y - 1)].symbol(), "─");
            assert_eq!(buffer[(x, editor.y - 1)].fg, color);
        }
        assert_eq!(buffer[(editor.x - 1, editor.y - 1)].symbol(), "├");
        assert_eq!(buffer[(editor.right(), editor.y - 1)].symbol(), "┤");
        assert_eq!(buffer[(editor.x, editor.y - 2)].symbol(), "+");
        assert_eq!(buffer[(editor.x + 3, editor.y - 2)].symbol(), "@");
        assert_eq!(buffer[(editor.x, editor.y)].symbol(), "d");
    }
}

#[test]
fn room_notes_have_a_box_and_top_section_is_separated_from_history() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, "history body").unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.submit_draft(room, 10).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().notes.insert("Room notes");
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let color = buffer[(27, 0)].fg;
    // One line of notes gets a one-row box; history starts right below it.
    for (x, y, symbol) in [(29, 2, "┌"), (98, 2, "┐"), (29, 4, "└"), (98, 4, "┘")] {
        assert_eq!(buffer[(x, y)].symbol(), symbol);
        assert_eq!(buffer[(x, y)].fg, color);
    }
    for x in 30..98 {
        for y in [2, 4, 5] {
            assert_eq!(buffer[(x, y)].symbol(), "─");
            assert_eq!(buffer[(x, y)].fg, color);
        }
    }
    assert_eq!(buffer[(30, 3)].symbol(), "R");
    assert_eq!(buffer[(30, 6)].symbol(), "Y");
    assert_eq!(buffer[(30, 7)].symbol(), "h");
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('!'), KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].notes.text, "Room notes!");
    assert!(ui.send_intent.is_none());
}

#[test]
fn composer_sits_on_last_row_unless_a_visible_notice_needs_it() {
    let (mut ui, room, _) = fixture();
    for (cols, rows, lines) in [(100, 30, 1), (100, 30, 80), (60, 12, 80)] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("draft\n".repeat(lines));
        ui.toast = None;
        for notice in [false, true] {
            if notice {
                ui.show_toast("Save failed");
            }
            ui.compute_view(cols, rows);
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
            ui.render(&mut buffer);
            let editor = composer_rect(&ui);
            let border_y = rows - 1 - u16::from(notice);
            assert_eq!(editor.bottom() + 1, border_y, "no empty footer row");
            assert_eq!(buffer[(editor.x - 1, border_y)].symbol(), "└");
            assert_eq!(buffer[(editor.right(), border_y)].symbol(), "┘");
            for x in editor.x..editor.right() {
                assert_eq!(buffer[(x, border_y)].symbol(), "─");
            }
            if notice {
                let footer: String = (editor.x..editor.right())
                    .map(|x| buffer[(x, rows - 1)].symbol())
                    .collect();
                assert!(footer.starts_with("Save failed"));
            }
        }
        ui.toast.as_mut().unwrap().started_at -= std::time::Duration::from_secs(15);
        ui.compute_view(cols, rows);
        assert_eq!(composer_rect(&ui).bottom() + 1, rows - 1);
    }
}

#[test]
fn history_scroll_moves_one_row_per_event() {
    use crossterm::event::MouseEventKind::ScrollUp;
    let (mut ui, room, agent) = fixture();
    let history = (0..80)
        .map(|i| format!("history-{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, &history).unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.submit_draft(room, 10).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    room_screen(&mut ui, 100, 30);
    let bottom = ui.main_scroll;
    assert!(bottom > 1);

    mouse(&mut ui, ScrollUp, 40, 12);

    assert_eq!(ui.main_scroll, bottom - 1);
}

#[test]
fn history_scrolls_and_clamps_without_moving_or_editing_the_composer() {
    use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
    let (mut ui, room, agent) = fixture();
    let history = (0..80)
        .map(|i| format!("history-{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_text(room, &history).unwrap();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.submit_draft(room, 10).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("unsent draft");
    assert!(room_screen(&mut ui, 100, 30).contains("history-79"));
    let composer = composer_rect(&ui);
    for _ in 0..80 {
        mouse(&mut ui, ScrollDown, 40, 12);
        ui.compute_view(100, 30);
    }
    let bottom = room_screen(&mut ui, 100, 30);
    assert!(
        bottom.contains("history-79"),
        "last history line must remain visible after repeated scrolling"
    );
    assert!(!bottom.contains("history-00"));
    assert_eq!(composer_rect(&ui), composer);
    assert_eq!(ui.locals[&room].text.text, "unsent draft");
    assert_eq!(ui.locals[&room].composer_scroll, None);
    assert_eq!(ui.sidebar_scroll, 0);
    for _ in 0..80 {
        mouse(&mut ui, ScrollUp, 40, 12);
        ui.compute_view(100, 30);
    }
    assert!(room_screen(&mut ui, 100, 30).contains("history-00"));
    mouse(&mut ui, ScrollDown, 40, 3); // Notes are not the history viewport.
    assert_eq!(ui.main_scroll, 0);
}

#[test]
fn help_enter_is_local_and_preserves_room_context() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .notes
        .insert("remember this");
    let before = serde_json::to_value(&ui.snapshot.state).unwrap();
    ui.input(
        &RawInputEvent::Paste("/help".into()),
        false,
        &mut Default::default(),
    );
    assert!(ui.form.is_none(), "paste alone must not execute commands");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 120, 40).contains("Keyboard shortcuts"));
    assert!(ui.locals[&room].text.text.is_empty());
    assert_eq!(ui.locals[&room].recipients, [agent].into());
    assert_eq!(ui.locals[&room].notes.text, "remember this");
    assert_eq!(serde_json::to_value(&ui.snapshot.state).unwrap(), before);
    assert!(ui.send_intent.is_none());
    assert!(ui
        .pending
        .iter()
        .all(|p| matches!(p.command, BusCommand::SetDraftText(..))));
    ui.input(
        &RawInputEvent::Paste("do not insert into draft".into()),
        false,
        &mut Default::default(),
    );
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn help_scrolls_on_short_terminals_without_sending() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert(" /help ");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    let start = room_screen(&mut ui, 60, 12);
    assert!(start.contains("Keyboard shortcuts"));
    key(&mut ui, KeyCode::PageDown, KeyModifiers::NONE);
    assert_ne!(room_screen(&mut ui, 60, 12), start);
    key(&mut ui, KeyCode::End, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 60, 12).contains("Ctrl+Q"));
    key(&mut ui, KeyCode::Home, KeyModifiers::NONE);
    assert_eq!(room_screen(&mut ui, 60, 12), start);
    assert!(ui.send_intent.is_none());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.form.is_none());
}

#[test]
fn help_only_intercepts_the_whole_composer_command() {
    for text in ["/help me with this", "quote: /help", "/helper"] {
        let (mut ui, room, agent) = fixture();
        ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
        ui.locals.get_mut(&room).unwrap().text.insert(text);
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(ui.send_intent, Some(room));
        assert!(ui.form.is_none());
        assert_eq!(ui.locals[&room].text.text, text);
    }
    let (mut ui, room, _) = fixture();
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("/help".into()),
        false,
        &mut Default::default(),
    );
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.locals[&room].notes.text, "/help\n");
    assert!(ui.form.is_none());
}

#[test]
fn help_cancels_same_room_unsent_intent_before_it_can_become_an_agent_prompt() {
    let (mut ui, room, agent) = fixture();
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("original prompt");
    ui.text_changed(room);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.send_intent, Some(room));
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("/help".into());
    ui.text_changed(room);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Keyboard shortcuts"));
    assert!(ui.send_intent.is_none());
    let mut snapshot = (*ui.snapshot).clone();
    for pending in &mut ui.pending {
        pending.result = Some(Ok(()));
        snapshot.last_command_id = pending.id;
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::Submit(_))));
    assert!(ui.locals[&room].text.text.is_empty());
}

#[test]
fn enter_without_checked_agents_shows_error_and_preserves_draft() {
    let (mut ui, room, _) = fixture();
    for text in ["hello", "@author hello", "/help me", "/other", ""] {
        ui.locals.get_mut(&room).unwrap().text = editor::Editor::new(text.into());
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert!(ui.send_intent.is_none());
        assert!(ui
            .pending
            .iter()
            .all(|p| !matches!(p.command, BusCommand::Submit(_))));
        assert_eq!(ui.locals[&room].text.text, text);
        assert!(room_screen(&mut ui, 100, 30).contains("Choose agents with @ before sending."));
    }
    ui.locals.get_mut(&room).unwrap().text = editor::Editor::new("/help".into());
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Keyboard shortcuts"));
    assert!(ui.send_intent.is_none());
}

#[test]
fn agent_enter_adds_from_non_model_fields_with_its_own_directory_and_escape_cancels() {
    for field in [0, 2, 3] {
        let (mut ui, room, _) = fixture();
        ui.form = Some(forms::Form::Agent {
            name: editor::Editor::new("frontend".into()),
            provider: Some(Provider::ClaudeCode),
            provider_cursor: Provider::ClaudeCode,
            cwd: editor::Editor::new("/projects/frontend".into()),
            args: Box::new(editor::Editor::new("--model sonnet".into())),
            field,
            orchestrates: None,
            prompt: None,
        });
        // Suggestions must not hijack Enter; Tab remains path completion.
        ui.suggestions
            .entries
            .push(crate::bus::launch::PathSuggestion {
                path: "/projects/backend".into(),
                is_directory: true,
            });
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        assert!(ui
            .pending
            .iter()
            .any(|p| matches!(&p.command, BusCommand::AddAgent(input)
            if input.room == room && input.name == "frontend" && input.cwd == "/projects/frontend"
            && input.provider == Provider::ClaudeCode && input.extra_args == "--model sonnet")));
        let (mut ui, _, _) = fixture();
        ui.action(render::Action::NewAgent);
        key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
        assert!(ui.form.is_none());
        assert!(!ui
            .pending
            .iter()
            .any(|p| matches!(p.command, BusCommand::AddAgent(_))));
    }
}

#[test]
fn new_agent_starts_without_a_selected_model() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);

    assert!(room_screen(&mut ui, 100, 30).contains("Choose model"));
}

#[test]
fn add_agent_form_keeps_model_and_pwd_labels_concise() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Provider(Provider::ClaudeCode));

    let screen = room_screen(&mut ui, 100, 30);
    assert!(screen.contains("< Claude Code >"), "{screen}");
    assert!(screen.contains("PWD"), "{screen}");
    assert!(!screen.contains("arrows /"), "{screen}");
    assert!(!screen.contains("(this agent only)"), "{screen}");
}

#[test]
fn model_menu_enter_selects_without_submitting_or_showing_an_error() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Field(1));

    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);

    assert!(matches!(ui.form, Some(forms::Form::Agent { field: 2, .. })));
    assert!(ui
        .pending
        .iter()
        .all(|pending| !matches!(pending.command, BusCommand::AddAgent(_))));
    assert!(ui.visible_error().is_none());
    assert!(room_screen(&mut ui, 100, 30).contains("Claude Code"));
}

#[test]
fn add_agent_rejects_each_missing_required_field_and_keeps_the_form_open() {
    for missing in ["Name", "Model", "PWD"] {
        let (mut ui, _, _) = fixture();
        ui.action(render::Action::NewAgent);
        if let Some(forms::Form::Agent { name, cwd, .. }) = &mut ui.form {
            name.text = "frontend".into();
            if missing == "PWD" {
                cwd.text.clear();
            }
        }
        if missing != "Model" {
            ui.action(render::Action::Provider(Provider::Codex));
        }
        if missing == "Name" {
            if let Some(forms::Form::Agent { name, .. }) = &mut ui.form {
                name.text.clear();
            }
        }

        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);

        assert!(
            matches!(ui.form, Some(forms::Form::Agent { .. })),
            "{missing}"
        );
        assert!(
            ui.pending
                .iter()
                .all(|pending| !matches!(pending.command, BusCommand::AddAgent(_))),
            "{missing}"
        );
        assert!(
            ui.visible_error()
                .is_some_and(|error| error.contains(missing)),
            "{missing}: {:?}",
            ui.visible_error()
        );
    }
}

#[test]
fn room_creation_needs_only_name_and_does_not_inherit_agent_pwd_error() {
    let (mut ui, _, _) = fixture();
    ui.error = Some("Agent PWD must be an absolute path or ~/ path".into());
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = ui.error.clone();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.action(render::Action::NewRoom);
    ui.input(
        &RawInputEvent::Paste("frontend-and-backend".into()),
        false,
        &mut Default::default(),
    );
    assert!(!room_screen(&mut ui, 120, 40).contains("PWD"));
    // A new failure with identical text is still visible, as are unrelated
    // runtime failures; dismissing an old operation is not a blanket mute.
    ui.error = Some("Agent PWD must be an absolute path or ~/ path".into());
    assert!(room_screen(&mut ui, 120, 40).contains("PWD"));
    ui.error = None;
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.error = Some("Runtime connection unavailable".into());
    ui.receive_snapshot(Arc::new(snapshot));
    assert!(room_screen(&mut ui, 120, 40).contains("Runtime connection unavailable"));
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert!(ui.pending.iter().any(
        |p| matches!(&p.command, BusCommand::CreateRoom(name) if name == "frontend-and-backend")
    ));
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewRoom);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    assert!(ui.form.is_none());
    assert!(!ui
        .pending
        .iter()
        .any(|p| matches!(p.command, BusCommand::CreateRoom(_))));
}

#[test]
fn composer_autosave_is_silent_but_save_errors_remain_visible() {
    let (mut ui, room, _) = fixture();
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(ui.pending.iter().any(|pending| {
        matches!(&pending.command, BusCommand::SetDraftText(id, text) if *id == room && text == "x")
    }), "typing must still autosave");
    assert!(!room_screen(&mut ui, 100, 30).contains("Saving"));
    ui.error = Some("Could not save: disk full".into());
    assert!(room_screen(&mut ui, 100, 30).contains("Could not save: disk full"));
}

#[test]
fn composer_grows_with_newlines_and_wrapping_then_shrinks_after_deletion() {
    let (mut ui, room, _) = fixture();
    ui.compute_view(100, 30);
    let compact = composer_rect(&ui);
    for _ in 0..7 {
        key(&mut ui, KeyCode::Enter, KeyModifiers::SHIFT);
    }
    ui.compute_view(100, 30);
    let grown = composer_rect(&ui);
    assert_eq!(grown.height, 8, "every new blank line gets space");
    assert!(grown.y < compact.y);
    assert_eq!(grown.bottom(), compact.bottom());
    assert!(ui.send_intent.is_none());
    for _ in 0..7 {
        key(&mut ui, KeyCode::Backspace, KeyModifiers::NONE);
    }
    ui.compute_view(100, 30);
    assert_eq!(composer_rect(&ui), compact);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"界".repeat(150));
    ui.compute_view(100, 30);
    assert_eq!(composer_rect(&ui).height, 5, "wide wrapped text grows too");
    ui.compute_view(80, 30);
    assert!(
        composer_rect(&ui).height > 5,
        "resizing recomputes visual rows"
    );
}

#[test]
fn composer_uses_full_height_and_toggle_preserves_draft_and_recipients() {
    let (mut ui, room, agent) = fixture();
    let draft = (0..80)
        .map(|n| format!("line-{n:02}\n"))
        .collect::<String>();
    ui.locals.get_mut(&room).unwrap().text.insert(&draft);
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    room_screen(&mut ui, 100, 30);
    let full = composer_rect(&ui);
    assert_eq!(
        full.height,
        30 - ui.view.recipient_bar.height - 4,
        "use every row below the recipient chips and borders"
    );
    assert!(full.bottom() <= 28);
    key(
        &mut ui,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    room_screen(&mut ui, 100, 30);
    assert_eq!(composer_rect(&ui).height, 3);
    key(
        &mut ui,
        KeyCode::Char('e'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    );
    room_screen(&mut ui, 100, 30);
    assert_eq!(composer_rect(&ui), full);
    assert_eq!(ui.locals[&room].text.text, draft);
    assert_eq!(ui.locals[&room].recipients, [agent].into());
    assert!(ui.send_intent.is_none());
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("Add notes"));
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    assert!(room_screen(&mut ui, 100, 30).contains("[x] author"));
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(ui.send_intent, Some(room));
}

#[test]
fn composer_page_keys_scroll_without_editing_and_typing_returns_to_caret() {
    let (mut ui, room, _) = fixture();
    let draft = (0..90)
        .map(|n| format!("draft-{n:02}\n"))
        .collect::<String>();
    ui.locals.get_mut(&room).unwrap().text.insert(&draft);
    let bottom = room_screen(&mut ui, 100, 30);
    assert!(bottom.contains("draft-89"));
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    let previous = room_screen(&mut ui, 100, 30);
    assert_ne!(previous, bottom, "Page Up must move the draft viewport");
    assert!(!previous.contains("draft-89"));
    assert!(ui.cursor().is_none(), "do not show a fake cursor offscreen");
    assert_eq!(ui.locals[&room].text.text, draft);
    assert_eq!(ui.locals[&room].text.cursor, draft.len());
    assert_eq!(
        ui.main_scroll, 0,
        "draft scrolling is separate from replies"
    );
    key(&mut ui, KeyCode::PageDown, KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("draft-89"));
    for _ in 0..10 {
        key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
        ui.compute_view(100, 30);
    }
    assert!(room_screen(&mut ui, 100, 30).contains("draft-00"));
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(room_screen(&mut ui, 100, 30).contains("draft-89"));
    assert!(ui.cursor().is_some());
    assert_eq!(ui.locals[&room].text.text, format!("{draft}x"));
    for (cols, rows) in [(60, 12), (100, 45)] {
        room_screen(&mut ui, cols, rows);
        let rect = composer_rect(&ui);
        let cursor = ui.cursor().unwrap();
        assert!(rect.bottom() < rows);
        assert!(rect.contains((cursor.x, cursor.y).into()));
    }
}

#[test]
fn composer_full_height_picker_covers_draft() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"underlying-draft-suffix-should-not-bleed-through\n".repeat(80));
    ui.compute_view(100, 30);
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    ui.compute_view(100, 30);
    let hit = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::Recipient(None))
        .unwrap();
    let rect = hit.rect;
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let label: String = (rect.x..rect.right())
        .map(|x| buffer[(x, rect.y)].symbol())
        .collect();
    assert_eq!(label.trim(), "[ ] All");
}

#[test]
fn composer_full_height_long_file_details_stay_readable() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"long draft\n".repeat(80));
    ui.detail_path = Some(format!("/tmp/{}/required-detail-tail.md", "a".repeat(70)));
    assert!(room_screen(&mut ui, 100, 30).contains("required-detail-tail.md"));
}

#[test]
fn composer_yields_space_to_visible_notes_in_short_terminal() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&"long draft\n".repeat(80));
    room_screen(&mut ui, 60, 12);
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("visible notes".into()),
        false,
        &mut Default::default(),
    );
    assert!(room_screen(&mut ui, 60, 12).contains("visible notes"));
    let cursor = ui.cursor().expect("visible notes caret");
    let notes = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::Notes)
        .unwrap()
        .rect;
    assert!(notes.contains((cursor.x, cursor.y).into()));
}

#[test]
fn consecutive_sends_keep_checked_recipients_and_queue_the_successor() {
    // Resetting persisted recipients after Submit breaks the second Enter even
    // though the actual UI still shows the same checked recipient.
    fn process_commands(ui: &mut BusUi, state: &mut BusState) {
        ui.settle();
        while let Some(pending) = ui.pending.front().cloned() {
            let result = match pending.command {
                BusCommand::SetDraftText(room, text) => state.set_draft_text(room, &text),
                BusCommand::SetRecipients(room, ids) => state.set_draft_recipients(room, ids),
                BusCommand::Submit(room) => state.submit_draft(room, 10).map(|_| ()),
                command => panic!("unexpected command: {command:?}"),
            };
            ui.receive_event(BusEvent::CommandFinished {
                command_id: pending.id,
                result: result.map_err(|error| error.to_string()),
            });
            ui.receive_snapshot(Arc::new(BusSnapshot {
                state: state.clone(),
                revision: pending.id,
                last_command_id: pending.id,
                error: None,
            }));
            ui.settle();
        }
    }

    let (mut ui, room, agent) = fixture();
    let mut state = ui.snapshot.state.clone();
    let file = std::path::PathBuf::from("/project/context.md");
    state.attach_file(room, file.clone()).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    key(&mut ui, KeyCode::Char('p'), KeyModifiers::CONTROL);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Esc, KeyModifiers::NONE);
    for text in ["first prompt", "queued successor"] {
        ui.input(
            &RawInputEvent::Paste(text.into()),
            false,
            &mut Default::default(),
        );
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
        process_commands(&mut ui, &mut state);
        assert!(ui.error.is_none(), "send failed: {:?}", ui.error);
        assert!(ui.locals[&room].text.text.is_empty());
        assert_eq!(ui.locals[&room].recipients, [agent].into());
        assert!(state.room(room).unwrap().draft.text.is_empty());
        assert!(state.room(room).unwrap().draft.files.is_empty());
        if text == "first prompt" {
            let first = state.queued_requests(agent)[0];
            assert_eq!(
                state.request(first).unwrap().prompt.files,
                vec![file.clone()]
            );
            state.begin_submission(first, "launch", 0).unwrap();
            state
                .record_submission(
                    first,
                    SubmissionOutcome::Confirmed {
                        provider_session_id: Some("session".into()),
                        provider_turn_id: Some("turn".into()),
                    },
                )
                .unwrap();
        }
    }
    let first = state.agent(agent).unwrap().current_request.unwrap();
    assert_eq!(state.request(first).unwrap().phase, RequestPhase::Active);
    assert_eq!(state.request(first).unwrap().prompt.text, "first prompt");
    assert_eq!(state.queued_requests(agent).len(), 1);
    let next = state.request(state.queued_requests(agent)[0]).unwrap();
    assert_eq!(next.phase, RequestPhase::Queued);
    assert_eq!(next.prompt.text, "queued successor");
    assert_eq!(next.prompt.recipient_ids, [agent].into());
    assert!(next.prompt.files.is_empty());
    assert_eq!(
        state.room(room).unwrap().draft.recipient_ids,
        [agent].into()
    );
    assert_eq!(ui.snapshot.state, state);
}

#[test]
fn direct_text_render_filters_terminal_controls_without_changing_source() {
    let (mut ui, room, _) = fixture();
    let dangerous = "x\u{1b}]52;c;ZXZpbA==\u{7}y";
    ui.locals.get_mut(&room).unwrap().text.insert(dangerous);
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    assert!(buffer
        .content
        .iter()
        .all(|cell| !cell.symbol().chars().any(char::is_control)));
    assert_eq!(ui.locals[&room].text.text, dangerous);
}

#[test]
fn wrap_ranges_break_between_words_and_cover_every_source_byte() {
    let text = "hello world\nabcdefghij 界界xy";
    let rows: Vec<_> = render::wrap_ranges(text, 5)
        .into_iter()
        .map(|range| &text[range])
        .collect();
    assert_eq!(rows, ["hello ", "world", "abcde", "fghij ", "界界", "xy"]);
    assert_eq!(rows.concat(), text.replace('\n', ""));
}

#[test]
fn composer_wraps_whole_words_and_places_the_caret_on_the_wrapped_row() {
    let (mut ui, room, _) = fixture();
    ui.compute_view(100, 30);
    let width = usize::from(composer_rect(&ui).width);
    let filler = "a".repeat(width - 3);
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert(&format!("{filler} hello world"));
    assert_eq!(
        composer_rows(&mut ui, 100, 30)[..2],
        [filler, "hello world".into()]
    );
    let rect = composer_rect(&ui);
    let cursor = ui.cursor().unwrap();
    assert_eq!((cursor.x, cursor.y), (rect.x + 11, rect.y + 1));
    // The caret before a wrapped word sits where that word starts.
    ui.locals.get_mut(&room).unwrap().text.cursor = width - 2;
    ui.compute_view(100, 30);
    let cursor = ui.cursor().unwrap();
    assert_eq!((cursor.x, cursor.y), (rect.x, rect.y + 1));
}

#[test]
fn dragging_a_rendered_prompt_copies_exactly_the_dragged_text_and_highlights_it() {
    let (mut ui, room, agent) = fixture();
    let source = "alpha **beta** gamma delta epsilon zeta eta theta\nsecond line";
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state.set_draft_recipients(room, [agent]).unwrap();
    snapshot.state.set_draft_text(room, source).unwrap();
    snapshot.state.submit_draft(room, 1).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(60, 30);
    let text = ui.view.history_text;
    let row_of = |ui: &BusUi, prefix: &str| {
        let index = ui
            .history
            .cached()
            .iter()
            .position(|line| line.text.starts_with(prefix))
            .unwrap();
        text.y + (index - ui.main_scroll) as u16
    };
    let first = row_of(&ui, "alpha beta");
    let second = row_of(&ui, "second");
    assert!(second > first + 1, "the long prompt must soft-wrap");
    // From "beta" to the end of "second": the soft wrap rejoins with a space,
    // the real line end stays, and the Markdown markers are not copied.
    let copied = drag_copy(&mut ui, (text.x + 6, first), (text.x + 5, second));
    assert_eq!(
        copied.as_deref(),
        Some("beta gamma delta epsilon zeta eta theta\nsecond")
    );
    ui.compute_view(60, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 60, 30));
    ui.render(&mut buffer);
    let tint = ratatui::style::Color::Rgb(44, 88, 56);
    assert_ne!(buffer[(text.x, first)].bg, tint, "before the press point");
    assert_eq!(buffer[(text.x + 6, first)].bg, tint);
    assert_eq!(buffer[(text.x + 5, second)].bg, tint);
    assert_ne!(
        buffer[(text.x + 7, second)].bg,
        tint,
        "after the release point"
    );
    // Typing ends the history selection.
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    assert!(ui.history_selection.is_none());
}

#[test]
fn dragging_the_draft_copies_and_typing_replaces_the_selection() {
    let (mut ui, room, _) = fixture();
    ui.locals
        .get_mut(&room)
        .unwrap()
        .text
        .insert("hello brave world");
    ui.notes_focus = true;
    ui.compute_view(100, 30);
    let rect = composer_rect(&ui);
    let copied = drag_copy(&mut ui, (rect.x + 6, rect.y), (rect.x + 10, rect.y));
    assert_eq!(copied.as_deref(), Some("brave"));
    assert!(!ui.notes_focus, "pressing the draft focuses it");
    ui.input(
        &RawInputEvent::Paste("bold".into()),
        false,
        &mut Default::default(),
    );
    assert_eq!(ui.locals[&room].text.text, "hello bold world");
    assert!(ui.pending.iter().any(|p| matches!(&p.command,
        BusCommand::SetDraftText(id, text) if *id == room && text == "hello bold world")));
}

#[test]
fn dragging_notes_copies_and_a_plain_click_only_moves_the_caret() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::*};
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_room_notes(room, "goal: ship it")
        .unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    let notes = ui.view.notes;
    assert_eq!(pointer(&mut ui, Down(Left), notes.x + 6, notes.y), None);
    assert_eq!(pointer(&mut ui, Drag(Left), notes.x + 8, notes.y), None);
    // Routine coordinator snapshots must not reset notes mid-selection.
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let copied = pointer(&mut ui, Up(Left), notes.x + 8, notes.y);
    assert_eq!(copied.as_deref(), Some("shi"));
    assert!(ui.notes_focus);
    assert_eq!(pointer(&mut ui, Down(Left), notes.x + 2, notes.y), None);
    assert_eq!(pointer(&mut ui, Up(Left), notes.x + 2, notes.y), None);
    let local = &ui.locals[&room];
    assert_eq!((local.notes.cursor, local.notes.selection()), (2, None));
    assert_eq!(local.notes.text, "goal: ship it");
}

#[test]
fn quote_preserves_newer_local_text_recipients_and_escaped_multiline_content() {
    let (mut ui, room, agent) = fixture();
    // Build a durable reply fixture through serialization, without exercising provider IO.
    let mut json = serde_json::to_value(&ui.snapshot.state).unwrap();
    json["rooms"][room.0.to_string()]["latest_replies"][agent.0.to_string()] = serde_json::json!({"request_id":100,"agent_id":agent.0,"text":"a\n\"quoted\" \\ @author","received_at_ms":5000});
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.state = serde_json::from_value(json).unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.locals.get_mut(&room).unwrap().text.insert("new local");
    ui.locals.get_mut(&room).unwrap().recipients.insert(agent);
    ui.notes_focus = true;
    ui.quote(RequestId(100));
    assert_eq!(
        ui.locals[&room].text.text,
        "new local\nauthor: \"a\n\\\"quoted\\\" \\\\ @author\"\n"
    );
    assert!(ui.locals[&room].recipients.contains(&agent));
    assert!(!ui.notes_focus);
    assert!(ui.send_intent.is_none());
}

#[test]
fn pending_text_commands_coalesce_without_overwriting_the_inflight_generation() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().text.insert("a");
    ui.text_changed(room);
    ui.pending.front_mut().unwrap().enqueued = true;
    for _ in 0..100 {
        ui.locals.get_mut(&room).unwrap().text.insert("x");
        ui.text_changed(room);
    }
    assert_eq!(ui.pending.len(), 2);
    assert!(matches!(&ui.pending[0].command,BusCommand::SetDraftText(_,text) if text=="a"));
    assert!(matches!(&ui.pending[1].command,BusCommand::SetDraftText(_,text) if text.len()==101));
}

#[test]
fn failed_attachment_cannot_be_erased_by_different_successful_attachment() {
    let (mut ui, room, _) = fixture();
    ui.queue(
        BusCommand::AttachFile(room, "/missing".into()),
        Effect::Files(room),
    );
    ui.queue(
        BusCommand::AttachFile(room, "/existing".into()),
        Effect::Files(room),
    );
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 1,
        result: Err("missing".into()),
    });
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 2,
        result: Ok(()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = 2;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    assert_eq!(ui.failed.len(), 1);
}

#[test]
fn add_agent_provider_selection_exposes_all_choices_as_mouse_targets() {
    let (mut ui, _, _) = fixture();
    ui.action(render::Action::NewAgent);
    ui.action(render::Action::Field(1));
    ui.compute_view(100, 30);
    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
    ui.render(&mut buffer);
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(
        text.contains("Claude Code") && text.contains("Cursor"),
        "{text}"
    );
}

#[test]
fn failed_file_is_removable_and_does_not_permanently_block_later_send() {
    let (mut ui, room, _) = fixture();
    ui.queue(
        BusCommand::AttachFile(room, "/missing.md".into()),
        Effect::Files(room),
    );
    ui.receive_event(BusEvent::CommandFinished {
        command_id: 1,
        result: Err("missing".into()),
    });
    let mut snapshot = (*ui.snapshot).clone();
    snapshot.last_command_id = 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.settle();
    ui.compute_view(100, 30);
    assert!(ui.view.hits.iter().any(|hit|matches!(&hit.action,render::Action::RemoveFile(path) if path.to_str()==Some("/missing.md"))));
    ui.action(render::Action::RemoveFile("/missing.md".into()));
    assert!(ui.failed.is_empty());
}

#[test]
fn notes_room_navigation_and_all_recipients_remain_room_local() {
    let (mut ui, room, agent) = fixture();
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    ui.input(
        &RawInputEvent::Paste("notes @literal".into()),
        false,
        &mut Default::default(),
    );
    ui.action(render::Action::Recipients);
    ui.action(render::Action::Recipient(None));
    assert!(ui.locals[&room].recipients.contains(&agent));
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(other);
    assert!(ui.locals[&other].notes.text.is_empty());
    assert!(ui.locals[&other].recipients.is_empty());
    ui.open_room(room);
    assert_eq!(ui.locals[&room].notes.text, "notes @literal");
}

#[test]
fn bus_presentation_1_and_15_agents_stays_bounded_to_visible_rows() {
    for count in [1, 15] {
        let (mut ui, room, _) = fixture();
        let mut snapshot = (*ui.snapshot).clone();
        for index in 1..count {
            snapshot
                .state
                .create_agent(
                    room,
                    &format!("agent{index}"),
                    Provider::ClaudeCode,
                    "/does/not/exist".into(),
                    None,
                )
                .unwrap();
        }
        let recipients: Vec<_> = snapshot.state.agents().map(|agent| agent.id).collect();
        snapshot
            .state
            .set_draft_recipients(room, recipients)
            .unwrap();
        snapshot
            .state
            .set_draft_text(
                room,
                "Exercise visible recipient names in the history header",
            )
            .unwrap();
        snapshot.state.submit_draft(room, 1).unwrap();
        ui.receive_snapshot(Arc::new(snapshot));
        let started = std::time::Instant::now();
        for _ in 0..200 {
            ui.compute_view(100, 30);
            let mut buffer =
                ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
            ui.render(&mut buffer);
            std::hint::black_box(buffer);
        }
        println!(
            "Bus fixed 100x30, agents={count}, 200 compute+render frames: {:?}",
            started.elapsed()
        );
        // The whole-column viewport now uses the former empty footer rows,
        // but hit targets must still be bounded by visible content, not agents.
        // Name, status and delete can share a row; targets stay viewport-bounded.
        assert!(ui.view.hits.len() <= 3 * 30);
        assert!(ui
            .view
            .hits
            .iter()
            .all(|hit| hit.rect.bottom() <= 30 && hit.rect.right() <= 100));
        assert_eq!(ui.snapshot.state.agents().count(), count);
    }
}

#[test]
fn overflow_mixed_file_chips_keep_full_path_inspection_and_removal() {
    use crossterm::event::MouseEventKind::{Moved, ScrollDown, ScrollUp};
    let (mut ui, room, _) = fixture();
    let paths: Vec<std::path::PathBuf> = [
        format!("/one/{}.md", "very-long-name-".repeat(10)),
        "/two/ordinary-second-attachment.md".into(),
        "/three/ordinary-third-attachment.md".into(),
        "/missing/failed-overflow-attachment.md".into(),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    let mut snapshot = (*ui.snapshot).clone();
    for path in &paths[..3] {
        snapshot.state.attach_file(room, path.clone()).unwrap();
    }
    ui.receive_snapshot(Arc::new(snapshot));
    ui.failed.push(Pending {
        id: 50,
        command: BusCommand::AttachFile(room, paths[3].display().to_string()),
        effect: Effect::Files(room),
        enqueued: true,
        result: Some(Err("missing".into())),
    });
    ui.compute_view(100, 30);
    let next = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::ScrollFiles(true))
        .unwrap()
        .clone();
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        next.rect.x,
        next.rect.y,
    );
    ui.compute_view(100, 30);
    assert!(ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::RemoveFile(paths[1].clone())));
    assert!(!ui
        .view
        .hits
        .iter()
        .any(|h| h.action == render::Action::RemoveFile(paths[0].clone())));
    let previous = ui
        .view
        .hits
        .iter()
        .find(|h| h.action == render::Action::ScrollFiles(false))
        .unwrap()
        .clone();
    mouse(
        &mut ui,
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        previous.rect.x,
        previous.rect.y,
    );
    for path in &paths {
        for _ in 0..paths.len() {
            ui.compute_view(100, 30);
            if ui
                .view
                .hits
                .iter()
                .any(|h| h.action == render::Action::RemoveFile(path.clone()))
            {
                break;
            }
            let files = ui.view.files;
            mouse(&mut ui, ScrollDown, files.x + 1, files.y);
        }
        let hit = ui
            .view
            .hits
            .iter()
            .find(|h| h.action == render::Action::RemoveFile(path.clone()))
            .expect("every successful and failed attachment must have a reachable removal target")
            .clone();
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let chip: String = (hit.rect.x..hit.rect.right())
            .map(|x| buffer[(x, hit.rect.y)].symbol())
            .collect();
        assert!(
            chip.contains(if path == &paths[3] { "! ×]" } else { "×]" }),
            "clipped chip retains error/removal suffix: {chip}"
        );
        mouse(&mut ui, Moved, hit.rect.x, hit.rect.y);
        assert_eq!(ui.detail_path.as_deref(), path.to_str());
        mouse(
            &mut ui,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            hit.rect.x,
            hit.rect.y,
        );
        if path == &paths[3] {
            assert!(ui.failed.is_empty());
        } else {
            assert!(ui.pending.iter().any(|p| matches!(&p.command, BusCommand::RemoveFile(id, removed) if *id == room && removed == path)));
        }
        for _ in 0..paths.len() {
            let files = ui.view.files;
            mouse(&mut ui, ScrollUp, files.x + 1, files.y);
        }
    }
    assert_eq!(ui.main_scroll, 0, "file scrolling must not move replies");
}

#[test]
fn pasted_images_are_saved_once_under_the_bus_data_dir() {
    let root = std::env::temp_dir().join(format!(
        "bus-paste-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let room = RoomId(7);
    let first = input::save_pasted_image(&root, room, b"\x89PNG fake", "png").unwrap();
    let again = input::save_pasted_image(&root, room, b"\x89PNG fake", "png").unwrap();
    let other = input::save_pasted_image(&root, room, b"other", "png").unwrap();

    assert!(first.is_absolute());
    assert!(first.starts_with(root.join("attachments").join("room-7")));
    assert_eq!(first.extension().unwrap(), "png");
    assert_eq!(std::fs::read(&first).unwrap(), b"\x89PNG fake");
    assert_eq!(first, again, "one image pasted twice is one file");
    assert_ne!(first, other);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn pasted_temporary_images_are_copied_into_the_room_attachments() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let temp = std::env::temp_dir().join(format!("bus-temp-paste-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&temp).unwrap();
    let pasted = temp.join("pasted-image.PNG");
    std::fs::write(&pasted, b"\x89PNG pasted").unwrap();
    let notes = temp.join("notes.md");
    std::fs::write(&notes, "# notes").unwrap();
    let root = temp.join("data");
    let room = RoomId(3);

    let copy = input::copy_temporary_image(&root, room, &pasted)
        .unwrap()
        .expect("a temp image is copied");
    assert!(
        copy.starts_with(root.canonicalize().unwrap().join("attachments/room-3"))
            || copy.starts_with(root.join("attachments/room-3"))
    );
    assert_eq!(copy.extension().unwrap(), "png");
    assert_eq!(std::fs::read(&copy).unwrap(), b"\x89PNG pasted");
    assert_eq!(
        input::copy_temporary_image(&root, room, &notes).unwrap(),
        None
    );
    assert_eq!(
        input::copy_temporary_image(&root, room, &copy).unwrap(),
        None,
        "an attachment Bus already owns stays where it is"
    );
    std::fs::remove_dir_all(&temp).unwrap();
}

#[test]
fn room_notes_grow_to_a_quarter_of_the_window_and_history_takes_the_rest() {
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new(checklist(30));
    let rows = screen_rows(&mut ui, 100, 30);
    // 30 rows / 4 = 7 note rows inside the box's borders.
    assert_eq!(ui.view.notes.height, 7);
    assert_eq!(ui.view.notes_box, ratatui::layout::Rect::new(29, 2, 70, 9));
    assert_eq!(ui.view.history_text.y, 12, "history starts under the box");
    assert!(rows[1].contains("# "), "the room name keeps its row");
    assert!(rows[3].contains("item 01") && rows[9].contains("item 07"));
    assert!(rows[10].contains("↓ more"), "{:?}", rows[10]);
    assert!(!rows[2].contains("↑ more"));

    for (cols, height, notes) in [(100, 40, 10), (100, 24, 6), (60, 24, 6)] {
        screen_rows(&mut ui, cols, height);
        assert_eq!(ui.view.notes.height, notes, "{cols}x{height}");
        assert!(ui.view.notes_box.bottom() < ui.view.history_text.y);
        assert!(
            ui.view.history_text.height >= 1,
            "{cols}x{height}: history keeps a row"
        );
    }

    // Short notes shrink the box back; empty notes keep the F3 prompt row.
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new("two\nlines".into());
    screen_rows(&mut ui, 100, 30);
    assert_eq!(ui.view.notes.height, 2);
    assert_eq!(ui.view.history_text.y, 7);
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new(String::new());
    let rows = screen_rows(&mut ui, 100, 30);
    assert_eq!(ui.view.notes.height, 1);
    assert!(rows[3].contains("Add notes… (F3)"));
}

#[test]
fn room_notes_scroll_with_the_wheel_and_page_keys_and_keep_the_caret_visible() {
    use crossterm::event::MouseEventKind::{ScrollDown, ScrollUp};
    let (mut ui, room, _) = fixture();
    ui.locals.get_mut(&room).unwrap().notes = editor::Editor::new(checklist(30));
    screen_rows(&mut ui, 100, 30);
    let notes = ui.view.notes;

    // The wheel over the box scrolls only the notes.
    mouse(&mut ui, ScrollDown, notes.x + 2, notes.y + 1);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[3].contains("item 02"), "{:?}", rows[3]);
    assert!(rows[2].contains("↑ more") && rows[10].contains("↓ more"));
    assert_eq!(ui.main_scroll, ui.view.history_max_scroll);
    for _ in 0..40 {
        mouse(&mut ui, ScrollDown, notes.x + 2, notes.y + 1);
    }
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[9].contains("item 30") && !rows[10].contains("↓ more"));
    mouse(&mut ui, ScrollUp, notes.x + 2, notes.y + 1);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[9].contains("item 29"));

    // While editing, the caret starts in view, PgDn/PgUp page, and typing or
    // arrows bring the caret back into view.
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(
        rows[9].contains("item 30"),
        "F3 brings the end caret into view"
    );
    ui.locals.get_mut(&room).unwrap().notes.cursor = 0;
    screen_rows(&mut ui, 100, 30);
    let visible = |ui: &BusUi| {
        ui.cursor()
            .is_some_and(|cursor| ui.view.notes.contains((cursor.x, cursor.y).into()))
    };
    assert!(visible(&ui));
    key(&mut ui, KeyCode::PageDown, KeyModifiers::NONE);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(
        rows[3].contains("item 07"),
        "a page keeps one row of context"
    );
    assert!(!visible(&ui), "paging scrolls without moving the caret");
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    let rows = screen_rows(&mut ui, 100, 30);
    assert!(rows[3].contains("item 01"));
    for _ in 0..20 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
        screen_rows(&mut ui, 100, 30);
        assert!(visible(&ui), "arrow keys keep the caret visible");
    }
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    key(&mut ui, KeyCode::PageUp, KeyModifiers::NONE);
    screen_rows(&mut ui, 100, 30);
    assert!(!visible(&ui));
    key(&mut ui, KeyCode::Char('x'), KeyModifiers::NONE);
    screen_rows(&mut ui, 100, 30);
    assert!(visible(&ui), "typing scrolls back to the caret");
    assert!(ui.locals[&room].notes.text.contains("x- [ ] item 21"));
}

#[test]
fn switching_bus_screens_requests_one_full_repaint() {
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    ui.compute_view(100, 30);
    ui.full_repaint = false;
    ui.compute_view(100, 30);
    assert!(!ui.full_repaint, "redrawing the same screen diffs as usual");
    ui.action(render::Action::Settings);
    ui.compute_view(100, 30);
    assert!(std::mem::take(&mut ui.full_repaint), "opening a form");
    ui.compute_view(100, 30);
    assert!(!ui.full_repaint, "once per switch");
    ui.action(render::Action::Cancel);
    ui.compute_view(100, 30);
    assert!(std::mem::take(&mut ui.full_repaint), "closing a form");
    ui.open_room(other);
    ui.compute_view(100, 30);
    assert!(std::mem::take(&mut ui.full_repaint), "switching rooms");
    ui.open_room(room);
    ui.terminal = Some(agent);
    ui.compute_view(100, 30);
    assert!(
        std::mem::take(&mut ui.full_repaint),
        "opening an agent terminal"
    );
}

/// Replays real room frames through a terminal emulator, as a host terminal
/// would show them.
#[test]
fn a_screen_switch_repaints_stale_cells_in_the_right_margin() {
    use crate::protocol::render_ansi::BlitEncoder;
    let (cols, rows) = (100u16, 30u16);
    let frame = |ui: &mut BusUi| {
        ui.compute_view(cols, rows);
        let mut buffer =
            ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, cols, rows));
        ui.render(&mut buffer);
        crate::protocol::FrameData::from_ratatui_buffer_with_hyperlinks(&buffer, None, &[])
    };
    let margin = |terminal: &crate::ghostty::Terminal| -> String {
        (0..u32::from(rows))
            .map(|row| {
                let (_, graphemes) = terminal.screen_cell(cols - 1, row).unwrap();
                graphemes
                    .first()
                    .and_then(|code| char::from_u32(*code))
                    .unwrap_or(' ')
            })
            .collect()
    };
    let (mut ui, room, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let other = snapshot.state.create_room("other").unwrap();
    ui.receive_snapshot(Arc::new(snapshot));
    let mut terminal = crate::ghostty::Terminal::new(cols, rows, 0).unwrap();
    let mut encoder = BlitEncoder::new();
    let mut present = |terminal: &mut crate::ghostty::Terminal, frame, repaint| {
        let encoded = encoder.encode(&frame, repaint);
        terminal.write(&encoded.bytes);
        encoder.commit(frame, encoded);
    };

    let first = frame(&mut ui);
    present(&mut terminal, first, false);
    assert!(
        margin(&terminal).trim().is_empty(),
        "Bus never draws in the margin"
    );

    // The terminal shows something in the margin that no Bus frame drew
    // there (e.g. text a terminal draws differently). The diff encoder never
    // revisits cells whose frame content is unchanged.
    terminal.write(b"\x1b[5;100Hr\x1b[20;100Hd");
    ui.locals.get_mut(&room).unwrap().notes.insert("changed");
    let changed = frame(&mut ui);
    present(&mut terminal, changed, ui.full_repaint);
    assert_eq!(
        margin(&terminal).replace(' ', ""),
        "rd",
        "a diff frame keeps them"
    );

    // Any screen switch repaints every cell, margin included.
    ui.open_room(other);
    let switched = frame(&mut ui);
    assert!(ui.full_repaint);
    present(
        &mut terminal,
        switched,
        std::mem::take(&mut ui.full_repaint),
    );
    assert!(
        margin(&terminal).trim().is_empty(),
        "{:?}",
        margin(&terminal)
    );
}

#[test]
fn master_room_shows_no_notes_and_f3_never_focuses_them() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let mut ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    ui.open_room(master);
    assert!(!room_screen(&mut ui, 100, 30).contains("Add notes"));
    assert_eq!(ui.view.notes_box.height, 0);
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(!ui.notes_focus, "MASTER has no notes to edit");

    ui.open_room(work);
    assert!(room_screen(&mut ui, 100, 30).contains("Add notes"));
    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    assert!(ui.notes_focus, "work rooms keep their notes");
}

/// Typed text in every input sits on the box's own background: the grey
/// highlight is for selections and selected rows only, and the cursor still
/// shows where typing goes.
#[test]
fn typed_text_in_inputs_has_no_highlight_background() {
    let highlight = ratatui::style::Color::Rgb(46, 48, 58);
    let typed_cells = |ui: &mut BusUi, text: &str| {
        ui.compute_view(100, 30);
        let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, 100, 30));
        ui.render(&mut buffer);
        let cursor = ui
            .view
            .cursor
            .as_ref()
            .expect("the focused input shows a cursor");
        let row: String = (0..100).map(|x| buffer[(x, cursor.y)].symbol()).collect();
        let start = row
            .find(text)
            .unwrap_or_else(|| panic!("{text} not on {row:?}"));
        let start = row[..start].chars().count() as u16;
        (0..text.len() as u16)
            .map(|offset| buffer[(start + offset, cursor.y)].bg)
            .collect::<Vec<_>>()
    };
    let type_text = |ui: &mut BusUi, text: &str| {
        for c in text.chars() {
            key(ui, KeyCode::Char(c), KeyModifiers::NONE);
        }
    };

    let (mut ui, room, _) = fixture();
    ui.open_room(room);
    type_text(&mut ui, "composerdraft");
    let cells = typed_cells(&mut ui, "composerdraft");
    assert!(
        cells.iter().all(|bg| *bg != highlight),
        "composer: {cells:?}"
    );

    key(&mut ui, KeyCode::F(3), KeyModifiers::NONE);
    type_text(&mut ui, "notesline");
    let cells = typed_cells(&mut ui, "notesline");
    assert!(cells.iter().all(|bg| *bg != highlight), "notes: {cells:?}");

    let (mut ui, room, _) = fixture();
    ui.open_room(room);
    ui.action(render::Action::NewAgent);
    type_text(&mut ui, "formname");
    let cells = typed_cells(&mut ui, "formname");
    assert!(cells.iter().all(|bg| *bg != highlight), "form: {cells:?}");
}
