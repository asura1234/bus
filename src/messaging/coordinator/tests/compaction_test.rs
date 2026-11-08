use super::*;
use crate::messaging::model::BusState;

struct NoAgents;

impl Transport for NoAgents {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(
            method,
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_)
        ));
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
    assert_eq!(posted[0].author, Author::Bus);
    assert_eq!(posted[0].recipient_ids, [orchestrator.unwrap()].into());
    assert_eq!(
        posted[0].text,
        "impl reached 5 compactions; get a handover note and replace it."
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
    state.delete_agent(old, 0).unwrap();
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
    assert_eq!(posted[0].author, Author::Bus);
    assert!(posted[0].recipient_ids.is_empty());
    assert_eq!(
        posted[0].text,
        "impl reached 5 compactions; get a handover note and replace it."
    );
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
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

fn banner_listing(
    agent: &str,
    banner: bool,
    skipped: bool,
) -> Result<ResponseResult, TransportError> {
    let info = serde_json::from_value(json!({
        "terminal_id": "terminal",
        "pane_id": "pane",
        "agent": agent,
        "agent_status": "idle",
        "agent_session": {
            "source": format!("herdr:{agent}"),
            "agent": agent,
            "kind": "id",
            "value": "session"
        },
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "interactive_ready": true,
        "revision": 1,
        "compaction_banner": banner,
        "screen_detection_skipped": skipped
    }))
    .unwrap();
    Ok(ResponseResult::AgentList { agents: vec![info] })
}

fn poll_banner(worker: &mut Worker, at_ms: u64) {
    worker.test_now_ms = Some(at_ms);
    worker.poll().unwrap();
}

/// Five open/idle polls of a pane flag that the real matcher already decided.
/// Returns the saved count and the limit-notice texts.
#[cfg(feature = "dev-tools")]
pub(crate) fn poll_observed_grok_banner(open: bool, idle: bool) -> (u32, Vec<String>) {
    let mut replies = Vec::new();
    for _ in 0..5 {
        replies.push(banner_listing("grok", open, false));
        replies.push(banner_listing("grok", idle, false));
    }
    let (mut worker, agent, room, dir, _) = fixture(Provider::Grok, replies);
    let mut state = worker.state.clone();
    state.rename_agent(agent, "g1").unwrap();
    worker.save(state).unwrap();
    for episode in 0..5 {
        poll_banner(&mut worker, 10_000 + episode * 2);
        worker.refresh_setting_notices().unwrap();
        poll_banner(&mut worker, 10_001 + episode * 2);
        worker.refresh_setting_notices().unwrap();
    }
    let count = worker.state.agent(agent).unwrap().compactions.count;
    let posted = notices(&worker, room)
        .into_iter()
        .map(|prompt| prompt.text)
        .collect();
    cleanup(worker, dir);
    (count, posted)
}

#[test]
fn compaction_banner_grok_counts_once_and_sets_last_at_ms() {
    let (mut worker, agent, _, dir, _) =
        fixture(Provider::Grok, vec![banner_listing("grok", true, false)]);
    poll_banner(&mut worker, 1_700_000_000_000);
    let compactions = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(compactions.count, 1);
    assert_eq!(compactions.last_at_ms, Some(1_700_000_000_000));
    assert!(worker.state.agent(agent).unwrap().compaction_banner_open);
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_narrow_wrap_between_full_lines_counts_once() {
    let full = "Context compacted: 209.9k -> 37.8k tokens (131.2s)";
    let wrapped = "Context compacted: 209.9k -> 37.8k tok\nens (131.2s)";
    let flags = [full, wrapped, full]
        .map(|screen| crate::agents::compaction_banner::visible(Some("grok"), screen));
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Grok,
        flags
            .into_iter()
            .map(|banner| banner_listing("grok", banner, false))
            .collect(),
    );
    poll_banner(&mut worker, 1_000);
    poll_banner(&mut worker, 2_000);
    poll_banner(&mut worker, 3_000);
    assert_eq!(
        worker.state.agent(agent).unwrap().compactions.count,
        1,
        "a wrapped frame of the same banner must not close the latch: {flags:?}"
    );
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_grok_repeat_poll_does_not_increment() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Grok,
        vec![
            banner_listing("grok", true, false),
            banner_listing("grok", true, false),
            banner_listing("grok", true, false),
        ],
    );
    poll_banner(&mut worker, 1_000);
    poll_banner(&mut worker, 2_000);
    // A resize re-render keeps the same banner flag. It is not a new episode.
    poll_banner(&mut worker, 3_000);
    let compactions = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(compactions.count, 1);
    assert_eq!(compactions.last_at_ms, Some(1_000));
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_grok_second_episode_counts() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Grok,
        vec![
            banner_listing("grok", true, false),
            banner_listing("grok", false, false),
            banner_listing("grok", true, false),
        ],
    );
    poll_banner(&mut worker, 1_000);
    poll_banner(&mut worker, 2_000);
    assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 1);
    assert!(!worker.state.agent(agent).unwrap().compaction_banner_open);
    poll_banner(&mut worker, 3_000);
    let compactions = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(compactions.count, 2);
    assert_eq!(compactions.last_at_ms, Some(3_000));
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_grok_limit_notice_posts_once() {
    let mut replies = Vec::new();
    for _ in 0..6 {
        replies.push(banner_listing("grok", true, false));
        replies.push(banner_listing("grok", false, false));
    }
    let (mut worker, agent, room, dir, _) = fixture(Provider::Grok, replies);
    let mut state = worker.state.clone();
    state.rename_agent(agent, "impl").unwrap();
    worker.save(state).unwrap();
    for episode in 0..6 {
        poll_banner(&mut worker, 10_000 + episode * 2);
        worker.refresh_setting_notices().unwrap();
        poll_banner(&mut worker, 10_001 + episode * 2);
        worker.refresh_setting_notices().unwrap();
        let posted = notices(&worker, room);
        if episode < 4 {
            assert!(posted.is_empty(), "episode {episode}");
        } else {
            assert_eq!(posted.len(), 1, "episode {episode}");
            assert_eq!(
                posted[0].text,
                "impl reached 5 compactions; get a handover note and replace it."
            );
        }
    }
    assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 6);
    assert_eq!(notices(&worker, room).len(), 1);
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_skipped_frame_does_not_reopen_the_latch() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Grok,
        vec![
            banner_listing("grok", true, false),
            banner_listing("grok", false, true),
            banner_listing("grok", true, false),
        ],
    );
    poll_banner(&mut worker, 1_000);
    poll_banner(&mut worker, 2_000);
    poll_banner(&mut worker, 3_000);
    let agent = worker.state.agent(agent).unwrap();
    assert_eq!(agent.compactions.count, 1);
    assert_eq!(agent.compactions.last_at_ms, Some(1_000));
    assert!(agent.compaction_banner_open);
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_latch_roundtrip_does_not_recount() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Grok,
        vec![
            banner_listing("grok", true, false),
            banner_listing("grok", true, false),
        ],
    );
    poll_banner(&mut worker, 1_000);
    let saved = serde_json::to_string(&worker.state).unwrap();
    let restored: BusState = serde_json::from_str(&saved).unwrap();
    assert!(restored.agent(agent).unwrap().compaction_banner_open);
    assert_eq!(restored.agent(agent).unwrap().compactions.count, 1);
    worker.state = restored;
    poll_banner(&mut worker, 4_000);
    let agent = worker.state.agent(agent).unwrap();
    assert_eq!(agent.compactions.count, 1);
    assert_eq!(agent.compactions.last_at_ms, Some(1_000));
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_session_reset_closes_the_latch() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Grok,
        vec![
            banner_listing("grok", true, false),
            banner_listing("grok", true, false),
        ],
    );
    poll_banner(&mut worker, 1_000);
    let mut state = worker.state.clone();
    state.begin_session_reset(agent).unwrap();
    worker.save(state).unwrap();
    let reset = worker.state.agent(agent).unwrap();
    assert_eq!(reset.compactions.count, 0);
    assert!(reset.compactions.last_at_ms.is_none());
    assert!(!reset.compaction_banner_open);
    poll_banner(&mut worker, 5_000);
    let again = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(again.count, 1);
    assert_eq!(again.last_at_ms, Some(5_000));
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_kimi_does_not_count() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::Kimi,
        vec![
            banner_listing("kimi", true, false),
            banner_listing("kimi", true, false),
        ],
    );
    poll_banner(&mut worker, 8_000);
    poll_banner(&mut worker, 9_000);
    let compactions = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(compactions.count, 0);
    assert!(compactions.last_at_ms.is_none());
    cleanup(worker, dir);
}

