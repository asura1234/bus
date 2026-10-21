//! A tick's callbacks share one `state.json` write: what is durable, what
//! stays spooled, and what the UI sees when that write or a callback fails.
use super::*;
use std::time::Instant;

/// An idle agent whose server answers every other request with a bare ok,
/// which a session lookup cannot use.
struct BareServer {
    managed_name: String,
}

impl Transport for BareServer {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentList(_) => {
                let mut info = native_info("pane", "claude", "session");
                info.name = Some(self.managed_name.clone());
                info.interactive_ready = true;
                Ok(ResponseResult::AgentList { agents: vec![info] })
            }
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

fn settled_worker() -> (Worker, AgentId, RoomId, PathBuf) {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    worker.transport = Box::new(BareServer {
        managed_name: format!("bus-r{}-a{}", room.0, agent.0),
    });
    worker.tick().unwrap();
    (worker, agent, room, dir)
}

fn spool_len(dir: &Path) -> usize {
    callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .len()
}

fn saved(dir: &Path) -> BusState {
    JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap()
}

#[test]
fn a_failed_callback_write_moves_the_revision_forward_and_applies_on_recovery() {
    let (mut worker, _, _, dir) = settled_worker();
    for turn in 0..3 {
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start(&format!("own-turn-{turn}"), &format!("task {turn}")),
        );
    }
    let published = worker.snapshot().revision;
    let before = worker.state.clone();
    worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    assert!(worker.tick().is_err());
    assert!(
        worker.snapshot().revision > published,
        "a client never sees the revision go back after a failed write"
    );
    assert!(!worker.state.durable_poll_change_from(&before));
    let (events, _events) = mpsc::channel();
    worker.retry_storage(Instant::now() + Duration::from_secs(2), &events);
    assert!(worker.storage_pause.is_none(), "storage recovered");
    worker.tick().unwrap();
    assert_eq!(
        spool_len(&dir),
        0,
        "the retained callbacks apply after recovery"
    );
    assert_eq!(saved(&dir), worker.state);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_callback_error_mid_tick_still_saves_and_acknowledges_the_callbacks_before_it() {
    let (mut worker, _, room, dir) = settled_worker();
    // A later agent whose fresh session cannot be read back from the server.
    let mut state = worker.state.clone();
    let other = state
        .create_agent(room, "tester", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            other,
            AgentRuntimeIdentity {
                launch_id: Some("launch-b".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane-b".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(other).unwrap();
    state.begin_session_reset(other).unwrap();
    worker.save(state).unwrap();
    let other_spool = dir.join("callbacks/launch-b");
    callbacks::initialize(
        &other_spool,
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(other.0),
            provider: Provider::ClaudeCode,
            launch_id: "launch-b".into(),
        },
    )
    .unwrap();
    // Applied: a turn of the first agent in its known session.
    record(
        &dir,
        Provider::ClaudeCode,
        claude_start("own-turn", "own task"),
    );
    // Fails: the second agent's rebind cannot confirm the new session.
    callbacks::append(
        &other_spool,
        "launch-b",
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"fresh","source":"clear"}),
        &[],
        false,
    )
    .unwrap();
    let before = worker.state.clone();
    assert!(worker.tick().is_err());
    assert!(
        worker.state.durable_poll_change_from(&before),
        "the first agent's callback was applied"
    );
    assert_eq!(saved(&dir), worker.state, "and is durable");
    assert_eq!(spool_len(&dir), 0, "the applied callback is acknowledged");
    assert_eq!(
        callbacks::records(&other_spool).unwrap().len(),
        1,
        "the failed callback stays"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn read_call(id: &str) -> BusCommand {
    let (reply, _) = mpsc::sync_channel(1);
    BusCommand::Control(crate::messaging::control::server::ControlCall {
        request: crate::messaging::control::Request {
            id: id.into(),
            method: "state".into(),
            params: json!({}),
        },
        reply,
        _poll_permit: None,
    })
}

#[test]
fn a_ui_command_queued_after_shutdown_never_runs() {
    let (worker, _, _, dir) = settled_worker();
    let (commands, receiver) = mpsc::sync_channel(16);
    commands.send((1, read_call("read"))).unwrap();
    commands.send((2, BusCommand::Shutdown)).unwrap();
    commands
        .send((3, BusCommand::CreateRoom("after-shutdown".into())))
        .unwrap();
    let (events, _events) = mpsc::channel();
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    worker.run(receiver, events, snapshot);
    assert!(
        saved(&dir)
            .rooms()
            .all(|room| room.name != "after-shutdown"),
        "a UI command never overtakes Shutdown"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
