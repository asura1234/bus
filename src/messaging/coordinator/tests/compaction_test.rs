use super::*;

struct NoAgents;

impl Transport for NoAgents {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(method, Method::AgentList(_)));
        Ok(ResponseResult::AgentList { agents: vec![] })
    }
}

fn setup(orchestrated: bool) -> (Worker, AgentId, RoomId, Option<AgentId>, PathBuf) {
    let dir = crate::utils::test_temp::unique_temp_path("bus-compaction");
    let mut worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    worker.settings_path = Some(dir.join("settings.json"));
    let mut state = worker.state.clone();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(room, "impl", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let orchestrator = orchestrated.then(|| {
        let master = state.master_room().unwrap().id;
        let id = state
            .create_agent(master, "lead", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        state.bind_orchestrator(id, room).unwrap();
        id
    });
    worker.save(state).unwrap();
    (worker, agent, room, orchestrator, dir)
}

fn compact(worker: &mut Worker, agent: AgentId, times: u32) {
    for _ in 0..times {
        let mut state = worker.state.clone();
        state.record_compaction(agent, io::now_ms()).unwrap();
        worker.save(state).unwrap();
        worker.tick().unwrap();
    }
}

fn notices(worker: &Worker, room: RoomId) -> Vec<crate::messaging::model::Prompt> {
    let mut prompts = BTreeMap::new();
    for request in worker.state.requests().filter(|r| r.room_id == room) {
        prompts.insert(request.prompt.id, request.prompt.clone());
    }
    for prompt in &worker.state.room(room).unwrap().notices {
        prompts.insert(prompt.id, prompt.clone());
    }
    prompts
        .into_values()
        .filter(|p| p.text.contains("get a handover note and replace it."))
        .collect()
}

fn limit(worker: &mut Worker, value: u32) {
    std::fs::write(
        worker.settings_path.as_ref().unwrap(),
        json!({"max_compactions_per_agent": value}).to_string(),
    )
    .unwrap();
    worker.tick().unwrap();
}

fn cleanup(worker: Worker, dir: PathBuf) {
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn persisted_compaction_callback_is_not_counted_again_after_spool_replay() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let spool = dir.join("callbacks/launch");
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart", "session_id":"session", "source":"compact"}),
    );
    let records = callbacks::records(&spool).unwrap();
    assert_eq!(records.len(), 1);
    let path = records[0].0.clone();
    let original_record = std::fs::read(&path).unwrap();
    worker.consume_callbacks(agent, &spool).unwrap();
    assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 1);
    drop(worker);

    // A crash after saving state but before unlinking leaves this exact envelope.
    std::fs::write(&path, original_record).unwrap();
    let mut recovered = reopen_saved(&dir);
    recovered.consume_callbacks(agent, &spool).unwrap();
    let count = recovered.state.agent(agent).unwrap().compactions.count;
    cleanup(recovered, dir);
    assert_eq!(count, 1, "a persisted compaction must not be counted twice");
}

#[test]
fn state_saved_before_the_compaction_marker_loads_and_still_counts() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let spool = dir.join("callbacks/launch");
    let compact =
        json!({"hook_event_name":"SessionStart", "session_id":"session", "source":"compact"});
    record(&dir, Provider::ClaudeCode, compact.clone());
    worker.consume_callbacks(agent, &spool).unwrap();
    drop(worker);

    // Rewrite the saved state into the shape older builds wrote.
    let path = dir.join("state.json");
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("\"last_compaction_callback\""));
    let mut document: serde_json::Value = serde_json::from_str(&saved).unwrap();
    fn strip(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                map.remove("last_compaction_callback");
                map.values_mut().for_each(strip);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    strip(&mut document);
    std::fs::write(&path, document.to_string()).unwrap();

    let mut recovered = reopen_saved(&dir);
    let loaded = recovered.state.agent(agent).unwrap();
    assert_eq!(loaded.compactions.count, 1);
    assert_eq!(loaded.last_compaction_callback, None);
    // An agent without the marker counts its next compaction as before.
    record(&dir, Provider::ClaudeCode, compact);
    recovered.consume_callbacks(agent, &spool).unwrap();
    let count = recovered.state.agent(agent).unwrap().compactions.count;
    cleanup(recovered, dir);
    assert_eq!(count, 2);
}

