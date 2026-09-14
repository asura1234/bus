use super::{
    local_sessions::{
        format_session_list, LocalSessionRegistry, LocalSessionSummary, ResumeTarget,
    },
    model::BusState,
    store::JsonStore,
};

fn temp_root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "bus-local-sessions-{label}-{}-{}",
        std::process::id(),
        super::io::now_ns()
    ))
}

struct Fixture(std::path::PathBuf);

impl Fixture {
    fn new(label: &str) -> Self {
        Self(temp_root(label))
    }

    fn registry(&self) -> LocalSessionRegistry {
        LocalSessionRegistry::new(self.0.clone())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn save_rooms(session: &super::local_sessions::LocalSession, names: &[&str]) {
    let mut state = BusState::new();
    for name in names {
        state.create_room(name).unwrap();
    }
    JsonStore::new(session.root.join("state.json"))
        .save(&state)
        .unwrap();
}

#[test]
fn fresh_sessions_have_unique_persisted_ids_and_isolated_roots() {
    let fixture = Fixture::new("fresh");
    let registry = fixture.registry();

    let first = registry.create().unwrap();
    let second = registry.create().unwrap();

    assert_ne!(first.id, second.id);
    assert_eq!(first.root, fixture.0.join("sessions").join(&first.id));
    assert_eq!(second.root, fixture.0.join("sessions").join(&second.id));
    let first_metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(first.root.join("session.json")).unwrap()).unwrap();
    assert_eq!(first_metadata["version"], 1);
    assert_eq!(first_metadata["id"], first.id);
    assert_eq!(registry.resume(ResumeTarget::Last).unwrap(), second);
}

#[test]
fn resuming_an_exact_id_updates_what_last_means() {
    let fixture = Fixture::new("exact");
    let registry = fixture.registry();
    let first = registry.create().unwrap();
    let _second = registry.create().unwrap();

    assert_eq!(
        registry.resume(ResumeTarget::Id(first.id.clone())).unwrap(),
        first
    );
    assert_eq!(registry.resume(ResumeTarget::Last).unwrap(), first);
}

#[test]
fn resume_fails_for_missing_last_missing_id_and_corrupt_metadata() {
    let fixture = Fixture::new("missing");
    let registry = fixture.registry();
    assert_eq!(
        registry.resume(ResumeTarget::Last).unwrap_err(),
        "No local Bus session has been recorded yet"
    );
    assert_eq!(
        registry
            .resume(ResumeTarget::Id("0123456789abcdef".into()))
            .unwrap_err(),
        "Bus session '0123456789abcdef' was not found"
    );

    let session = registry.create().unwrap();
    std::fs::write(session.root.join("session.json"), b"{}").unwrap();
    assert_eq!(
        registry
            .resume(ResumeTarget::Id(session.id.clone()))
            .unwrap_err(),
        format!("Bus session '{}' metadata is invalid", session.id)
    );
}

#[test]
fn resume_rejects_ids_that_could_escape_the_session_directory() {
    let fixture = Fixture::new("unsafe");
    let registry = fixture.registry();

    for id in ["../state", "ABCDEF0123456789", "short", ""] {
        assert_eq!(
            registry
                .resume(ResumeTarget::Id(id.to_owned()))
                .unwrap_err(),
            format!("Invalid Bus session ID '{id}'")
        );
    }
    assert!(!fixture.0.exists());
}

#[test]
fn session_list_returns_room_names_and_omits_empty_sessions() {
    let fixture = Fixture::new("list");
    let registry = fixture.registry();
    let populated = registry.create().unwrap();
    save_rooms(
        &populated,
        &["debug bus message routing", "make bus look good"],
    );
    let empty = registry.create().unwrap();
    save_rooms(&empty, &[]);

    let sessions = registry.list().unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, populated.id);
    assert_eq!(
        sessions[0].room_names,
        ["debug bus message routing", "make bus look good"]
    );
    assert!(!sessions[0].is_last);
}

#[test]
fn session_list_format_keeps_copyable_id_rooms_age_and_last_marker() {
    let now = 7_200_000;
    let sessions = vec![
        LocalSessionSummary {
            id: "0123456789abcdef".into(),
            room_names: vec![
                "debug bus message routing".into(),
                "make bus look good".into(),
            ],
            last_activity_ms: now - 3_600_000,
            is_last: false,
        },
        LocalSessionSummary {
            id: "fedcba9876543210".into(),
            room_names: vec!["wheels on the bus go around and round".into()],
            last_activity_ms: now - 60_000,
            is_last: true,
        },
    ];

    assert_eq!(
        format_session_list(&sessions, now),
        "0123456789abcdef  [# debug bus message routing] [# make bus look good]  1h ago\n\
fedcba9876543210  [# wheels on the bus go around and round]  1m ago  ← last"
    );
}

#[test]
fn discarding_an_empty_session_removes_it_and_repairs_last() {
    let fixture = Fixture::new("discard");
    let registry = fixture.registry();
    let populated = registry.create().unwrap();
    save_rooms(&populated, &["keep"]);
    let empty = registry.create().unwrap();
    save_rooms(&empty, &[]);

    assert!(registry.discard_if_empty(&empty.id).unwrap());
    assert!(!empty.root.exists());
    assert_eq!(registry.resume(ResumeTarget::Last).unwrap(), populated);
    assert!(!registry.discard_if_empty(&populated.id).unwrap());
    assert!(populated.root.exists());
}

#[test]
fn discarding_never_removes_a_session_with_a_live_coordinator() {
    let fixture = Fixture::new("discard-live");
    let registry = fixture.registry();
    let empty = registry.create().unwrap();
    save_rooms(&empty, &[]);
    let _coordinator = super::io::lock(&empty.root.join("coordinator.lock")).unwrap();

    assert!(!registry.discard_if_empty(&empty.id).unwrap());
    assert!(empty.root.exists());
}
