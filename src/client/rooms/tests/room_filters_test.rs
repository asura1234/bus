use super::*;

fn apply_filter_commands(ui: &mut BusUi) {
    let updates: Vec<_> = ui
        .pending
        .iter()
        .filter_map(|pending| match pending.command {
            BusCommand::SetRoomHistoryFilters(room, filters) => Some((room, filters)),
            _ => None,
        })
        .collect();
    assert!(!updates.is_empty(), "expected a filter command");
    let mut snapshot = (*ui.snapshot).clone();
    for (room, filters) in updates {
        snapshot
            .state
            .set_room_history_filters(room, filters)
            .unwrap();
    }
    snapshot.revision += 1;
    ui.pending.clear();
    ui.receive_snapshot(Arc::new(snapshot));
}

fn assert_visible_directions(ui: &BusUi, to_you: bool, from_you: bool) {
    assert!(ui.history.cached().iter().all(|line| {
        line.text.is_empty()
            || (!to_you && !from_you)
            || (to_you && line.to_developer)
            || (from_you && line.from_developer)
    }));
}

#[test]
fn shortcuts_filter_history_directly_and_combine_with_text_search() {
    for master in [false, true] {
        let (mut ui, room, agent) = fixture();
        let (room, agent) = if master {
            let mut snapshot = (*ui.snapshot).clone();
            let master = snapshot.state.ensure_master_room();
            let orch = snapshot
                .state
                .create_agent(master, "orch", Provider::ClaudeCode, "/p".into(), None)
                .unwrap();
            snapshot.revision += 1;
            ui.receive_snapshot(Arc::new(snapshot));
            (master, orch)
        } else {
            (room, agent)
        };
        mixed_find_history(&mut ui, room, agent);
        ui.open_room(room);
        let unfiltered = room_screen(&mut ui, 100, 60);
        assert!(unfiltered.contains("Needle ordinary peer traffic"));

        key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
        apply_filter_commands(&mut ui);
        let incoming = room_screen(&mut ui, 100, 60);
        assert!(incoming.contains("[x] * → You"), "{incoming}");
        assert!(!incoming.contains("Needle ordinary peer traffic"));
        assert!(incoming.contains("Needle direct ask"));
        assert_visible_directions(&ui, true, false);

        key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
        for c in "needle".chars() {
            key(&mut ui, KeyCode::Char(c), KeyModifiers::NONE);
        }
        room_screen(&mut ui, 100, 60);
        assert_eq!(ui.view.search_matches.len(), 6, "master: {master}");
        key(&mut ui, KeyCode::Char('o'), KeyModifiers::ALT);
        apply_filter_commands(&mut ui);
        room_screen(&mut ui, 100, 60);
        assert_eq!(ui.view.search_matches.len(), 7, "union: master {master}");
        assert_visible_directions(&ui, true, true);

        key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
        apply_filter_commands(&mut ui);
        let outgoing = room_screen(&mut ui, 100, 60);
        assert!(outgoing.contains("[ ] * → You"));
        assert!(outgoing.contains("[x] You → *"));
        assert_eq!(ui.view.search_matches.len(), 1, "master: {master}");
        assert!(!outgoing.contains("Needle direct ask"));
        assert!(outgoing.contains("Needle @you outgoing prompt"));
        assert_visible_directions(&ui, false, true);
    }
    assert!(help::TEXT.contains("Alt+I"));
    assert!(help::TEXT.contains("Alt+O"));
}

