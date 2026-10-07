use super::*;

fn sound_fixture(work_rooms: usize) -> (BusUi, RoomId, Vec<RoomId>) {
    let mut state = BusState::default();
    let master = state.ensure_master_room();
    let rooms = (0..work_rooms)
        .map(|index| state.create_room(&format!("room-{index}")).unwrap())
        .collect();
    let ui = BusUi::new(Arc::new(BusSnapshot {
        state,
        revision: 0,
        last_command_id: 0,
        error: None,
    }));
    (ui, master, rooms)
}

fn settings_rows(ui: &mut BusUi, rows: u16) -> Vec<String> {
    let text = room_screen(ui, 100, rows);
    text.chars()
        .collect::<Vec<_>>()
        .chunks(100)
        .map(|row| row.iter().collect::<String>().trim().to_owned())
        .collect()
}

fn queued_sound(ui: &BusUi) -> Vec<(RoomId, bool)> {
    ui.pending
        .iter()
        .filter_map(|p| match p.command {
            BusCommand::SetRoomSound(room, on) => Some((room, on)),
            _ => None,
        })
        .collect()
}

#[test]
fn settings_lists_master_sound_above_every_work_room() {
    let (mut ui, _, _) = sound_fixture(2);
    ui.action(render::Action::Settings);
    let rows = settings_rows(&mut ui, 40);
    let position = |needle: &str| {
        rows.iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("missing {needle}: {rows:#?}"))
    };
    let heading = position("Sound notifications");
    // Group headings share their rows with sidebar text, so match row endings below the heading.
    let below = |suffix: &str| {
        heading
            + rows[heading..]
                .iter()
                .position(|row| row.ends_with(suffix))
                .unwrap_or_else(|| panic!("missing {suffix}: {rows:#?}"))
    };
    let master_group = below("MASTER");
    let master = position("[x] # MASTER");
    let rooms_group = below("ROOMS");
    let new_rooms = position("[ ] New rooms");
    let first = position("[ ] # room-0");
    let second = position("[ ] # room-1");
    assert!(position("Color blind mode") < heading);
    assert!(heading < master_group && master_group < master, "{rows:#?}");
    assert!(master < rooms_group && rooms_group < new_rooms && new_rooms < first);
    assert!(first < second);
}

#[test]
fn settings_keyboard_and_mouse_toggle_room_sound() {
    let (mut ui, master, rooms) = sound_fixture(2);
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 100, 40);
    // Field 0 is still color blind mode.
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    // The new-room default row sits between MASTER and the rooms.
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(queued_sound(&ui), [(master, false)], "Space never toggles");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(queued_sound(&ui), [(master, false), (rooms[0], true)]);
    ui.action(render::Action::ToggleSound(render::SoundTarget::Room(
        rooms[1],
    )));
    assert_eq!(ui.settings_field, 4);
    assert_eq!(queued_sound(&ui).last(), Some(&(rooms[1], true)));
    // Down stops at the last room.
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(ui.settings_field, 4);
    assert!(!ui.settings.color_blind_mode);
}

#[test]
fn settings_new_rooms_row_sets_the_global_default_for_rooms_created_later() {
    let (mut ui, _, _) = sound_fixture(0);
    ui.system_sounds = Some(vec!["Glass".into()]);
    ui.action(render::Action::Settings);
    let rows = settings_rows(&mut ui, 40);
    let new_rooms = rows
        .iter()
        .position(|row| row.contains("[ ] New rooms") && row.ends_with("‹ Default ›"))
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(rows[new_rooms + 1].contains("No rooms yet"), "{rows:#?}");

    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    let queued: Vec<_> = ui
        .pending
        .iter()
        .filter_map(|p| match &p.command {
            BusCommand::SetNewRoomSound(on) => Some(format!("on {on}")),
            BusCommand::SetNewRoomSoundName(name) => Some(format!("name {name:?}")),
            BusCommand::SetRoomSound(..) | BusCommand::SetRoomSoundName(..) => Some("room".into()),
            _ => None,
        })
        .collect();
    assert_eq!(queued, ["on true", "name Some(\"Glass\")"]);
    let rows = settings_rows(&mut ui, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("[x] New rooms") && row.ends_with("‹ Glass ›")),
        "{rows:#?}"
    );

    // The coordinator's saved copy replaces the UI's.
    ui.receive_event(BusEvent::SettingsChanged(
        crate::bus::settings::BusSettings {
            color_blind_mode: true,
            ..Default::default()
        },
    ));
    let rows = settings_rows(&mut ui, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("[ ] New rooms") && row.ends_with("‹ Default ›")),
        "{rows:#?}"
    );
}

