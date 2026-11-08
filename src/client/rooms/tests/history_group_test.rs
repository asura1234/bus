use super::*;
use crate::messaging::model::{Draft, Provider};

#[test]
fn delivered_history_shows_the_message_without_a_reply_lifecycle() {
    let mut state = BusState::default();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    state
        .set_draft_text(room, "PLAIN_MESSAGE for the worker")
        .unwrap();
    let request = state.submit_draft(room, 1000).unwrap()[0];
    let mut document = serde_json::to_value(state).unwrap();
    document["requests"][request.0.to_string()]["phase"] = "completed".into();
    document["requests"][request.0.to_string()]["completed_reply"] = serde_json::json!({
        "group": request.0,
        "content": {
            "callback_id": "final-1",
            "text": "Reply to message 1\nREPLY_BODY",
            "received_at_ms": 2000
        }
    });
    let state: BusState = serde_json::from_value(document).unwrap();
    let mut history = History::default();
    let text = history
        .lines(
            &state,
            state.room(room).unwrap(),
            100,
            1,
            2001,
            &mut Thumbnails::default(),
        )
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("PLAIN_MESSAGE"), "{text}");
    assert!(!text.contains("REPLY_BODY"), "{text}");
    assert!(!text.contains("Reply to message"), "{text}");
    assert!(!text.contains("answered with message"), "{text}");
    assert!(!text.contains('…'), "{text}");
}

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
    let watch = text.find("watch").expect("lead prompt");
    let follow = text.find("follow up").expect("later prompt");
    assert!(watch < follow, "{text}");
    assert!(!text.contains("INDIVIDUAL RESULT"), "{text}");
    assert!(!text.contains("Reply to message"), "{text}");
    assert!(!text.contains("answered with message"), "{text}");
    assert!(!text.contains('…'), "{text}");
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
        let text = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("shared reply"), "{room:?} {text}");
        assert!(!text.contains("Reply to message"), "{room:?} {text}");
        assert!(!text.contains("answered with message"), "{room:?} {text}");
        assert!(lines.iter().all(|line| !line.text.contains('…')));
        if room == work {
            let dialog = text.find("dialog").expect("work prompt");
            let follow = text.find("follow-up").expect("work follow-up");
            assert!(dialog < follow, "{text}");
            assert!(!text.contains("developer question"), "{text}");
        } else {
            assert!(text.contains("developer question"), "{text}");
            assert!(!text.contains("dialog"), "{text}");
            assert!(!text.contains("follow-up"), "{text}");
        }
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
    let text = rows.join("\n");
    let header = rows
        .iter()
        .position(|row| {
            row.starts_with("You →")
                && row.contains("alpha")
                && row.contains("beta")
                && row.contains("gamma")
        })
        .expect("recipient header");
    let body = rows.iter().position(|row| *row == "first").expect("prompt");
    assert!(header < body, "{rows:?}");
    assert!(!text.contains("fake reply"), "{text}");
    assert!(!text.contains("Reply to message"), "{text}");
    assert!(!text.contains("answered with message"), "{text}");
    assert!(!text.contains("Quote"), "{text}");
    assert!(!text.contains('…'), "{text}");
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
        let text = History::default()
            .lines(
                state,
                state.room(room).unwrap(),
                100,
                1,
                3_000,
                &mut Thumbnails::default(),
            )
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let first = text.find("first task").expect("first task");
        let second = text.find("second task").expect("second task");
        let third = text.find("third task").expect("third task");
        assert!(first < second && second < third, "{text}");
        assert!(!text.contains("shared completed answer"), "{text}");
        assert!(!text.contains("Reply to message"), "{text}");
        assert!(!text.contains("answered with message"), "{text}");
        assert!(!text.contains('…'), "{text}");
    };
    render(&state);
    assert!(state.archive(2_000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    assert_eq!(state.history().len(), 3);
    render(&state);
}

#[test]
fn a_rebuild_names_the_layout_input_that_changed() {
    let key = (1, RoomId(1), 80, 10, None, Default::default(), 0);
    assert_eq!(rebuild_reason(None, key), "first");
    assert_eq!(
        rebuild_reason(
            Some((1, RoomId(2), 80, 10, None, Default::default(), 0)),
            key
        ),
        "room"
    );
    assert_eq!(
        rebuild_reason(
            Some((1, RoomId(1), 100, 10, None, Default::default(), 0)),
            key
        ),
        "width"
    );
    assert_eq!(
        rebuild_reason(
            Some((2, RoomId(1), 80, 9, None, Default::default(), 0)),
            key
        ),
        "content"
    );
    assert_eq!(
        rebuild_reason(
            Some((1, RoomId(1), 80, 10, Some((8, 16)), Default::default(), 0)),
            key
        ),
        "thumbnails"
    );
    assert_eq!(
        rebuild_reason(
            Some((1, RoomId(1), 80, 9, None, Default::default(), 0)),
            key
        ),
        "minute"
    );
    assert_eq!(
        rebuild_reason(
            Some((
                1,
                RoomId(1),
                80,
                10,
                None,
                crate::messaging::model::RoomHistoryFilters {
                    to_you: true,
                    from_you: false,
                },
                0,
            )),
            key
        ),
        "filters"
    );
    assert_eq!(
        rebuild_reason(
            Some((1, RoomId(1), 80, 10, None, Default::default(), 1)),
            key
        ),
        "submissions"
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
        (first, 80, 1),
        (second, 80, 1),
        (first, 80, 0),
        (first, 40, 1),
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
            .any(|line| line.text.trim() == "prompt"));
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
        1,
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
    for expected in ["sender [deleted]", "reviewer [deleted]", "original task"] {
        assert!(text.contains(expected), "missing {expected}: {text}");
    }
    assert!(!text.contains("original verdict"), "{text}");
}

fn hidden_routing_envelope_fixture(
    developer_question: bool,
) -> (BusState, RoomId, RequestId, RequestId) {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let room = state.create_room("work").unwrap();
    let orch = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            "/project".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    let worker = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    let lead = state
        .submit_message_from(
            room,
            Draft {
                text: "AUTHORED_QUESTION".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            if developer_question {
                Author::Developer
            } else {
                Author::Agent(worker)
            },
            1_000,
        )
        .unwrap()[0];
    let hidden = state
        .submit_bus_message_with(
            room,
            Draft {
                text: "HIDDEN_ROUTING_ENVELOPE".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(worker),
            1_001,
            false,
        )
        .unwrap()[0];
    (state, room, lead, hidden)
}

fn hidden_routing_history(state: &BusState, room: RoomId) -> String {
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
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn hidden_routing_envelope_keeps_the_authored_shared_reply_visible() {
    // Ordinary authored work can still share a reply with its routing envelope.
    let (mut state, room, lead, hidden) = hidden_routing_envelope_fixture(false);
    assert!(state.coalesce_queue(lead).is_some());
    assert_eq!(state.request(hidden).unwrap().group, Some(lead));
    for id in [lead, hidden] {
        state.finish_for_test(id, "SHARED_AUTHORED_ANSWER", 2_000);
    }
    let text = hidden_routing_history(&state, room);
    assert!(text.contains("AUTHORED_QUESTION"), "{text}");
    assert!(!text.contains("HIDDEN_ROUTING_ENVELOPE"), "{text}");
    assert!(!text.contains("SHARED_AUTHORED_ANSWER"), "{text}");
    assert!(!text.contains("answered with message"), "{text}");
    assert!(!text.contains('…'), "{text}");
    assert!(state.archive(2_000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    let text = hidden_routing_history(&state, room);
    assert!(text.contains("AUTHORED_QUESTION"), "{text}");
    assert!(!text.contains("HIDDEN_ROUTING_ENVELOPE"), "{text}");
    assert!(!text.contains("SHARED_AUTHORED_ANSWER"), "{text}");
}

#[test]
fn developer_question_stays_visible_when_an_agent_routing_envelope_cannot_coalesce() {
    let (mut state, room, lead, hidden) = hidden_routing_envelope_fixture(true);
    assert_eq!(
        state.request(lead).unwrap().prompt.author,
        Author::Developer
    );
    assert!(matches!(
        state.request(hidden).unwrap().prompt.author,
        Author::Agent(_)
    ));
    assert!(state.coalesce_queue(lead).is_none());
    assert_eq!(state.request(hidden).unwrap().group, None);
    assert_eq!(
        state.queued_requests(state.request(lead).unwrap().agent_id),
        &[lead, hidden]
    );
    state.finish_for_test(lead, "VISIBLE_DEVELOPER_ANSWER", 2_000);
    let text = hidden_routing_history(&state, room);
    assert!(text.contains("AUTHORED_QUESTION"), "{text}");
    assert!(!text.contains("HIDDEN_ROUTING_ENVELOPE"), "{text}");
    assert!(!text.contains("VISIBLE_DEVELOPER_ANSWER"), "{text}");
    assert!(!text.contains("Reply to message"), "{text}");
    assert!(state.archive(2_000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    let text = hidden_routing_history(&state, room);
    assert!(text.contains("AUTHORED_QUESTION"), "{text}");
    assert!(!text.contains("VISIBLE_DEVELOPER_ANSWER"), "{text}");
    assert!(!text.contains("HIDDEN_ROUTING_ENVELOPE"), "{text}");
    let hidden = state.request(hidden).unwrap();
    assert_eq!(hidden.phase, crate::messaging::model::RequestPhase::Queued);
    assert_eq!(hidden.group, None);
    assert!(!hidden.trusted_start_bound && !hidden.uncertain_outcome);
}

fn batch_history_state(hidden_lead: bool) -> (BusState, RoomId, Vec<RequestId>) {
    let mut state = BusState::default();
    let room = state.create_room("batch-work").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/project".into(), None)
        .unwrap();
    let mut requests = Vec::new();
    if hidden_lead {
        let sender = state
            .create_agent(room, "sender", Provider::Codex, "/project".into(), None)
            .unwrap();
        requests.push(
            state
                .submit_bus_message_with(
                    room,
                    Draft {
                        text: "HIDDEN_BATCH_ROUTING_ENVELOPE".into(),
                        files: vec![],
                        recipient_ids: [agent].into(),
                    },
                    Author::Agent(sender),
                    1_000,
                    false,
                )
                .unwrap()[0],
        );
    }
    for (index, text) in [
        "first visible task",
        "second visible task",
        "third visible task",
    ]
    .into_iter()
    .enumerate()
    {
        requests.push(
            state
                .submit_message_from(
                    room,
                    Draft {
                        text: text.into(),
                        files: vec![],
                        recipient_ids: [agent].into(),
                    },
                    Author::Developer,
                    1_001 + index as u64,
                )
                .unwrap()[0],
        );
    }
    assert!(state.coalesce_queue(requests[0]).is_some());
    for id in &requests {
        state.finish_for_test(*id, "SHARED_BATCH_ANSWER", 2_000);
    }
    (state, room, requests)
}

fn batch_history_lines(state: &BusState, room: RoomId) -> Vec<Line> {
    History::default()
        .lines(
            state,
            state.room(room).unwrap(),
            100,
            1,
            3_000,
            &mut Thumbnails::default(),
        )
        .to_vec()
}

fn batch_history_assert_lines(state: &BusState, room: RoomId, visible: &[RequestId]) {
    let lines = batch_history_lines(state, room);
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("SHARED_BATCH_ANSWER"),
        "reply bodies are not history rows: {text}"
    );
    assert!(
        !text.to_lowercase().contains("answered with message"),
        "{text}"
    );
    assert!(!text.contains("Reply to message"), "{text}");
    assert!(lines.iter().all(|line| !line.text.contains('…')), "{text}");
    assert!(
        lines
            .iter()
            .all(|line| !matches!(line.action, Some(Action::Quote(_)))),
        "history has no quote row"
    );
    let mut cursor = 0;
    for id in visible {
        let request = state.request(*id).unwrap();
        let prompt = request.prompt.text.as_str();
        let at = text[cursor..]
            .find(prompt)
            .unwrap_or_else(|| panic!("missing {prompt}: {text}"))
            + cursor;
        assert!(
            lines.iter().any(|line| {
                line.anchor.prompt == request.prompt.id && line.text.contains(prompt)
            }),
            "message {} keeps its prompt row",
            request.prompt.id.0
        );
        cursor = at + prompt.len();
    }
}

#[test]
fn batch_history_tui_places_body_under_first_visible_lead_and_references_each_follower() {
    let (mut state, room, requests) = batch_history_state(false);
    batch_history_assert_lines(&state, room, &requests);
    assert!(state.archive(2_000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    batch_history_assert_lines(&state, room, &requests);
}

#[test]
fn batch_history_tui_hidden_lead_falls_back_to_first_visible_member() {
    let (mut state, room, requests) = batch_history_state(true);
    for archived in [false, true] {
        if archived {
            assert!(state.archive(2_000 + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
        }
        batch_history_assert_lines(&state, room, &requests[1..]);
        assert!(batch_history_lines(&state, room)
            .iter()
            .all(|line| !line.text.contains("HIDDEN_BATCH_ROUTING_ENVELOPE")));
    }
}

#[test]
fn batch_history_tui_individual_answer_does_not_steal_the_later_shared_final() {
    let (state, room, requests) = batch_history_state(false);
    // Persisted shape after an explicit reply answers the original turn lead:
    // its individual answer has no shared group; outstanding members share a new anchor.
    let mut document = serde_json::to_value(state).unwrap();
    document["requests"][requests[0].0.to_string()]["completed_reply"]["group"] =
        serde_json::Value::Null;
    document["requests"][requests[0].0.to_string()]["completed_reply"]["content"]["text"] =
        "INDIVIDUAL_BATCH_ANSWER".into();
    for id in &requests[1..] {
        document["requests"][id.0.to_string()]["completed_reply"]["group"] = requests[1].0.into();
    }
    let state: BusState = serde_json::from_value(document).unwrap();
    let lines = batch_history_lines(&state, room);
    let text = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("INDIVIDUAL_BATCH_ANSWER"), "{text}");
    assert!(!text.contains("SHARED_BATCH_ANSWER"), "{text}");
    assert!(
        !text.to_lowercase().contains("answered with message"),
        "{text}"
    );
    let mut cursor = 0;
    for id in &requests {
        let prompt = state.request(*id).unwrap().prompt.text.clone();
        let at = text[cursor..]
            .find(&prompt)
            .unwrap_or_else(|| panic!("missing {prompt}: {text}"))
            + cursor;
        cursor = at + prompt.len();
    }
}

#[test]
fn batch_history_tui_cross_room_group_keeps_one_body_under_each_rooms_first_visible_lead() {
    let (mut state, work, requests) = batch_history_state(false);
    let master = state.ensure_master_room();
    let mut document = serde_json::to_value(state).unwrap();
    document["requests"][requests[1].0.to_string()]["room_id"] = master.0.into();
    let state: BusState = serde_json::from_value(document).unwrap();
    batch_history_assert_lines(&state, work, &[requests[0], requests[2]]);
    batch_history_assert_lines(&state, master, &[requests[1]]);
    let work_text = batch_history_lines(&state, work)
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let master_text = batch_history_lines(&state, master)
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!work_text.contains("second visible task"), "{work_text}");
    assert!(!master_text.contains("first visible task"), "{master_text}");
    assert!(!master_text.contains("third visible task"), "{master_text}");
}