#[test]
fn filter_choices_restore_per_room_and_reload_with_client_state() {
    let (mut ui, first, _) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let second = snapshot.state.create_room("second").unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(first);
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);

    ui.open_room(second);
    assert_eq!(
        ui.snapshot.state.room(second).unwrap().history_filters,
        RoomHistoryFilters::default()
    );
    key(&mut ui, KeyCode::Char('o'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);
    ui.open_room(first);
    assert_eq!(
        ui.snapshot.state.room(first).unwrap().history_filters,
        RoomHistoryFilters {
            to_you: true,
            from_you: false,
        }
    );
    ui.open_room(second);
    assert_eq!(
        ui.snapshot.state.room(second).unwrap().history_filters,
        RoomHistoryFilters {
            to_you: false,
            from_you: true,
        }
    );

    let snapshot = Arc::new(BusSnapshot {
        state: ui.snapshot.state.clone(),
        revision: ui.snapshot.revision + 1,
        last_command_id: ui.snapshot.last_command_id,
        error: None,
    });
    let mut reloaded = BusUi::new(snapshot);
    reloaded.open_room(first);
    assert!(
        reloaded
            .snapshot
            .state
            .room(first)
            .unwrap()
            .history_filters
            .to_you
    );
    assert!(
        !reloaded
            .snapshot
            .state
            .room(first)
            .unwrap()
            .history_filters
            .from_you
    );
    reloaded.open_room(second);
    assert!(
        !reloaded
            .snapshot
            .state
            .room(second)
            .unwrap()
            .history_filters
            .to_you
    );
    assert!(
        reloaded
            .snapshot
            .state
            .room(second)
            .unwrap()
            .history_filters
            .from_you
    );
}

#[test]
fn header_checkboxes_are_right_aligned_and_clickable_at_narrow_widths() {
    use crossterm::event::{MouseButton::Left, MouseEventKind::Down};
    for master in [false, true] {
        for cols in [50, 60, 100] {
            let (mut ui, room, _) = fixture();
            let room = if master {
                let mut snapshot = (*ui.snapshot).clone();
                let master = snapshot.state.ensure_master_room();
                snapshot.revision += 1;
                ui.receive_snapshot(Arc::new(snapshot));
                master
            } else {
                room
            };
            ui.open_room(room);
            let screen = room_screen(&mut ui, cols, 30);
            let hit = |ui: &BusUi, action: render::Action| {
                ui.view
                    .hits
                    .iter()
                    .find(|hit| hit.action == action)
                    .unwrap_or_else(|| panic!("missing {action:?} at {cols}: {screen}"))
                    .rect
            };
            let incoming = hit(&ui, render::Action::ToggleToYou);
            let outgoing = hit(&ui, render::Action::ToggleFromYou);
            assert_eq!(incoming.y, 2);
            assert_eq!(outgoing.y, incoming.y);
            assert_eq!(outgoing.right(), ui.view.history_text.right());
            assert_eq!(outgoing.x, incoming.right() + 2);
            assert!(screen.contains("[ ] * → You"), "{screen}");
            assert!(screen.contains("[ ] You → *"), "{screen}");

            mouse(&mut ui, Down(Left), incoming.x + 2, incoming.y);
            apply_filter_commands(&mut ui);
            let checked = room_screen(&mut ui, cols, 30);
            assert!(checked.contains("[x] * → You"), "{checked}");
            assert!(checked.contains("[ ] You → *"), "{checked}");
        }
    }
}

#[test]
fn ctrl_f_keeps_only_text_search_and_uses_the_reduced_panel_height() {
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    room_screen(&mut ui, 100, 30);
    assert_eq!(ui.view.search_box.height, 6);
    assert!(ui.view.hits.iter().all(|hit| {
        !ui.view.search_box.contains((hit.rect.x, hit.rect.y).into())
            || hit.action == render::Action::Search
    }));
    assert!(ui.view.hits.iter().any(|hit| {
        hit.rect.y < ui.view.search_box.y && hit.action == render::Action::ToggleToYou
    }));
}

#[test]
fn rapid_header_toggles_compose_before_snapshot_acknowledgment() {
    let cases = [
        (['i', 'i'], RoomHistoryFilters::default()),
        (
            ['i', 'o'],
            RoomHistoryFilters {
                to_you: true,
                from_you: true,
            },
        ),
    ];
    let mut observed = Vec::new();
    let mut expected = Vec::new();
    for (keys, filters) in cases {
        let (mut ui, room, _) = fixture();
        ui.open_room(room);
        for c in keys {
            key(&mut ui, KeyCode::Char(c), KeyModifiers::ALT);
        }
        apply_filter_commands(&mut ui);
        observed.push(ui.snapshot.state.room(room).unwrap().history_filters);
        expected.push(filters);
    }
    assert_eq!(
        observed, expected,
        "each press must compose with the preceding unacknowledged toggle"
    );
}