fn queued_sound_names(ui: &BusUi) -> Vec<(RoomId, Option<String>)> {
    ui.pending
        .iter()
        .filter_map(|p| match &p.command {
            BusCommand::SetRoomSoundName(room, name) => Some((*room, name.clone())),
            _ => None,
        })
        .collect()
}

fn with_sound_name(ui: &mut BusUi, room: RoomId, name: &str) {
    let mut snapshot = (*ui.snapshot).clone();
    snapshot
        .state
        .set_room_sound_name(room, Some(name.into()))
        .unwrap();
    snapshot.revision += 1;
    ui.receive_snapshot(Arc::new(snapshot));
}

#[test]
fn settings_rows_cycle_through_default_and_the_system_sounds() {
    let (mut ui, master, rooms) = sound_fixture(1);
    ui.system_sounds = Some(vec!["Basso".into(), "Glass".into()]);
    ui.action(render::Action::Settings);
    let rows = settings_rows(&mut ui, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("[x] # MASTER") && row.ends_with("‹ Default ›")),
        "{rows:#?}"
    );
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Right, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Left, KeyModifiers::NONE);
    assert_eq!(
        queued_sound_names(&ui),
        [
            (master, Some("Basso".into())),
            (master, Some("Glass".into()))
        ],
        "Right moves on from Default; Left wraps from Default to the last sound"
    );
    assert!(
        queued_sound(&ui).is_empty(),
        "choosing a sound keeps the checkbox"
    );

    // A saved choice continues from its place in the list, ignoring case.
    with_sound_name(&mut ui, rooms[0], "glass");
    ui.action(render::Action::CycleSound(
        render::SoundTarget::Room(rooms[0]),
        true,
    ));
    assert_eq!(ui.settings_field, 3);
    assert_eq!(queued_sound_names(&ui).last(), Some(&(rooms[0], None)));

    // A sound no longer installed is labelled, and plays the default ding.
    with_sound_name(&mut ui, rooms[0], "Sosumi");
    let rows = settings_rows(&mut ui, 40);
    assert!(
        rows.iter()
            .any(|row| row.contains("[ ] # room-0") && row.ends_with("‹ Sosumi (missing) ›")),
        "{rows:#?}"
    );
    let choice = ui
        .view
        .hits
        .iter()
        .find(|hit| {
            hit.action == render::Action::CycleSound(render::SoundTarget::Room(rooms[0]), false)
        })
        .expect("previous-sound arrow")
        .rect;
    assert_eq!(
        pointer(
            &mut ui,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            choice.x,
            choice.y
        ),
        None
    );
    assert_eq!(
        queued_sound_names(&ui).last(),
        Some(&(rooms[0], Some("Glass".into()))),
        "clicking ‹ steps back from Default"
    );
}

#[test]
fn settings_sound_list_scrolls_to_keep_the_focused_room_visible() {
    let (mut ui, _, rooms) = sound_fixture(30);
    ui.action(render::Action::Settings);
    let rows = settings_rows(&mut ui, 24);
    assert!(!rows.iter().any(|row| row.contains("# room-29")));
    for _ in 0..rooms.len() + 2 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    let rows = settings_rows(&mut ui, 24);
    assert!(
        rows.iter().any(|row| row.contains("# room-29")),
        "{rows:#?}"
    );
    assert!(!rows.iter().any(|row| row.contains("[x] # MASTER")));
    for _ in 0..rooms.len() + 2 {
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    }
    let rows = settings_rows(&mut ui, 24);
    assert!(
        rows.iter().any(|row| row.contains("[x] # MASTER")),
        "{rows:#?}"
    );
}

mod ring_decisions {
    use super::super::super::ring::{
        new_message_should_ring, ringing_room, Ringer, RING_COOLDOWN, STARTUP_GRACE,
    };
    use super::*;
    use std::time::{Duration, Instant};