#[test]
fn a_compaction_hook_posts_the_notice_in_the_same_worker_tick() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::ClaudeCode,
        vec![Ok(ResponseResult::AgentList { agents: vec![] })],
    );
    let mut state = worker.state.clone();
    for _ in 0..4 {
        state.record_compaction(agent, io::now_ms()).unwrap();
    }
    worker.save(state).unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({
            "hook_event_name":"SessionStart", "session_id":"session", "source":"compact"
        }),
    );
    worker.tick().unwrap();
    assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 5);
    assert_eq!(notices(&worker, room).len(), 1);
    cleanup(worker, dir);
}

#[test]
fn crossing_the_limit_posts_one_plain_notice_to_the_rooms_orchestrator() {
    let (mut worker, agent, room, orchestrator, dir) = setup(true);
    compact(&mut worker, agent, 4);
    assert!(notices(&worker, room).is_empty());
    compact(&mut worker, agent, 1);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].author, Author::Agent(agent));
    assert_eq!(posted[0].recipient_ids, [orchestrator.unwrap()].into());
    assert_eq!(
        posted[0].text,
        "impl -> orchestrator: reached 5 compactions; get a handover note and replace it."
    );
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    compact(&mut worker, agent, 3);
    worker.tick().unwrap();
    assert_eq!(notices(&worker, room), posted);
    cleanup(worker, dir);
}

#[test]
fn restart_does_not_refire_a_persisted_compaction_notice() {
    let (mut worker, agent, room, _, dir) = setup(true);
    compact(&mut worker, agent, 5);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 1);
    drop(worker);
    let mut reopened = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    reopened.settings_path = Some(dir.join("settings.json"));
    reopened.tick().unwrap();
    compact(&mut reopened, agent, 1);
    assert_eq!(notices(&reopened, room), posted);
    cleanup(reopened, dir);
}

#[test]
fn clear_resets_the_count_and_notice_before_the_new_session_binds() {
    let (mut worker, agent, room, _, dir) = setup(true);
    compact(&mut worker, agent, 5);
    assert_eq!(notices(&worker, room).len(), 1);
    let mut state = worker.state.clone();
    state.begin_session_reset(agent).unwrap();
    assert_eq!(state.agent(agent).unwrap().compactions.count, 0);
    state
        .rebind_reset_session(agent, "fresh-session".into())
        .unwrap();
    worker.save(state).unwrap();
    compact(&mut worker, agent, 4);
    assert_eq!(notices(&worker, room).len(), 1);
    compact(&mut worker, agent, 1);
    assert_eq!(notices(&worker, room).len(), 2);
    cleanup(worker, dir);
}

