use super::local_sessions::{LocalSessionRegistry, ResumeTarget};

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