    fn rooms_with_agents() -> (BusState, RoomId, RoomId, AgentId, AgentId) {
        let mut state = BusState::default();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let orchestrator = state
            .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
            .unwrap();
        let worker = state
            .create_agent(work, "builder", Provider::Codex, "/repo".into(), None)
            .unwrap();
        (state, master, work, orchestrator, worker)
    }

    /// Replies only land through provider callbacks, so tests write the saved shape.
    fn with_reply(state: &BusState, room: RoomId, agent: AgentId, request: u64) -> BusState {
        let mut document = serde_json::to_value(state).unwrap();
        document["rooms"][room.0.to_string()]["latest_replies"][agent.0.to_string()] = serde_json::json!({
            "request_id": request, "agent_id": agent, "text": "done", "received_at_ms": 1
        });
        serde_json::from_value(document).unwrap()
    }

    fn draft(to: AgentId) -> Draft {
        Draft {
            text: "next step".into(),
            files: Vec::new(),
            recipient_ids: AgentRecipients::from([to]),
        }
    }

    #[test]
    fn an_agent_reply_rings_once_in_an_enabled_room() {
        let (state, master, _, orchestrator, _) = rooms_with_agents();
        let replied = with_reply(&state, master, orchestrator, 40);
        assert!(new_message_should_ring(&state, &replied));
        // The same reply seen again is not new.
        assert!(!new_message_should_ring(&replied, &replied));
        let again = with_reply(&replied, master, orchestrator, 41);
        assert!(new_message_should_ring(&replied, &again));
    }

    #[test]
    fn the_ringing_room_is_the_one_that_gained_the_message() {
        let (mut state, _, work, _, worker) = rooms_with_agents();
        state.set_room_sound(work, true).unwrap();
        assert_eq!(
            ringing_room(&state, &with_reply(&state, work, worker, 40)),
            Some(work)
        );
        let mut muted = state.clone();
        muted.set_room_sound(work, false).unwrap();
        assert_eq!(
            ringing_room(&muted, &with_reply(&muted, work, worker, 40)),
            None
        );
    }

    #[test]
    fn disabled_rooms_stay_silent_until_the_human_enables_them() {
        let (mut state, _, work, _, worker) = rooms_with_agents();
        assert!(!new_message_should_ring(
            &state,
            &with_reply(&state, work, worker, 40)
        ));
        state.set_room_sound(work, true).unwrap();
        assert!(new_message_should_ring(
            &state,
            &with_reply(&state, work, worker, 40)
        ));
    }

    #[test]
    fn an_orchestrator_message_to_the_human_rings_like_a_reply() {
        let (state, master, _, orchestrator, _) = rooms_with_agents();
        let mut reported = state.clone();
        reported
            .post_to_human(master, orchestrator, "gate passed".into(), Vec::new(), 1)
            .unwrap();
        assert_eq!(ringing_room(&state, &reported), Some(master));
        assert!(!new_message_should_ring(&reported, &reported));
        // Bus's own notices stay silent.
        let mut noticed = state.clone();
        noticed.post_notice(master, "dialog".into(), 1).unwrap();
        assert!(!new_message_should_ring(&state, &noticed));
    }

    #[test]
    fn agent_authored_messages_ring_and_the_humans_own_never_do() {
        let (mut state, _, work, orchestrator, worker) = rooms_with_agents();
        state.set_room_sound(work, true).unwrap();
        let mut human = state.clone();
        human
            .submit_message_from(work, draft(worker), Author::Human, 5)
            .unwrap();
        assert!(!new_message_should_ring(&state, &human));

        let mut agent = state.clone();
        agent
            .submit_message_from(work, draft(worker), Author::Agent(orchestrator), 5)
            .unwrap();
        assert!(new_message_should_ring(&state, &agent));
        assert!(!new_message_should_ring(&agent, &agent));
    }

