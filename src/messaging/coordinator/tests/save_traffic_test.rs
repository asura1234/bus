//! How often the coordinator rewrites `state.json`, and what waits behind it.
use super::*;

/// The fixture's idle agent, as `agent.list` reports it every poll.
struct IdleAgent {
    managed_name: String,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl Transport for IdleAgent {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.calls
            .lock()
            .unwrap()
            .push(crate::protocol::api::api_method_name(&method));
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

fn idle_worker() -> (
    Worker,
    AgentId,
    RoomId,
    PathBuf,
    Arc<Mutex<Vec<&'static str>>>,
) {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(IdleAgent {
        managed_name: format!("bus-r{}-a{}", room.0, agent.0),
        calls: Arc::clone(&calls),
    });
    // Settle the first observation so later ticks only see the spool.
    worker.tick().unwrap();
    (worker, agent, room, dir, calls)
}

fn spooled(dir: &Path) -> usize {
    callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .len()
}

#[test]
fn a_retained_invalid_callback_is_not_rewritten_every_tick() {
    let (mut worker, agent, _, dir, _) = idle_worker();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"NotARealHook","session_id":"session"}),
    );
    worker.tick().unwrap();
    assert!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_some(),
        "an invalid callback is reported on its agent"
    );
    assert_eq!(spooled(&dir), 1, "an invalid callback stays for inspection");
    let writes = worker.store.writes();
    for _ in 0..5 {
        worker.tick().unwrap();
    }
    assert_eq!(
        worker.store.writes(),
        writes,
        "re-reading the same retained callback changes nothing and must not save"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callbacks_applied_in_one_tick_are_saved_once_before_the_spool_is_cleared() {
    let (mut worker, _, _, dir, _) = idle_worker();
    for turn in 0..5 {
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start(&format!("own-turn-{turn}"), &format!("task {turn}")),
        );
    }
    let before = worker.state.clone();
    let writes = worker.store.writes();
    worker.tick().unwrap();
    assert_eq!(
        spooled(&dir),
        0,
        "every record was applied and acknowledged"
    );
    assert!(worker.state.durable_poll_change_from(&before));
    assert_eq!(
        worker.store.writes(),
        writes + 1,
        "five callbacks applied in one tick are persisted in one write"
    );
    // What was acknowledged is durable.
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved, worker.state);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_failed_batched_save_keeps_every_callback_in_the_spool() {
    let (mut worker, _, _, dir, _) = idle_worker();
    for turn in 0..3 {
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start(&format!("own-turn-{turn}"), &format!("task {turn}")),
        );
    }
    let durable = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    worker
        .store
        .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    assert!(worker.tick().is_err());
    assert_eq!(
        spooled(&dir),
        3,
        "nothing is acknowledged before it is durable"
    );
    assert_eq!(
        JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap(),
        durable
    );
    assert!(
        !worker.state.durable_poll_change_from(&durable),
        "an unsaved callback is not applied in memory either"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

fn status_call(id: usize, message: PromptId) -> BusCommand {
    let (reply, _) = mpsc::sync_channel(1);
    BusCommand::Control(crate::messaging::control::server::ControlCall {
        request: crate::messaging::control::Request {
            id: format!("status-{id}"),
            method: "message.status".into(),
            params: json!({"message": message.0.to_string()}),
        },
        reply,
    })
}

/// Follower polls of a message whose recipient is flagged on a dialog: each
/// one re-reads that recipient's screen.
fn polled_worker() -> (
    Worker,
    AgentId,
    PromptId,
    PathBuf,
    Arc<Mutex<Vec<&'static str>>>,
) {
    let (mut worker, agent, room, dir, calls) = idle_worker();
    let request = queue(&mut worker, room, agent, "build it");
    let message = worker.state.request(request).unwrap().prompt.id;
    let mut state = worker.state.clone();
    state.observe_dialog(agent, true).unwrap();
    worker.save(state).unwrap();
    calls.lock().unwrap().clear();
    (worker, agent, message, dir, calls)
}

fn run_queued(worker: Worker, queued: Vec<BusCommand>) {
    let (commands, receiver) = mpsc::sync_channel(256);
    for (id, command) in queued.into_iter().enumerate() {
        commands.send((id as u64 + 1, command)).unwrap();
    }
    commands.send((999, BusCommand::Shutdown)).unwrap();
    let (events, _events) = mpsc::channel();
    let snapshot = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    worker.run(receiver, events, snapshot);
}

#[test]
fn a_ui_command_queued_behind_follower_polls_runs_first() {
    let (worker, agent, message, dir, calls) = polled_worker();
    let mut queued: Vec<_> = (0..20).map(|id| status_call(id, message)).collect();
    queued.push(BusCommand::FocusTerminal(agent));
    run_queued(worker, queued);
    let calls = calls.lock().unwrap().clone();
    let focus = calls.iter().position(|call| *call == "pane.focus");
    let first_poll = calls
        .iter()
        .position(|call| *call == "agent.dialog.observe");
    assert!(
        focus.is_some() && first_poll.is_some() && focus < first_poll,
        "the developer's command must not wait behind follower polls: {calls:?}"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_ui_command_never_overtakes_a_control_mutation() {
    let (worker, _, message, dir, _) = polled_worker();
    let (reply, _) = mpsc::sync_channel(1);
    let mutation = BusCommand::Control(crate::messaging::control::server::ControlCall {
        request: crate::messaging::control::Request {
            id: "create".into(),
            method: "room.create".into(),
            params: json!({"name": "from-control"}),
        },
        reply,
    });
    let queued = vec![
        status_call(0, message),
        mutation,
        BusCommand::CreateRoom("from-ui".into()),
    ];
    run_queued(worker, queued);
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    let id = |name: &str| saved.rooms().find(|room| room.name == name).unwrap().id;
    assert!(
        id("from-control") < id("from-ui"),
        "FIFO order holds between control mutations and UI commands"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_spool_survives_every_atomic_save_failure_boundary() {
    use crate::messaging::storage::state_store::SaveStage;

    for stage in [
        SaveStage::TempCreate,
        SaveStage::TempWrite,
        SaveStage::TempSync,
        SaveStage::Replace,
        SaveStage::ParentSync,
    ] {
        let (mut worker, _, _, dir, _) = idle_worker();
        for turn in 0..3 {
            record(
                &dir,
                Provider::ClaudeCode,
                claude_start(&format!("own-turn-{turn}"), &format!("task {turn}")),
            );
        }
        let before = worker.state.clone();
        worker.store.fail_once_at(stage);
        assert!(worker.tick().is_err(), "{stage:?}");
        assert_eq!(spooled(&dir), 3, "{stage:?}: no unsynced acknowledgement");
        assert!(
            !worker.state.durable_poll_change_from(&before),
            "{stage:?}: failed batch does not publish its model changes"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
