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
    let first = position("[ ] # room-0");
    let second = position("[ ] # room-1");
    assert!(position("Color blind mode") < heading);
    assert!(heading < master_group && master_group < master, "{rows:#?}");
    assert!(master < rooms_group && rooms_group < first && first < second);
}

#[test]
fn settings_keyboard_and_mouse_toggle_room_sound() {
    let (mut ui, master, rooms) = sound_fixture(2);
    ui.action(render::Action::Settings);
    room_screen(&mut ui, 100, 40);
    // Field 0 is still color blind mode.
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    key(&mut ui, KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(queued_sound(&ui), [(master, false)], "Space never toggles");
    key(&mut ui, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(queued_sound(&ui), [(master, false), (rooms[0], true)]);
    ui.action(render::Action::ToggleRoomSound(rooms[1]));
    assert_eq!(ui.settings_field, 3);
    assert_eq!(queued_sound(&ui).last(), Some(&(rooms[1], true)));
    // Down stops at the last room.
    key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    assert_eq!(ui.settings_field, 3);
    assert!(!ui.settings.color_blind_mode);
}

#[test]
fn settings_sound_list_scrolls_to_keep_the_focused_room_visible() {
    let (mut ui, _, rooms) = sound_fixture(30);
    ui.action(render::Action::Settings);
    let rows = settings_rows(&mut ui, 24);
    assert!(!rows.iter().any(|row| row.contains("# room-29")));
    for _ in 0..rooms.len() + 1 {
        key(&mut ui, KeyCode::Down, KeyModifiers::NONE);
    }
    let rows = settings_rows(&mut ui, 24);
    assert!(
        rows.iter().any(|row| row.contains("# room-29")),
        "{rows:#?}"
    );
    assert!(!rows.iter().any(|row| row.contains("[x] # MASTER")));
    for _ in 0..rooms.len() + 1 {
        key(&mut ui, KeyCode::Up, KeyModifiers::NONE);
    }
    let rows = settings_rows(&mut ui, 24);
    assert!(
        rows.iter().any(|row| row.contains("[x] # MASTER")),
        "{rows:#?}"
    );
}
