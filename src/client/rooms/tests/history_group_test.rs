use super::*;
use crate::messaging::model::{Draft, Provider};

#[test]
fn an_individual_answer_stays_visible_when_later_messages_join_the_same_open_turn() {
    let mut state = BusState::default();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            room,
            "watcher",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    state.set_draft_text(room, "watch").unwrap();
    let lead = state.submit_draft(room, 1000).unwrap()[0];
    state.set_draft_text(room, "follow up").unwrap();
    let follow = state.submit_draft(room, 1001).unwrap()[0];
    let mut document = serde_json::to_value(state).unwrap();
    document["requests"][lead.0.to_string()]["phase"] = "active".into();
    document["requests"][follow.0.to_string()]["phase"] = "active".into();
    document["requests"][follow.0.to_string()]["group"] = lead.0.into();
    let mut state: BusState = serde_json::from_value(document).unwrap();
    let message = state.request(lead).unwrap().prompt.id;
    state
        .answer_message(message, agent, "INDIVIDUAL RESULT".into(), 2000)
        .unwrap();
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    let text = history
        .lines(
            &state,
            state.room(room).unwrap(),
            100,
            1,
            2001,
            &mut thumbnails,
        )
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text.matches("INDIVIDUAL RESULT").count(), 1, "{text}");
    assert!(text.contains("follow up"), "{text}");
}

#[test]
fn hidden_waiting_tag_does_not_color_visible_plain_text() {
    let mut state = BusState::default();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    state
        .set_draft_text(
            room,
            "[label](https://example.com/[waiting-on-you])\nwaiting-on-you",
        )
        .unwrap();
    state.submit_draft(room, 1).unwrap();
    let mut history = History::default();
    let lines = history.lines(
        &state,
        state.room(room).unwrap(),
        80,
        1,
        2,
        &mut Default::default(),
    );
    let plain = lines
        .iter()
        .find(|line| line.text == "waiting-on-you")
        .expect("visible plain text");
    assert!(
        plain
            .styles
            .iter()
            .all(|(_, style)| style.fg != Some(super::super::super::render::ACCENT)),
        "a hidden link target must not turn plain text into a tag: {:?}",
        plain.styles
    );
}

#[test]
fn developer_attention_master_notice_renders_badge_instead_of_raw_tag() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    state
        .post_to_developer(
            master,
            agent,
            "[needs_developer_attention] Choose a target".into(),
            vec![],
            1_000,
        )
        .unwrap();
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    let text = history
        .lines(
            &state,
            state.room(master).unwrap(),
            100,
            1,
            2_000,
            &mut thumbnails,
        )
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Needs developer"), "{text}");
    assert!(text.contains("Choose a target"), "{text}");
    assert!(!text.contains("[needs_developer_attention]"), "{text}");
}

#[test]
fn a_cross_room_group_shows_its_reply_once_in_each_room() {
    let mut state = BusState::default();
    state.ensure_master_room();
    let master = state.master_room().unwrap().id;
    let work = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    let mut requests = Vec::new();
    for (room, text) in [
        (work, "dialog"),
        (master, "developer question"),
        (work, "follow-up"),
    ] {
        requests.push(
            state
                .submit_message_from(
                    room,
                    Draft {
                        text: text.into(),
                        files: Vec::new(),
                        recipient_ids: [agent].into(),
                    },
                    Author::Developer,
                    1_000 + requests.len() as u64,
                )
                .unwrap()[0],
        );
    }
    let mut saved = serde_json::to_value(&state).unwrap();
    for id in &requests {
        let request = &mut saved["requests"][id.0.to_string()];
        request["phase"] = "completed".into();
        request["completed_reply"] = serde_json::json!({"group":request["group"].as_u64().unwrap_or(request["id"].as_u64().unwrap()),"content":serde_json::json!({
            "callback_id": "final", "text": "shared reply", "received_at_ms": 2_000,
            "provider_session_id": "session", "provider_turn_id": "turn"
        })});
        if *id != requests[0] {
            request["group"] = requests[0].0.into();
            request["completed_reply"]["group"] = requests[0].0.into();
        }
    }
    let state: BusState = serde_json::from_value(saved).unwrap();
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    for room in [work, master] {
        let lines = history.lines(
            &state,
            state.room(room).unwrap(),
            100,
            1,
            3_000,
            &mut thumbnails,
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.text.contains("shared reply"))
                .count(),
            1,
            "{room:?}"
        );
        assert!(lines.iter().all(|line| !line.text.contains('…')));
    }
}