#[test]
fn a_replacement_agent_starts_from_zero_and_can_notify_again() {
    let (mut worker, old, room, _, dir) = setup(true);
    compact(&mut worker, old, 5);
    assert_eq!(notices(&worker, room).len(), 1);
    let mut state = worker.state.clone();
    state.delete_agent(old).unwrap();
    let new = state
        .create_agent(room, "impl-fresh", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    assert_eq!(state.agent(new).unwrap().compactions.count, 0);
    worker.save(state).unwrap();
    compact(&mut worker, new, 4);
    assert_eq!(notices(&worker, room).len(), 1);
    compact(&mut worker, new, 1);
    assert_eq!(notices(&worker, room).len(), 2);
    cleanup(worker, dir);
}

#[test]
fn raising_above_the_count_rearms_and_lowering_fires_an_armed_notice_once() {
    let (mut worker, agent, room, _, dir) = setup(true);
    compact(&mut worker, agent, 5);
    assert_eq!(notices(&worker, room).len(), 1);
    limit(&mut worker, 4);
    assert_eq!(notices(&worker, room).len(), 1, "already fired stays fired");
    limit(&mut worker, 7);
    compact(&mut worker, agent, 1);
    assert_eq!(notices(&worker, room).len(), 1);
    compact(&mut worker, agent, 1);
    assert_eq!(notices(&worker, room).len(), 2);
    limit(&mut worker, 10);
    limit(&mut worker, 6);
    assert_eq!(notices(&worker, room).len(), 3);
    worker.tick().unwrap();
    assert_eq!(notices(&worker, room).len(), 3);
    cleanup(worker, dir);
}

#[test]
fn a_room_without_an_orchestrator_addresses_the_developer() {
    let (mut worker, agent, room, _, dir) = setup(false);
    compact(&mut worker, agent, 5);
    let posted = notices(&worker, room);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].author, Author::Agent(agent));
    assert!(posted[0].recipient_ids.is_empty());
    assert_eq!(
        posted[0].text,
        "impl -> You: reached 5 compactions; get a handover note and replace it."
    );
    assert!(worker.state.requests().next().is_none());
    cleanup(worker, dir);
}

#[test]
fn orchestrators_never_receive_compaction_limit_notices_for_themselves() {
    let (mut worker, agent, room, orchestrator, dir) = setup(true);
    let orchestrator = orchestrator.unwrap();
    let master = worker.state.master_room().unwrap().id;
    compact(&mut worker, orchestrator, 8);
    // Even an unbound legacy MASTER agent is an orchestrator, not a worker.
    let mut state = worker.state.clone();
    let legacy = state
        .create_agent(master, "legacy-lead", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    compact(&mut worker, legacy, 8);
    assert!(notices(&worker, master).is_empty());
    assert!(notices(&worker, room).is_empty());
    assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 0);
    cleanup(worker, dir);
}

#[test]
fn settings_command_saves_publishes_and_rearms_before_the_next_compaction() {
    let (mut worker, agent, room, _, dir) = setup(true);
    compact(&mut worker, agent, 5);
    let path = worker.settings_path.clone().unwrap();
    let (events, receiver) = mpsc::channel();
    worker
        .command(BusCommand::SetMaxCompactionsPerAgent(6), &events)
        .unwrap();
    assert!(
        matches!(receiver.recv().unwrap(), BusEvent::SettingsChanged(settings) if settings.max_compactions_per_agent == 6)
    );
    let saved = crate::messaging::prefs::settings::load(&path).unwrap();
    assert_eq!(saved.max_compactions_per_agent, 6);
    assert!(
        !worker
            .state
            .agent(agent)
            .unwrap()
            .compactions
            .limit_notice_sent
    );
    compact(&mut worker, agent, 1);
    assert_eq!(notices(&worker, room).len(), 2);
    let before = worker.state.clone();
    let config = std::fs::read(&path).unwrap();
    for invalid in [0, 101, u32::MAX] {
        assert!(worker
            .command(BusCommand::SetMaxCompactionsPerAgent(invalid), &events)
            .is_err());
        assert_eq!(worker.state, before);
        assert_eq!(std::fs::read(&path).unwrap(), config);
    }
    cleanup(worker, dir);
}

#[test]
fn rearmed_notice_stays_rearmed_across_a_restart() {
    let (mut worker, agent, room, _, dir) = setup(true);
    compact(&mut worker, agent, 5);
    limit(&mut worker, 7);
    drop(worker);
    let mut reopened = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    reopened.settings_path = Some(dir.join("settings.json"));
    compact(&mut reopened, agent, 2);
    assert_eq!(notices(&reopened, room).len(), 2);
    cleanup(reopened, dir);
}
