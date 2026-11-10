use super::*;
use crate::agents::providers::ProviderKind;
use crate::messaging::coordinator::tests::support::{fixture, queue};
use crate::messaging::coordinator::{AgentRuntimeIdentity, Provider, RoomKind};
use crate::messaging::native::TransportError;
use crate::messaging::storage::state_store::SaveStage;

pub(super) fn launch(root: &Path, name: &str, owner: AgentId, provider: ProviderKind) -> PathBuf {
    let path = root.join("callbacks").join(name);
    spool::initialize(
        &path,
        &spool::Manifest {
            routing_key: spool::RoutingKey(owner.0),
            provider,
            launch_id: name.into(),
        },
    )
    .unwrap();
    std::fs::write(path.join("hook.log"), "callback diagnostic").unwrap();
    path
}

#[test]
fn deletion_collects_every_retired_launch_and_preserves_history_and_other_agents() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let old = launch(&dir, "old-launch", agent, ProviderKind::Codex);
    let mut state = worker.state.clone();
    let other = state
        .create_agent(room, "other", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    let live = launch(&dir, "other-launch", other, ProviderKind::Codex);
    let request = queue(&mut worker, room, agent, "retained historical message");
    let snapshots = dir.join("exit-snapshots");
    std::fs::create_dir(&snapshots).unwrap();
    std::fs::write(snapshots.join("exit.txt"), "saved screen").unwrap();
    std::fs::write(dir.join("callbacks/codex-hook-bridge"), "bridge").unwrap();

    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();

    assert!(!dir.join("callbacks/launch").exists());
    assert!(!old.exists());
    assert!(live.join("manifest.json").is_file());
    assert!(dir.join("callbacks/codex-hook-bridge").is_file());
    assert_eq!(
        std::fs::read_to_string(snapshots.join("exit.txt")).unwrap(),
        "saved screen"
    );
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .failure_reason
            .as_deref(),
        Some("agent_deleted")
    );
    assert!(worker.state.agent(other).is_some());
    drop(worker);
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(saved.agent(agent).is_none());
    assert!(saved.request(request).is_some());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_terminal_close_retains_callback_launches() {
    let failure = Err(TransportError {
        timestamps: Default::default(),
        message: "terminal is still owned".into(),
        code: Some("terminal_stop_failed".into()),
        definitely_rejected: false,
    });
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![failure]);
    let old = launch(&dir, "old-launch", agent, ProviderKind::Codex);
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(dir.join("callbacks/launch/manifest.json").is_file());
    assert!(old.is_dir());
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_final_deletion_save_retains_callbacks_after_terminal_close() {
    let (mut worker, agent, _, dir, calls) = fixture(Provider::Codex, vec![]);
    worker.store.fail_once_at_after(SaveStage::TempWrite, 1);
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .is_err());
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "the final save failed after closing the terminal"
    );
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(dir.join("callbacks/launch/manifest.json").is_file());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn batched_deletion_collects_only_after_commit_and_callback_acknowledgement() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.prepare_delete_agent(agent).unwrap();
    worker.save(state).unwrap();
    let spool = dir.join("callbacks/launch");
    let event = spool.join("event-consumed.json");
    std::fs::write(&event, "consumed").unwrap();
    worker.begin_batch();
    worker
        .acknowledge_spool(&spool, vec![event.clone()])
        .unwrap();
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(spool.is_dir(), "deferred state has not been committed");
    assert!(event.is_file());
    worker.finish_batch().unwrap();
    assert!(!spool.exists());
    assert!(worker.durable_state.agent(agent).is_none());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_batched_deletion_retains_callbacks_and_restores_the_owner() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    state.prepare_delete_agent(agent).unwrap();
    worker.save(state).unwrap();
    worker.begin_batch();
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    worker.store.fail_once_at(SaveStage::TempSync);
    assert!(worker.finish_batch().is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(dir.join("callbacks/launch/manifest.json").is_file());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_deletion_collects_members_and_its_master_orchestrator() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let master = state
        .rooms()
        .find(|r| r.kind == RoomKind::Master)
        .unwrap()
        .id;
    let lead = state
        .create_agent(master, "lead", Provider::Codex, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(lead, room).unwrap();
    worker.save(state).unwrap();
    let lead_spool = launch(&dir, "lead-launch", lead, ProviderKind::Codex);
    worker
        .command(BusCommand::DeleteRoom(room), &mpsc::channel().0)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(worker.state.agent(lead).is_none());
    assert!(!dir.join("callbacks/launch").exists());
    assert!(!lead_spool.exists());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_keeps_a_replacement_agents_current_launch_despite_a_stale_manifest() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let mut state = worker.state.clone();
    let replacement = state
        .create_agent(room, "replacement", Provider::Codex, dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            replacement,
            AgentRuntimeIdentity {
                launch_id: Some("replacement-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    worker.save(state).unwrap();
    let protected = launch(&dir, "replacement-launch", agent, ProviderKind::Codex);
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(!dir.join("callbacks/launch").exists());
    assert!(
        protected.is_dir(),
        "a live launch is protected independently of manifest ownership"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_retains_unattributable_and_mismatched_manifests() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let unknown = dir.join("callbacks/unknown");
    std::fs::create_dir(&unknown).unwrap();
    std::fs::write(unknown.join("manifest.json"), "broken").unwrap();
    let mismatched = launch(&dir, "different-name", agent, ProviderKind::Codex);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(mismatched.join("manifest.json")).unwrap()).unwrap();
    manifest["launch_id"] = "not-the-directory".into();
    std::fs::write(
        mismatched.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(!dir.join("callbacks/launch").exists());
    assert!(unknown.is_dir());
    assert!(mismatched.is_dir());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_keeps_native_kimi_history_at_its_original_path() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Kimi, vec![]);
    let home = dir.join("callbacks/launch/kimi-home");
    crate::platform::fs::private_dir(&home).unwrap();
    std::fs::create_dir_all(home.join("sessions/conversation")).unwrap();
    std::fs::write(
        home.join("sessions/conversation/context.jsonl"),
        "native conversation",
    )
    .unwrap();
    std::fs::write(home.join("config.toml"), "default_model = 'synthetic'\n").unwrap();
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert_eq!(
        std::fs::read_to_string(home.join("sessions/conversation/context.jsonl")).unwrap(),
        "native conversation"
    );
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        "default_model = 'synthetic'\n"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn kimi_captures_without_native_homes_are_collectible() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Kimi, vec![]);
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert!(!dir.join("callbacks/launch").exists());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deleting_an_agent_or_room_keeps_a_left_open_kimi_sessions_home() {
    for delete_room in [false, true] {
        let moved: crate::protocol::api::schema::AgentInfo = serde_json::from_value(
            serde_json::json!({
                "terminal_id":"terminal", "pane_id":"pane", "name":"bus-r1-a2",
                "agent":"kimi", "agent_status":"idle", "workspace_id":"w1",
                "tab_id":"t1", "focused":false, "revision":1,
                "agent_session":{"source":"herdr:kimi", "agent":"kimi", "kind":"id", "value":"moved"}
            }),
        ).unwrap();
        let replies = vec![
            Err(TransportError {
                timestamps: Default::default(),
                message: "session moved".into(),
                code: Some("terminal_identity_changed".into()),
                definitely_rejected: true,
            }),
            Ok(crate::protocol::api::schema::ResponseResult::AgentList {
                agents: vec![moved.clone()],
            }),
            Ok(crate::protocol::api::schema::ResponseResult::AgentInfo { agent: moved }),
        ];
        let (mut worker, agent, room, dir, _) = fixture(Provider::Kimi, replies);
        let home = dir.join("callbacks/launch/kimi-home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("native-session"), "still running").unwrap();
        let (events, received) = mpsc::channel();
        worker
            .command(
                if delete_room {
                    BusCommand::DeleteRoom(room)
                } else {
                    BusCommand::DeleteAgent(agent)
                },
                &events,
            )
            .unwrap();
        assert!(worker.state.agent(agent).is_none());
        assert!(received.try_iter().any(|event| matches!(event, BusEvent::TerminalsLeftOpen(ref terminals) if terminals.len() == 1)));
        assert_eq!(
            std::fs::read_to_string(home.join("native-session")).unwrap(),
            "still running"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn callback_collection_never_follows_directory_or_manifest_links() {
    use std::os::unix::fs::symlink;
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    let outside = dir.join("outside");
    crate::platform::fs::private_dir(&outside).unwrap();
    std::fs::write(outside.join("marker"), "keep outside").unwrap();
    symlink(&outside, dir.join("callbacks/linked-directory")).unwrap();
    let linked_manifest = dir.join("callbacks/linked-manifest");
    std::fs::create_dir(&linked_manifest).unwrap();
    let fake_manifest = outside.join("manifest.json");
    std::fs::write(
        &fake_manifest,
        serde_json::to_vec(&spool::Manifest {
            routing_key: spool::RoutingKey(agent.0),
            provider: ProviderKind::Codex,
            launch_id: "linked-manifest".into(),
        })
        .unwrap(),
    )
    .unwrap();
    symlink(&fake_manifest, linked_manifest.join("manifest.json")).unwrap();
    worker
        .command(BusCommand::DeleteAgent(agent), &mpsc::channel().0)
        .unwrap();
    assert!(!dir.join("callbacks/launch").exists());
    assert!(dir.join("callbacks/linked-directory").is_symlink());
    assert!(linked_manifest.is_dir());
    assert_eq!(
        std::fs::read_to_string(outside.join("marker")).unwrap(),
        "keep outside"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