#[test]
fn incoming_filter_preserves_a_shared_answer_to_a_developer_member() {
    use crate::messaging::model::{Author, Draft};
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    let peer = snapshot
        .state
        .create_agent(room, "peer", Provider::Codex, "/peer".into(), None)
        .unwrap();
    let mut ids = Vec::new();
    for (author, text) in [
        (Author::Agent(peer), "ordinary worker prompt"),
        (Author::Developer, "developer followup"),
    ] {
        ids.push(
            snapshot
                .state
                .submit_message_from(
                    room,
                    Draft {
                        text: text.into(),
                        files: Vec::new(),
                        recipient_ids: [agent].into(),
                    },
                    author,
                    1000 + ids.len() as u64,
                )
                .unwrap()[0],
        );
    }
    let mut saved = serde_json::to_value(&snapshot.state).unwrap();
    saved["requests"][ids[1].0.to_string()]["group"] = ids[0].0.into();
    snapshot.state = serde_json::from_value(saved).unwrap();
    for id in &ids {
        snapshot
            .state
            .finish_for_test(*id, "SHARED ANSWER FOR DEVELOPER", 2000);
    }
    assert_eq!(
        snapshot.state.request(ids[1]).unwrap().prompt.author,
        Author::Developer
    );
    assert_eq!(
        snapshot
            .state
            .request(ids[1])
            .unwrap()
            .reply()
            .unwrap()
            .text,
        "SHARED ANSWER FOR DEVELOPER"
    );
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    let before = room_screen(&mut ui, 100, 60);
    assert_eq!(
        before.matches("SHARED ANSWER FOR DEVELOPER").count(),
        1,
        "{before}"
    );
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);
    let after = room_screen(&mut ui, 100, 60);
    assert_eq!(
        after.matches("SHARED ANSWER FOR DEVELOPER").count(),
        1,
        "incoming filter must retain exactly one developer reply: {after}"
    );
}

#[test]
fn clicking_a_filter_clears_a_selection_whose_message_is_hidden() {
    use crossterm::event::{
        MouseButton::Left,
        MouseEventKind::{Down, Drag, Up},
    };
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    room_screen(&mut ui, 100, 60);
    let target = "Needle ordinary peer traffic";
    let selected_len = 16usize;
    let index = ui
        .history
        .cached()
        .iter()
        .position(|line| line.text == target)
        .unwrap();
    assert!(
        (ui.main_scroll..ui.main_scroll + usize::from(ui.view.history.height)).contains(&index)
    );
    let x = ui.view.history_text.x;
    let y = ui.view.history_text.y + (index - ui.main_scroll) as u16;
    mouse(&mut ui, Down(Left), x, y);
    mouse(&mut ui, Drag(Left), x + selected_len as u16 - 1, y);
    mouse(&mut ui, Up(Left), x + selected_len as u16 - 1, y);
    let (a, b) = ui
        .history_selection_range()
        .expect("public drag selected history");
    let before = super::super::selection::history_text(ui.history.cached(), a, b);
    assert_eq!(before, &target[..selected_len]);
    let hit = ui
        .view
        .hits
        .iter()
        .find(|hit| hit.action == render::Action::ToggleToYou)
        .unwrap()
        .rect;
    mouse(&mut ui, Down(Left), hit.x + 2, hit.y);
    apply_filter_commands(&mut ui);
    room_screen(&mut ui, 100, 60);
    assert!(!ui.history.cached().iter().any(|line| line.text == target));
    let after = ui
        .history_selection_range()
        .map(|(a, b)| super::super::selection::history_text(ui.history.cached(), a, b));
    assert!(
        ui.history_selection.is_none(),
        "filter changed selected clipboard text from {before:?} to {after:?}"
    );
}