#[test]
fn compaction_banner_claude_hook_counts_once_ignoring_the_pane() {
    let (mut worker, agent, _, dir, _) = fixture(
        Provider::ClaudeCode,
        vec![
            banner_listing("claude", true, false),
            banner_listing("claude", true, false),
        ],
    );
    poll_banner(&mut worker, 1_000);
    assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 0);
    let spool = dir.join("callbacks/launch");
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"session","source":"compact"}),
    );
    let records = callbacks::records(&spool).unwrap();
    assert_eq!(records.len(), 1);
    let original = std::fs::read(&records[0].0).unwrap();
    worker.consume_callbacks(agent, &spool).unwrap();
    let counted = worker.state.agent(agent).unwrap().compactions;
    assert_eq!(counted.count, 1);
    assert!(counted.last_at_ms.is_some());
    // The same envelope replayed after the count was saved is not a second compaction.
    std::fs::write(&records[0].0, original).unwrap();
    worker.consume_callbacks(agent, &spool).unwrap();
    poll_banner(&mut worker, 2_000);
    let after = worker.state.agent(agent).unwrap();
    assert_eq!(after.compactions.count, 1);
    assert_eq!(after.compactions.last_at_ms, counted.last_at_ms);
    assert!(!after.compaction_banner_open);
    cleanup(worker, dir);
}