#[test]
fn cached_work_history_updates_a_renamed_master_recipient() {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(
            master,
            "old-orchestrator",
            Provider::Codex,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, work).unwrap();
    state
        .submit_message_from(
            work,
            Draft {
                text: "question for the orchestrator".into(),
                files: Vec::new(),
                recipient_ids: [orchestrator].into(),
            },
            Author::Developer,
            1_000,
        )
        .unwrap();
    let mut history = History::default();
    let before = history.lines(
        &state,
        state.room(work).unwrap(),
        100,
        1,
        2_000,
        &mut Thumbnails::default(),
    );
    assert!(before
        .iter()
        .any(|line| line.text.contains("old-orchestrator")));

    state
        .rename_agent(orchestrator, "new-orchestrator")
        .unwrap();
    let after = history.lines(
        &state,
        state.room(work).unwrap(),
        100,
        2,
        2_000,
        &mut Thumbnails::default(),
    );
    assert!(
        after
            .iter()
            .any(|line| line.text.starts_with("You → new-orchestrator")),
        "a new revision must show the recipient's current name: {:?}",
        after
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn consecutive_replies_to_one_message_are_separated_by_one_blank_row() {
    let mut state = BusState::default();
    let room = state.create_room("work").unwrap();
    let agents: Vec<_> = ["alpha", "beta", "gamma"]
        .into_iter()
        .map(|name| {
            state
                .create_agent(room, name, Provider::ClaudeCode, "/project".into(), None)
                .unwrap()
        })
        .collect();
    let requests = state
        .submit_message_from(
            room,
            Draft {
                text: "first".into(),
                files: Vec::new(),
                recipient_ids: agents.iter().copied().collect(),
            },
            Author::Developer,
            1_000,
        )
        .unwrap();
    // alpha has replied; beta and gamma are still pending.
    let mut saved = serde_json::to_value(&state).unwrap();
    let request = &mut saved["requests"][requests[0].0.to_string()];
    request["phase"] = "completed".into();
    request["completed_reply"] = serde_json::json!({"group":request["group"].as_u64().unwrap_or(request["id"].as_u64().unwrap()),"content":serde_json::json!({
        "callback_id": "final", "text": "fake reply: first", "received_at_ms": 2_000,
        "provider_session_id": "session", "provider_turn_id": "turn"
    })});
    let state: BusState = serde_json::from_value(saved).unwrap();
    let mut history = History::default();
    let lines = history.lines(
        &state,
        state.room(room).unwrap(),
        100,
        1,
        3_000,
        &mut Thumbnails::default(),
    );
    let rows: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
    assert_eq!(
        rows[1..],
        [
            "first",
            "",
            "    alpha  Claude Code",
            "    fake reply: first",
            "    ",
            "    Quote",
            "",
            "    beta  Claude Code",
            "    …",
            "",
            "    gamma  Claude Code",
            "    …",
            "",
        ],
        "{rows:?}"
    );
}

#[test]
fn an_archived_group_still_shows_its_shared_reply_once() {
    let mut state = BusState::default();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            room,
            "worker",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    let mut requests = Vec::new();
    for text in ["first task", "second task", "third task"] {
        requests.push(
            state
                .submit_message_from(
                    room,
                    Draft {
                        text: text.into(),
                        files: Vec::new(),
                        recipient_ids: [agent].into(),
                    },
                    Author::Developer,
                    1_000,
                )
                .unwrap()[0],
        );
    }
    assert!(state.coalesce_queue(requests[0]).is_some());
    for id in requests {
        state.finish_for_test(id, "shared completed answer", 2_000);
    }
    let render = |state: &BusState| {
        History::default()
            .lines(
                state,
                state.room(room).unwrap(),
                100,
                1,
                3_000,
                &mut Thumbnails::default(),
            )
            .iter()
            .filter(|line| line.text.contains("shared completed answer"))
            .count()
    };
    assert_eq!(render(&state), 1, "the live group shares one reply");
    assert!(state.archive(2_000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    assert_eq!(state.history().len(), 3);
    assert_eq!(
        render(&state),
        1,
        "archiving must preserve the group's single shared reply"
    );
}

#[test]
fn a_rebuild_names_the_layout_input_that_changed() {
    let key = (1, RoomId(1), 80, 10, None);
    assert_eq!(rebuild_reason(None, key), "first");
    assert_eq!(
        rebuild_reason(Some((1, RoomId(2), 80, 10, None)), key),
        "room"
    );
    assert_eq!(
        rebuild_reason(Some((1, RoomId(1), 100, 10, None)), key),
        "width"
    );
    assert_eq!(
        rebuild_reason(Some((2, RoomId(1), 80, 9, None)), key),
        "content"
    );
    assert_eq!(
        rebuild_reason(Some((1, RoomId(1), 80, 10, Some((8, 16)))), key),
        "thumbnails"
    );
    assert_eq!(
        rebuild_reason(Some((1, RoomId(1), 80, 9, None)), key),
        "minute"
    );
}

#[test]
fn returning_to_a_room_reuses_layout_until_width_changes() {
    let mut state = BusState::default();
    let first = state.create_room("first").unwrap();
    let second = state.create_room("second").unwrap();
    for room in [first, second] {
        let agent = state
            .create_agent(
                room,
                &format!("worker-{}", room.0),
                Provider::Codex,
                "/project".into(),
                None,
            )
            .unwrap();
        let request = state
            .submit_message_from(
                room,
                Draft {
                    text: "**prompt**".into(),
                    files: Vec::new(),
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1_000,
            )
            .unwrap()[0];
        state.finish_for_test(request, "**reply**", 2_000);
    }
    let mut history = History::default();
    for (room, width, expected) in [
        (first, 80, 2),
        (second, 80, 2),
        (first, 80, 0),
        (first, 40, 2),
    ] {
        history.lines(
            &state,
            state.room(room).unwrap(),
            width,
            1,
            3_000,
            &mut Thumbnails::default(),
        );
        assert_eq!(history.rendered, expected, "room={room:?} width={width}");
        assert!(history
            .cached()
            .iter()
            .any(|line| line.text.trim() == "reply"));
    }
    state.delete_room(second, 0).unwrap();
    history.lines(
        &state,
        state.room(first).unwrap(),
        40,
        2,
        63_000,
        &mut Thumbnails::default(),
    );
    assert_eq!(history.room_markdown.len(), 1);
    assert_eq!(
        history.markdown.len(),
        2,
        "deleted room's blocks are released"
    );
    assert_eq!(history.rendered, 0);
}

#[test]
fn rebuild_counts_only_markdown_laid_out_afresh_across_room_switches() {
    let mut state = BusState::default();
    let mut rooms = Vec::new();
    for name in ["first", "second"] {
        let room = state.create_room(name).unwrap();
        let agent = state
            .create_agent(
                room,
                &format!("agent-{name}"),
                Provider::Codex,
                "/project".into(),
                None,
            )
            .unwrap();
        state.set_draft_recipients(room, [agent]).unwrap();
        for i in 0..3 {
            state
                .set_draft_text(room, &format!("**{name}** prompt {i}"))
                .unwrap();
            state.submit_draft(room, 1_000 + i).unwrap();
        }
        rooms.push(room);
    }
    let mut history = History::default();
    let mut thumbnails = Thumbnails::default();
    let mut rendered = |history: &mut History, room, width| {
        history.lines(
            &state,
            state.room(room).unwrap(),
            width,
            1,
            3_000,
            &mut thumbnails,
        );
        history.rendered
    };

    assert_eq!(rendered(&mut history, rooms[0], 80), 3, "first visit");
    assert_eq!(rendered(&mut history, rooms[1], 80), 3, "other room");
    assert_eq!(rendered(&mut history, rooms[0], 80), 0, "switch back");
    assert_eq!(rendered(&mut history, rooms[0], 60), 3, "new width");
}

#[test]
fn audit_history_renders_deleted_sender_recipient_and_reply_by_name() {
    let mut state = BusState::default();
    let room = state.create_room("audit").unwrap();
    let author = state
        .create_agent(room, "sender", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let recipient = state
        .create_agent(room, "reviewer", Provider::Codex, "/repo".into(), None)
        .unwrap();
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "original task".into(),
                files: vec![],
                recipient_ids: [recipient].into(),
            },
            crate::messaging::model::Author::Agent(author),
            1,
        )
        .unwrap()[0];
    state.finish_for_test(request, "original verdict", 2);
    state.delete_agent(author, 3).unwrap();
    state.delete_agent(recipient, 4).unwrap();
    let mut history = History::default();
    let text = history
        .lines(
            &state,
            state.room(room).unwrap(),
            100,
            1,
            5,
            &mut Default::default(),
        )
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "sender [deleted]",
        "reviewer [deleted]",
        "original task",
        "original verdict",
    ] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
}