#[test]
fn changing_a_filter_keeps_the_selected_find_match_visible() {
    use crate::messaging::model::{Author, Draft};
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    for i in 0..40u64 {
        let id = snapshot
            .state
            .submit_message_from(
                room,
                Draft {
                    text: format!("Needle outgoing {i:02}"),
                    files: Vec::new(),
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1000 + i * 10,
            )
            .unwrap()[0];
        snapshot
            .state
            .finish_for_test(id, "ordinary answer", 1001 + i * 10);
        snapshot
            .state
            .post_to_developer(
                room,
                agent,
                format!("Needle incoming {i:02}"),
                Vec::new(),
                1002 + i * 10,
            )
            .unwrap();
    }
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    for c in "needle".chars() {
        key(&mut ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
    room_screen(&mut ui, 100, 30);
    assert_eq!(ui.view.search_matches.len(), 80);
    for _ in 0..60 {
        key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    }
    room_screen(&mut ui, 100, 30);
    let current = ui.chat_search.as_ref().unwrap().current;
    let row = ui.view.search_matches[current].0;
    assert!((ui.main_scroll..ui.main_scroll + usize::from(ui.view.history.height)).contains(&row));
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);
    room_screen(&mut ui, 100, 30);
    assert_eq!(ui.view.search_matches.len(), 40);
    let current = ui.chat_search.as_ref().unwrap().current;
    let row = ui.view.search_matches[current].0;
    assert!(
        (ui.main_scroll..ui.main_scroll + usize::from(ui.view.history.height)).contains(&row),
        "Find selects result {current} at row {row}, outside viewport {}..{} after filtering",
        ui.main_scroll,
        ui.main_scroll + usize::from(ui.view.history.height)
    );
}

#[test]
fn finished_filter_commands_compose_until_the_snapshot_acknowledges_them() {
    let cases = [
        ('i', RoomHistoryFilters::default()),
        (
            'o',
            RoomHistoryFilters {
                to_you: true,
                from_you: true,
            },
        ),
    ];
    let mut observed = Vec::new();
    let mut expected = Vec::new();
    for (second_key, expected_filters) in cases {
        let (mut ui, room, _) = fixture();
        ui.open_room(room);
        key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
        let id = ui
            .pending
            .iter()
            .find(|pending| matches!(pending.command, BusCommand::SetRoomHistoryFilters(..)))
            .unwrap()
            .id;
        ui.receive_event(BusEvent::CommandFinished {
            command_id: id,
            result: Ok(()),
        });
        assert!(ui.snapshot.last_command_id < id);
        assert_eq!(
            ui.snapshot.state.room(room).unwrap().history_filters,
            RoomHistoryFilters::default()
        );
        key(&mut ui, KeyCode::Char(second_key), KeyModifiers::ALT);
        apply_filter_commands(&mut ui);
        observed.push(ui.snapshot.state.room(room).unwrap().history_filters);
        expected.push(expected_filters);
    }
    assert_eq!(
        observed, expected,
        "CommandFinished is not a snapshot acknowledgment"
    );
}

#[test]
fn filter_acknowledgment_keeps_find_selection_visible_after_an_old_frame() {
    use crate::messaging::model::{Author, Draft};
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    for i in 0..40u64 {
        let id = snapshot
            .state
            .submit_message_from(
                room,
                Draft {
                    text: format!("Needle outgoing {i:02}"),
                    files: Vec::new(),
                    recipient_ids: [agent].into(),
                },
                Author::Developer,
                1000 + i * 10,
            )
            .unwrap()[0];
        snapshot
            .state
            .finish_for_test(id, "ordinary answer", 1001 + i * 10);
        snapshot
            .state
            .post_to_developer(
                room,
                agent,
                format!("Needle incoming {i:02}"),
                Vec::new(),
                1002 + i * 10,
            )
            .unwrap();
    }
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    for c in "needle".chars() {
        key(&mut ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
    room_screen(&mut ui, 100, 30);
    assert_eq!(ui.view.search_matches.len(), 40);
    let row = ui.view.search_matches[ui.chat_search.as_ref().unwrap().current].0;
    assert!((ui.main_scroll..ui.main_scroll + usize::from(ui.view.history.height)).contains(&row));
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    room_screen(&mut ui, 100, 30);
    assert!(!ui.chat_search.as_ref().unwrap().jump_to_newest);
    apply_filter_commands(&mut ui);
    room_screen(&mut ui, 100, 30);
    assert_eq!(ui.view.search_matches.len(), 80);
    let current = ui.chat_search.as_ref().unwrap().current;
    let row = ui.view.search_matches[current].0;
    assert!(
        (ui.main_scroll..ui.main_scroll + usize::from(ui.view.history.height)).contains(&row),
        "snapshot selects result {current} at row {row}, outside viewport {}..{}",
        ui.main_scroll,
        ui.main_scroll + usize::from(ui.view.history.height)
    );
}

#[test]
fn stale_filter_frame_does_not_move_find_selection() {
    let mut search = Some(super::super::chat_search::ChatSearch {
        query: "needle".into(),
        current: 5,
        jump_to_newest: true,
    });
    let mut view = render::View::default();
    view.search_matches = vec![(0, 0..1)];
    let (mut scroll, mut follow_tail) = (0, true);
    super::super::render::room::update_chat_search(
        &mut search,
        &mut view,
        0,
        100,
        &mut scroll,
        &mut follow_tail,
        false,
    );
    let state = search.as_ref().unwrap();
    assert_eq!(state.current, 5);
    assert!(state.jump_to_newest);

    view.search_matches.push((1, 0..1));
    super::super::render::room::update_chat_search(
        &mut search,
        &mut view,
        0,
        100,
        &mut scroll,
        &mut follow_tail,
        true,
    );
    assert_eq!(search.as_ref().unwrap().current, 1);
    assert!(!search.as_ref().unwrap().jump_to_newest);
}

#[test]
fn background_layout_carries_filters_and_releases_find_jump() {
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    ui.history.enable_background().unwrap();
    key(&mut ui, KeyCode::Char('f'), KeyModifiers::CONTROL);
    for c in "needle".chars() {
        key(&mut ui, KeyCode::Char(c), KeyModifiers::NONE);
    }
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);
    for _ in 0..1_000 {
        room_screen(&mut ui, 100, 30);
        if ui.history.current_filters().to_you && !ui.chat_search.as_ref().unwrap().jump_to_newest {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(ui.history.current_filters().to_you);
    assert!(!ui.chat_search.as_ref().unwrap().jump_to_newest);
    assert_eq!(
        ui.chat_search.as_ref().unwrap().current,
        ui.view.search_matches.len() - 1
    );
}

#[test]
fn incoming_message_during_filter_ack_keeps_checkbox_and_hides_peer_traffic() {
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    let unfiltered = room_screen(&mut ui, 100, 60);
    assert!(unfiltered.contains("Needle ordinary peer traffic"));
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    let id = ui
        .pending
        .iter()
        .find(|pending| matches!(pending.command, BusCommand::SetRoomHistoryFilters(..)))
        .unwrap()
        .id;
    // A message snapshot can arrive before the filter command's save is acknowledged.
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(
            room,
            agent,
            "new matching direct post".into(),
            vec![],
            200_000,
        )
        .unwrap();
    snapshot
        .state
        .submit_message_from(
            room,
            Draft {
                text: "new nonmatching peer post".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Agent(agent),
            200_001,
        )
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.receive_event(BusEvent::CommandFinished {
        command_id: id,
        result: Ok(()),
    });
    let screen = room_screen(&mut ui, 100, 60);
    assert!(
        screen.contains("[x] * → You"),
        "pending user choice cleared by incoming snapshot: {screen}"
    );
    assert!(screen.contains("new matching direct post"), "{screen}");
    assert!(!screen.contains("Needle ordinary peer traffic"), "{screen}");
    assert!(!screen.contains("new nonmatching peer post"), "{screen}");
    apply_filter_commands(&mut ui);
    let screen = room_screen(&mut ui, 100, 60);
    assert!(screen.contains("[x] * → You"), "{screen}");
    assert!(!screen.contains("Needle ordinary peer traffic"), "{screen}");
}

#[test]
fn incoming_messages_and_rebuilds_keep_acknowledged_filters_enabled() {
    for key_char in ['i', 'o'] {
        let (mut ui, room, agent) = fixture();
        mixed_find_history(&mut ui, room, agent);
        ui.open_room(room);
        key(&mut ui, KeyCode::Char(key_char), KeyModifiers::ALT);
        apply_filter_commands(&mut ui);
        let mut snapshot = (*ui.snapshot).clone();
        snapshot
            .state
            .post_to_developer(
                room,
                agent,
                "new matching direct post".into(),
                vec![],
                200_000,
            )
            .unwrap();
        snapshot
            .state
            .submit_message_from(
                room,
                Draft {
                    text: "new nonmatching peer post".into(),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Agent(agent),
                200_001,
            )
            .unwrap();
        snapshot.revision += 1;
        ui.receive_snapshot(Arc::new(snapshot));
        for width in [100, 80, 100] {
            let screen = room_screen(&mut ui, width, 60);
            assert!(
                screen.contains(if key_char == 'i' {
                    "[x] * → You"
                } else {
                    "[x] You → *"
                }),
                "{screen}"
            );
            assert!(!screen.contains("new nonmatching peer post"), "{screen}");
            assert_visible_directions(&ui, key_char == 'i', key_char == 'o');
        }
    }
}

#[test]
fn background_refresh_never_presents_unfiltered_rows_under_a_checked_filter() {
    let (mut ui, room, agent) = fixture();
    mixed_find_history(&mut ui, room, agent);
    ui.open_room(room);
    ui.history.enable_background().unwrap();
    let started = std::time::Instant::now();
    loop {
        let screen = room_screen(&mut ui, 100, 60);
        if screen.contains("Needle ordinary peer traffic") {
            break;
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        std::thread::yield_now();
    }
    key(&mut ui, KeyCode::Char('i'), KeyModifiers::ALT);
    apply_filter_commands(&mut ui);
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(
            room,
            agent,
            "new matching during refresh".into(),
            vec![],
            200_000,
        )
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    let screen = room_screen(&mut ui, 100, 60);
    assert!(screen.contains("[x] * → You"), "{screen}");
    assert!(
        !screen.contains("Needle ordinary peer traffic"),
        "old unfiltered background layout leaked under the checked filter: {screen}"
    );
}

#[test]
fn filtered_page_that_fits_viewport_still_loads_older_on_scroll() {
    use crossterm::event::MouseEventKind::ScrollUp;
    let (mut ui, room, agent) = fixture();
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .post_to_developer(room, agent, "older direct needed".into(), vec![], 1000)
        .unwrap();
    for index in 0..260 {
        snapshot
            .state
            .submit_message_from(
                room,
                Draft {
                    text: format!("hidden peer {index}"),
                    files: vec![],
                    recipient_ids: [agent].into(),
                },
                Author::Agent(agent),
                1001 + index,
            )
            .unwrap();
    }
    for index in 0..40 {
        snapshot
            .state
            .post_to_developer(
                room,
                agent,
                format!("recent direct {index}"),
                vec![],
                2000 + index,
            )
            .unwrap();
    }
    snapshot
        .state
        .set_room_history_filters(
            room,
            RoomHistoryFilters {
                to_you: true,
                from_you: false,
            },
        )
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
    ui.open_room(room);
    let initial = room_screen(&mut ui, 100, 200);
    assert!(!initial.contains("older direct needed"));
    assert_eq!(ui.view.history_max_scroll, 0);
    for _ in 0..3 {
        let rect = ui.view.history_text;
        mouse(&mut ui, ScrollUp, rect.x + 2, rect.y + 2);
        room_screen(&mut ui, 100, 200);
    }
    assert!(ui.history.cached().iter().any(|line| line.text == "older direct needed"),
        "scrolling back must reach older matches even when the current filtered page fits the viewport");
    assert_visible_directions(&ui, true, false);
}