    #[test]
    fn ringer_is_silent_while_starting_and_collapses_a_burst() {
        let start = Instant::now();
        let mut ringer = Ringer::new(start);
        assert!(!ringer.allow(start));
        assert!(!ringer.allow(start + STARTUP_GRACE - Duration::from_millis(1)));
        let first = start + STARTUP_GRACE;
        assert!(ringer.allow(first));
        assert!(!ringer.allow(first + Duration::from_millis(10)));
        assert!(!ringer.allow(first + RING_COOLDOWN - Duration::from_millis(1)));
        assert!(ringer.allow(first + RING_COOLDOWN));
    }

    #[test]
    fn bus_ui_without_a_sound_config_never_rings() {
        let (state, master, _, orchestrator, _) = rooms_with_agents();
        let mut ui = BusUi::new(Arc::new(BusSnapshot {
            state: state.clone(),
            revision: 0,
            last_command_id: 0,
            error: None,
        }));
        assert!(ui.sound_config.is_none());
        ui.receive_snapshot(Arc::new(BusSnapshot {
            state: with_reply(&state, master, orchestrator, 40),
            revision: 1,
            last_command_id: 0,
            error: None,
        }));
        // The cooldown was never consumed because nothing played.
        assert!(ui.ringer.allow(Instant::now() + STARTUP_GRACE));
    }
}

mod ring_coalescing {
    use super::super::super::ring::new_message_should_ring;
    use super::*;

    #[test]
    fn an_agent_message_rings_even_when_the_human_sends_before_the_next_snapshot() {
        let mut state = BusState::default();
        state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let author = state
            .create_agent(work, "builder", Provider::Codex, "/repo".into(), None)
            .unwrap();
        let reviewer = state
            .create_agent(work, "reviewer", Provider::ClaudeCode, "/repo".into(), None)
            .unwrap();
        state.set_room_sound(work, true).unwrap();
        let draft = |to| Draft {
            text: "next step".into(),
            files: Vec::new(),
            recipient_ids: AgentRecipients::from([to]),
        };
        // The UI reads only the latest snapshot, so both sends can land between two reads.
        let mut next = state.clone();
        next.submit_message_from(work, draft(reviewer), Author::Agent(author), 5)
            .unwrap();
        next.submit_message_from(work, draft(author), Author::Human, 6)
            .unwrap();
        assert!(new_message_should_ring(&state, &next));
    }

    #[test]
    fn an_agent_reply_still_rings_when_a_human_prompt_is_newer_in_the_same_snapshot() {
        let mut state = BusState::default();
        state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let author = state
            .create_agent(work, "builder", Provider::Codex, "/repo".into(), None)
            .unwrap();
        let reviewer = state
            .create_agent(work, "reviewer", Provider::ClaudeCode, "/repo".into(), None)
            .unwrap();
        state.set_room_sound(work, true).unwrap();
        let mut next = state.clone();
        let mut document = serde_json::to_value(&next).unwrap();
        document["rooms"][work.0.to_string()]["latest_replies"][author.0.to_string()] = serde_json::json!({
            "request_id": 40,
            "agent_id": author.0,
            "text": "done",
            "received_at_ms": 1
        });
        next = serde_json::from_value(document).unwrap();
        next.submit_message_from(
            work,
            Draft {
                text: "ack".into(),
                files: Vec::new(),
                recipient_ids: AgentRecipients::from([reviewer]),
            },
            Author::Human,
            6,
        )
        .unwrap();
        assert!(new_message_should_ring(&state, &next));
    }

    #[test]
    fn an_agent_message_still_rings_when_the_human_writes_in_a_different_room() {
        let mut state = BusState::default();
        let master = state.ensure_master_room();
        let work = state.create_room("work").unwrap();
        let worker = state
            .create_agent(work, "builder", Provider::Codex, "/repo".into(), None)
            .unwrap();
        let orchestrator = state
            .create_agent(master, "orch", Provider::ClaudeCode, "/repo".into(), None)
            .unwrap();
        state.set_room_sound(work, true).unwrap();
        let draft = |to| Draft {
            text: "next step".into(),
            files: Vec::new(),
            recipient_ids: AgentRecipients::from([to]),
        };
        let mut next = state.clone();
        next.submit_message_from(work, draft(worker), Author::Agent(orchestrator), 5)
            .unwrap();
        next.submit_message_from(master, draft(orchestrator), Author::Human, 6)
            .unwrap();
        assert!(new_message_should_ring(&state, &next));
    }
}
