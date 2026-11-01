//! Trust lifecycle using scratch source homes; no real provider or credentials.
use super::*;
use std::os::unix::fs::{symlink, MetadataExt};

struct Fixture {
    root: PathBuf,
    source: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = crate::utils::test_temp::unique_temp_path("kimi-shared-trust");
        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("config.toml"), "").unwrap();
        Self { root, source }
    }

    fn launch(&self, name: &str) -> (HookContext, PathBuf) {
        let hooks = HookContext {
            binary: std::env::current_exe().unwrap(),
            spool: self.root.join("callbacks").join(name),
            launch_id: name.into(),
        };
        let home = private_home(&self.source, &hooks).unwrap();
        hook_json::atomic_write(
            &hooks.spool.join(HOME_FILE),
            &serde_json::to_vec(&home_record(&home, &self.source).unwrap()).unwrap(),
        )
        .unwrap();
        (hooks, home)
    }

    fn record(&self) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"root": self.root, "trustedAt": 123})).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn kimi_trust_answer_is_visible_to_a_second_fresh_agent() {
    let fixture = Fixture::new();
    let (_, first) = fixture.launch("first");
    // Simulate Kimi's native trust answer, including its atomic record replacement.
    let trust = first.join("workspace-trust");
    std::fs::create_dir_all(&trust).unwrap();
    std::fs::write(trust.join("pending"), fixture.record()).unwrap();
    std::fs::rename(trust.join("pending"), trust.join("wd_project_key")).unwrap();
    let (_, second) = fixture.launch("second");
    assert_eq!(
        std::fs::read(second.join("workspace-trust/wd_project_key")).unwrap(),
        fixture.record()
    );
}

#[test]
fn kimi_trust_first_launch_creates_an_empty_owner_only_shared_store() {
    let fixture = Fixture::new();
    let (_, home) = fixture.launch("first");
    let shared = fixture.source.join("workspace-trust");
    assert_eq!(
        std::fs::read_link(home.join("workspace-trust")).unwrap(),
        shared
    );
    assert_eq!(std::fs::metadata(&shared).unwrap().mode() & 0o777, 0o700);
    assert_eq!(std::fs::read_dir(shared).unwrap().count(), 0);
}

#[test]
fn kimi_trust_rebind_repairs_a_preexisting_home_without_trust() {
    let fixture = Fixture::new();
    let (hooks, home) = fixture.launch("old");
    let trust = home.join("workspace-trust");
    if trust.is_symlink() {
        std::fs::remove_file(&trust).unwrap();
    }
    assert!(rebind_hooks(&hooks).unwrap());
    assert_eq!(
        std::fs::read_link(&trust).unwrap(),
        fixture.source.join("workspace-trust")
    );
    assert!(!rebind_hooks(&hooks).unwrap());
}

#[test]
fn kimi_trust_rebind_preserves_old_private_decisions_without_overwriting_shared_records() {
    let fixture = Fixture::new();
    let (hooks, home) = fixture.launch("old");
    let trust = home.join("workspace-trust");
    if trust.is_symlink() {
        std::fs::remove_file(&trust).unwrap();
    }
    crate::platform::fs::private_dir(&trust).unwrap();
    std::fs::write(trust.join("wd_project_key"), fixture.record()).unwrap();
    std::fs::write(trust.join("existing"), fixture.record()).unwrap();
    let shared = fixture.source.join("workspace-trust");
    crate::platform::fs::private_dir(&shared).unwrap();
    std::fs::write(shared.join("existing"), b"keep shared bytes").unwrap();
    assert!(rebind_hooks(&hooks).unwrap());
    assert_eq!(std::fs::read_link(&trust).unwrap(), shared);
    assert_eq!(
        std::fs::read(trust.join("wd_project_key")).unwrap(),
        fixture.record()
    );
    assert_eq!(
        std::fs::read(trust.join("existing")).unwrap(),
        b"keep shared bytes"
    );
    let (_, next) = fixture.launch("next");
    assert_eq!(
        std::fs::read(next.join("workspace-trust/wd_project_key")).unwrap(),
        fixture.record()
    );
    assert!(!rebind_hooks(&hooks).unwrap());
}

#[test]
fn kimi_trust_rebind_rejects_an_unexpected_link_without_touching_its_target() {
    let fixture = Fixture::new();
    let (hooks, home) = fixture.launch("old");
    let trust = home.join("workspace-trust");
    if trust.is_symlink() {
        std::fs::remove_file(&trust).unwrap();
    }
    let outside = fixture.root.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"unchanged").unwrap();
    symlink(&outside, &trust).unwrap();
    assert!(rebind_hooks(&hooks).is_err());
    assert_eq!(std::fs::read_link(&trust).unwrap(), outside);
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn kimi_trust_cold_resume_repairs_the_home_before_launch() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let (hooks, home) = fixture.launch("old");
    std::fs::remove_file(home.join("workspace-trust")).unwrap();
    let executable = fixture.source.join("kimi");
    std::fs::write(&executable, b"synthetic executable; never invoked").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    hook_json::atomic_write(
        &hooks.spool.join(EXECUTABLE_FILE),
        &serde_json::to_vec(&(&executable, stamp(&executable).unwrap())).unwrap(),
    )
    .unwrap();
    let (_, _, rebound) = super::super::resume::capture_args(&hooks, &fixture.root).unwrap();
    assert!(rebound);
    assert_eq!(
        std::fs::read_link(home.join("workspace-trust")).unwrap(),
        fixture.source.join("workspace-trust")
    );
}

#[test]
fn kimi_trust_migration_does_not_follow_record_links_or_copy_invalid_records() {
    let fixture = Fixture::new();
    let (hooks, home) = fixture.launch("old");
    let trust = home.join("workspace-trust");
    std::fs::remove_file(&trust).unwrap();
    crate::platform::fs::private_dir(&trust).unwrap();
    let outside = fixture.root.join("outside-record");
    std::fs::write(&outside, fixture.record()).unwrap();
    symlink(&outside, trust.join("linked")).unwrap();
    std::fs::write(trust.join("invalid"), b"not JSON").unwrap();
    rebind_hooks(&hooks).unwrap();
    assert_eq!(std::fs::read_dir(&trust).unwrap().count(), 0);
    assert_eq!(std::fs::read(outside).unwrap(), fixture.record());
    assert!(home
        .join("workspace-trust.before-shared/linked")
        .is_symlink());
}

#[test]
fn kimi_trust_uses_the_configured_sources_linked_store() {
    let fixture = Fixture::new();
    let shared = fixture.root.join("shared");
    crate::platform::fs::private_dir(&shared).unwrap();
    symlink(&shared, fixture.source.join("workspace-trust")).unwrap();
    let (_, home) = fixture.launch("first");
    std::fs::write(home.join("workspace-trust/decision"), fixture.record()).unwrap();
    assert_eq!(
        std::fs::read(shared.join("decision")).unwrap(),
        fixture.record()
    );
}
