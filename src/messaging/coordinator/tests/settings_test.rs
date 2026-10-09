use super::*;
use crate::messaging::control::{Request as ControlRequest, Response as ControlResponse};
use crate::messaging::native::TransportError;
use serde_json::{json, Value};

struct NoTerminals;
impl Transport for NoTerminals {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        panic!("settings never reach the terminal server");
    }
}

fn root(label: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("temp")
        .join(format!(
            "bus-global-settings-{label}-{}-{}",
            std::process::id(),
            crate::messaging::storage::io::now_ns()
        ))
}

/// A session in `root/name` sharing `root/settings.json`, as registry sessions do.
fn session(root: &Path, name: &str) -> Worker {
    let mut worker = Worker::open(root.join(name), Box::new(NoTerminals)).unwrap();
    worker.settings_path = Some(root.join("settings.json"));
    let sounds = root.join("sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    for name in ["Blow.aiff", "Glass.aiff"] {
        std::fs::write(sounds.join(name), b"").unwrap();
    }
    worker.sound_dirs = Some(vec![sounds]);
    worker.apply_global_settings().unwrap();
    worker
}

fn sound(worker: &Worker, room: RoomId) -> (bool, Option<String>) {
    let room = worker.state.room(room).unwrap();
    (room.sound_enabled(), room.sound_name.clone())
}

fn master(worker: &Worker) -> RoomId {
    worker.state.master_room().unwrap().id
}

fn create_room(worker: &mut Worker, name: &str) -> RoomId {
    let (events, receiver) = mpsc::channel();
    worker
        .command(BusCommand::CreateRoom(name.into()), &events)
        .unwrap();
    receiver
        .try_iter()
        .find_map(|event| match event {
            BusEvent::RoomCreated(id) => Some(id),
            _ => None,
        })
        .unwrap()
}

fn call(worker: &mut Worker, id: &str, method: &str, params: Value) -> ControlResponse {
    let (events, _receiver) = mpsc::channel();
    worker.dev_response_with_events(
        &ControlRequest {
            id: id.into(),
            method: method.into(),
            params,
        },
        Some(&events),
    )
}

