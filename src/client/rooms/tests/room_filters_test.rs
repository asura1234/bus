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
