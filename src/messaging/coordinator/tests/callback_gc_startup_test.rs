use super::callback_gc_tests::launch;
use super::*;
use crate::agents::providers::ProviderKind;
use crate::messaging::coordinator::tests::support::fixture;
use crate::messaging::coordinator::Provider;
use crate::messaging::native::TransportError;
use crate::protocol::api::schema::{Method, ResponseResult};

struct NoTransport;

impl Transport for NoTransport {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        panic!("startup collection must not start or query a provider");
    }
}

fn reopen(dir: &Path) -> Worker {
    Worker::open(dir.to_path_buf(), Box::new(NoTransport)).unwrap()
}

#[test]
fn startup_collects_unregistered_attempts_and_preserves_every_live_owners_launch() {
    let (worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let older = launch(&dir, "older-live-launch", agent, ProviderKind::Codex);
    let unregistered = launch(
        &dir,
        "unregistered-attempt",
        AgentId(999_999),
        ProviderKind::Codex,
    );
    std::fs::write(dir.join("callbacks/codex-hook-bridge"), "shared bridge").unwrap();
    drop(worker);
    for _ in 0..2 {
        let recovered = reopen(&dir);
        assert!(!unregistered.exists());
        assert!(older.join("manifest.json").is_file());
        assert!(dir.join("callbacks/launch/manifest.json").is_file());
        assert!(dir.join("callbacks/codex-hook-bridge").is_file());
        assert!(recovered.state.agent(agent).is_some());
        drop(recovered);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_collects_folders_left_by_an_older_servers_committed_deletion() {
    let (worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let old = launch(&dir, "retired-launch", agent, ProviderKind::Codex);
    let mut state = worker.state.clone();
    state.delete_agent(agent, 50).unwrap();
    worker.store.save(&state).unwrap();
    drop(worker);
    let recovered = reopen(&dir);
    assert!(recovered.state.agent(agent).is_none());
    assert!(!old.exists());
    assert!(!dir.join("callbacks/launch").exists());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_keeps_pending_deletions_until_terminal_shutdown_can_finish() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.prepare_delete_agent(agent).unwrap();
    worker.save(state).unwrap();
    let orphan = launch(&dir, "orphan", AgentId(999_999), ProviderKind::Codex);
    drop(worker);
    let recovered = reopen(&dir);
    assert!(recovered.state.agent(agent).unwrap().deletion_pending);
    assert!(dir.join("callbacks/launch").is_dir());
    assert!(!orphan.exists());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unreadable_state_prevents_startup_collection() {
    let (worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    let orphan = launch(&dir, "orphan", AgentId(999_999), ProviderKind::Codex);
    std::fs::write(dir.join("state.json"), "unreadable state").unwrap();
    drop(worker);
    assert!(Worker::open(dir.clone(), Box::new(NoTransport)).is_err());
    assert!(orphan.join("manifest.json").is_file());
    assert!(dir.join("callbacks/launch").is_dir());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_protects_the_current_launch_independently_of_manifest_owner() {
    let (worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    launch(&dir, "launch", AgentId(999_999), ProviderKind::Codex);
    let orphan = launch(&dir, "orphan", AgentId(999_999), ProviderKind::Codex);
    drop(worker);
    let recovered = reopen(&dir);
    assert!(dir.join("callbacks/launch/manifest.json").is_file());
    assert!(!orphan.exists());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn startup_retains_unattributable_or_oversized_metadata() {
    let (worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    let partial = dir.join("callbacks/partial");
    std::fs::create_dir(&partial).unwrap();
    let oversized = launch(&dir, "oversized", AgentId(999_999), ProviderKind::Codex);
    std::fs::write(oversized.join("manifest.json"), vec![b' '; 65 * 1024]).unwrap();
    let orphan = launch(&dir, "orphan", AgentId(999_999), ProviderKind::Codex);
    drop(worker);
    let recovered = reopen(&dir);
    assert!(partial.is_dir());
    assert!(oversized.is_dir());
    assert!(!orphan.exists());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn startup_never_follows_a_replaced_callback_root() {
    use std::os::unix::fs::symlink;
    let (worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    launch(&dir, "orphan", AgentId(999_999), ProviderKind::Codex);
    let outside = dir.join("outside-callbacks");
    std::fs::rename(dir.join("callbacks"), &outside).unwrap();
    symlink(&outside, dir.join("callbacks")).unwrap();
    drop(worker);
    let recovered = reopen(&dir);
    assert!(dir.join("callbacks").is_symlink());
    assert!(outside.join("orphan/manifest.json").is_file());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn startup_retains_native_kimi_history_and_shared_auth_link_targets() {
    use std::os::unix::fs::symlink;
    let (worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    let orphan = launch(&dir, "retired-kimi", AgentId(999_999), ProviderKind::Kimi);
    let home = orphan.join("kimi-home");
    std::fs::create_dir_all(home.join("sessions/conversation")).unwrap();
    std::fs::write(
        home.join("sessions/conversation/context.jsonl"),
        "native history",
    )
    .unwrap();
    let shared = dir.join("shared-auth");
    std::fs::create_dir(&shared).unwrap();
    std::fs::write(shared.join("token"), "synthetic credential").unwrap();
    symlink(&shared, home.join("credentials")).unwrap();
    drop(worker);
    let recovered = reopen(&dir);
    assert!(orphan.is_dir());
    assert_eq!(
        std::fs::read_to_string(home.join("sessions/conversation/context.jsonl")).unwrap(),
        "native history"
    );
    assert_eq!(
        std::fs::read_link(home.join("credentials")).unwrap(),
        shared
    );
    assert_eq!(
        std::fs::read_to_string(shared.join("token")).unwrap(),
        "synthetic credential"
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn startup_retains_uncertain_native_home_links() {
    use std::os::unix::fs::symlink;
    let (worker, _, _, dir, _) = fixture(Provider::Codex, vec![]);
    let orphan = launch(&dir, "retired-kimi", AgentId(999_999), ProviderKind::Kimi);
    let home = orphan.join("kimi-home");
    let outside = dir.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("conversation"), "keep native data").unwrap();
    symlink(&outside, &home).unwrap();
    drop(worker);
    let recovered = reopen(&dir);
    assert_eq!(
        std::fs::read_to_string(home.join("conversation")).unwrap(),
        "keep native data"
    );
    assert!(home.is_symlink());
    assert!(orphan.is_dir());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}