#[test]
fn master_and_all_rooms_sounds_carry_into_every_later_session() {
    let root = root("carry");
    let mut first = session(&root, "first");
    // Defaults until someone chooses: MASTER rings with the ding, rooms stay silent.
    assert_eq!(sound(&first, master(&first)), (true, None));
    let earlier = create_room(&mut first, "earlier");
    assert_eq!(sound(&first, earlier), (false, None));

    let (events, receiver) = mpsc::channel();
    let id = master(&first);
    for command in [
        BusCommand::SetRoomSound(id, false),
        BusCommand::SetRoomSoundName(id, Some("Blow".into())),
        BusCommand::SetAllRoomsSound(true),
        BusCommand::SetAllRoomsSoundName(Some("Glass".into())),
    ] {
        first.command(command, &events).unwrap();
    }
    let changed: Vec<_> = receiver
        .try_iter()
        .filter_map(|event| match event {
            BusEvent::SettingsChanged(settings) => Some(settings.room_sound),
            _ => None,
        })
        .collect();
    let glass = crate::messaging::prefs::settings::SoundPref {
        enabled: true,
        name: Some("Glass".into()),
    };
    assert_eq!(changed.last(), Some(&glass), "the UI shows the new default");
    // All rooms sets existing rooms too; MASTER is not a work room.
    assert_eq!(sound(&first, earlier), (true, Some("Glass".into())));
    assert_eq!(sound(&first, id), (false, Some("Blow".into())));
    // A room's own choice afterwards stays its own and is not global.
    first
        .command(BusCommand::SetRoomSound(earlier, false), &events)
        .unwrap();
    assert_eq!(sound(&first, earlier), (false, Some("Glass".into())));
    let later = create_room(&mut first, "later");
    assert_eq!(sound(&first, later), (true, Some("Glass".into())));

    let mut second = session(&root, "second");
    assert_eq!(
        sound(&second, master(&second)),
        (false, Some("Blow".into()))
    );
    let room = create_room(&mut second, "work");
    assert_eq!(sound(&second, room), (true, Some("Glass".into())));
    // The session saved them, so it reopens with the same values.
    drop(second);
    let reopened = session(&root, "second");
    assert_eq!(sound(&reopened, room), (true, Some("Glass".into())));
    let saved = crate::messaging::prefs::settings::load(&root.join("settings.json")).unwrap();
    assert!(!saved.color_blind_mode);
    drop((first, reopened));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn resumed_sessions_follow_the_global_master_sound() {
    let root = root("resume");
    // A session whose own MASTER plays Blow.
    {
        let mut old = Worker::open(root.join("old"), Box::new(NoTerminals)).unwrap();
        let mut state = old.state.clone();
        state
            .set_room_sound_name(master(&old), Some("Blow".into()))
            .unwrap();
        old.save(state).unwrap();
    }
    // Settings without a MASTER sound: MASTER rings with Bus's ding.
    std::fs::write(root.join("settings.json"), br#"{"color_blind_mode":true}"#).unwrap();
    let old = session(&root, "old");
    assert_eq!(sound(&old, master(&old)), (true, None));
    let saved = crate::messaging::prefs::settings::load(&root.join("settings.json")).unwrap();
    assert!(saved.color_blind_mode, "launching keeps the other settings");
    assert_eq!(
        saved.master_sound,
        crate::messaging::prefs::settings::SoundPref {
            enabled: true,
            name: None
        }
    );
    drop(old);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dev_settings_show_and_set_the_global_sounds() {
    let root = root("dev");
    let mut worker = session(&root, "dev");
    worker.dev_enabled = true;
    let shown = call(&mut worker, "s1", "settings", json!({}));
    assert!(shown.ok, "{shown:?}");
    assert_eq!(
        shown.result["settings"]["room_sound"],
        json!({"enabled": false, "name": null})
    );
    assert_eq!(
        shown.result["settings"]["master_sound"],
        json!({"enabled": true, "name": null})
    );

    let unknown = call(
        &mut worker,
        "r0",
        "settings.room_sound",
        json!({"on": true, "sound": "Nope"}),
    );
    assert!(!unknown.ok);
    let existing = create_room(&mut worker, "existing");
    let set = call(
        &mut worker,
        "r1",
        "settings.room_sound",
        json!({"on": true, "sound": "glass"}),
    );
    assert!(set.ok, "{set:?}");
    assert_eq!(
        set.result["room_sound"],
        json!({"enabled": true, "name": "Glass"})
    );
    // It is the All rooms sound, so rooms that already exist take it too.
    assert_eq!(sound(&worker, existing), (true, Some("Glass".into())));
    // Without --sound the name stays; Default is Bus's own ding.
    call(
        &mut worker,
        "r2",
        "settings.room_sound",
        json!({"on": false}),
    );
    let saved = crate::messaging::prefs::settings::load(&root.join("settings.json")).unwrap();
    assert_eq!(saved.room_sound.name.as_deref(), Some("Glass"));
    call(
        &mut worker,
        "r3",
        "settings.room_sound",
        json!({"on": true, "sound": "Default"}),
    );

    let master_off = call(
        &mut worker,
        "m1",
        "room.sound",
        json!({"room": "master", "on": false, "sound": "Blow"}),
    );
    assert!(master_off.ok, "{master_off:?}");
    let shown = call(&mut worker, "s2", "settings", json!({}));
    assert_eq!(
        shown.result["settings"],
        json!({
            "color_blind_mode": false,
            "max_compactions_per_agent": 5,
            "master_sound": {"enabled": false, "name": "Blow"},
            "room_sound": {"enabled": true, "name": null},
        })
    );
    // state shows each room's effective sound.
    let room = create_room(&mut worker, "work");
    let state = call(&mut worker, "s3", "state", json!({}));
    let rooms = state.result["rooms"].as_array().unwrap();
    let find = |id: RoomId| {
        rooms
            .iter()
            .find(|room| room["id"] == json!(id))
            .unwrap()
            .clone()
    };
    assert_eq!(find(master(&worker))["sound"], false);
    assert_eq!(find(master(&worker))["sound_name"], "Blow");
    assert_eq!(find(room)["sound"], true);
    assert_eq!(find(room)["sound_name"], "Default");
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}
