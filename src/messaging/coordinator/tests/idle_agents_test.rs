use super::super::*;
use crate::messaging::model::MIN_IDLE_FOR_ALERT_MS;
use crate::messaging::native::TransportError;
use serde_json::json;

/// Reports each named agent idle and ready, as the native server does.
struct IdleAgents(Vec<String>);

impl Transport for IdleAgents {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(method, Method::AgentList(_)));
        let agents = self
            .0
            .iter()
            .map(|name| {
                serde_json::from_value(json!({
                    "terminal_id": format!("terminal-{name}"), "pane_id": format!("pane-{name}"),
                    "name": name, "agent": "claude", "agent_status": "idle",
                    "interactive_ready": true, "workspace_id": "w1", "tab_id": "t1",
                    "focused": false, "revision": 1
                }))
                .unwrap()
            })
            .collect();
        Ok(ResponseResult::AgentList { agents })
    }
}

struct Fixture {
    worker: Worker,
    room: RoomId,
    orchestrator: Option<AgentId>,
    workers: Vec<AgentId>,
    dir: PathBuf,
}

fn setup(orchestrated: bool, workers: usize) -> Fixture {
    setup_with(orchestrated, workers, |_| Box::new(IdleAgents(Vec::new())))
}

fn setup_with(
    orchestrated: bool,
    workers: usize,
    transport: impl FnOnce(&[String]) -> Box<dyn Transport>,
) -> Fixture {
    let dir = crate::utils::test_temp::unique_temp_path("bus-idle-agents");
    let mut opened = Worker::open(dir.clone(), Box::new(IdleAgents(Vec::new()))).unwrap();
    let mut state = opened.state.clone();
    let room = state.create_room("work").unwrap();
    let ids: Vec<_> = (0..workers)
        .map(|index| {
            state
                .create_agent(
                    room,
                    &format!("w{index}"),
                    Provider::ClaudeCode,
                    dir.clone(),
                    None,
                )
                .unwrap()
        })
        .collect();
    let orchestrator = orchestrated.then(|| {
        let master = state.master_room().unwrap().id;
        let id = state
            .create_agent(master, "lead", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        state.bind_orchestrator(id, room).unwrap();
        id
    });
    opened.save(state).unwrap();
    let names: Vec<_> = ids
        .iter()
        .map(|id| format!("bus-r{}-a{}", room.0, id.0))
        .collect();
    drop(opened);
    let mut worker = Worker::open(dir.clone(), transport(&names)).unwrap();
    worker.settings_path = Some(dir.join("settings.json"));
    Fixture {
        worker,
        room,
        orchestrator,
        workers: ids,
        dir,
    }
}

/// Sets each agent's status as first seen `ago_ms` before now.
fn observe(worker: &mut Worker, agents: &[AgentId], status: RuntimeStatus, ago_ms: u64) {
    let mut state = worker.state.clone();
    let at = io::now_ms().saturating_sub(ago_ms);
    for agent in agents {
        state.observe_status(*agent, status, at).unwrap();
    }
    worker.save(state).unwrap();
}

fn idle(worker: &mut Worker, agents: &[AgentId]) {
    observe(worker, agents, RuntimeStatus::Idle, MIN_IDLE_FOR_ALERT_MS);
}

fn alerts(worker: &Worker, room: RoomId) -> Vec<crate::messaging::model::Prompt> {
    let mut prompts = BTreeMap::new();
    for request in worker.state.requests().filter(|r| r.room_id == room) {
        prompts.insert(request.prompt.id, request.prompt.clone());
    }
    for prompt in &worker.state.room(room).unwrap().notices {
        prompts.insert(prompt.id, prompt.clone());
    }
    prompts
        .into_values()
        .filter(|p| p.text.contains("agents in room"))
        .collect()
}

fn limit(worker: &mut Worker, value: u32) {
    std::fs::write(
        worker.settings_path.as_ref().unwrap(),
        json!({"idle_agents_alert": value}).to_string(),
    )
    .unwrap();
}

/// Restarts the coordinator on the same saved state.
fn reopen(r: Fixture) -> Fixture {
    drop(r.worker);
    let mut worker = Worker::open(r.dir.clone(), Box::new(IdleAgents(Vec::new()))).unwrap();
    worker.settings_path = Some(r.dir.join("settings.json"));
    Fixture { worker, ..r }
}

fn refresh(worker: &mut Worker) {
    worker.refresh_setting_notices().unwrap();
}

fn cleanup(room: Fixture) {
    drop(room.worker);
    std::fs::remove_dir_all(room.dir).unwrap();
}

#[test]
fn crossing_default_limit_fires_once_with_actionable_text_only_to_the_orchestrator() {
    let mut r = setup(true, 7);
    idle(&mut r.worker, &r.workers[..5]);
    refresh(&mut r.worker);
    assert!(
        alerts(&r.worker, r.room).is_empty(),
        "at the limit is quiet"
    );
    idle(&mut r.worker, &r.workers[5..6]);
    let before = r.worker.state.clone();
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].author, Author::Bus);
    assert_eq!(posted[0].recipient_ids, [r.orchestrator.unwrap()].into());
    assert_eq!(
        posted[0].text,
        format!(
        "6 agents in room {} are idle (limit 5): w0, w1, w2, w3, w4, w5. Give them parallel work \
         or delete the ones you no longer need (bus agent delete NAME --confirm).", r.room.0
    )
    );
    assert!(!posted[0].text.contains("[waiting-on-you]"));
    for waiting_only in [false, true] {
        assert!(
            crate::messaging::model::new_developer_notifications(
                &before,
                &r.worker.state,
                waiting_only
            )
            .is_empty(),
            "no TUI developer alert or Feishu page"
        );
    }
    let master = r.worker.state.master_room().unwrap().id;
    assert!(alerts(&r.worker, master).is_empty());
    assert!(r.worker.state.room(r.room).unwrap().notices.is_empty());
    let revision = r.worker.revision;
    for _ in 0..20 {
        refresh(&mut r.worker);
    }
    assert_eq!(r.worker.revision, revision, "unchanged ticks do not save");
    idle(&mut r.worker, &r.workers[6..]);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room), posted, "staying over is quiet");
    cleanup(r);
}

#[test]
fn falling_to_the_limit_or_below_rearms_the_next_crossing() {
    let mut r = setup(true, 3);
    limit(&mut r.worker, 2);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    observe(&mut r.worker, &r.workers[2..], RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    assert!(!r.worker.idle_alerted_rooms.contains(&r.room));
    idle(&mut r.worker, &r.workers[2..]);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 2);
    observe(&mut r.worker, &r.workers, RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 3);
    cleanup(r);
}

#[test]
fn limit_changes_preserve_an_over_limit_episode_and_rearm_when_count_is_within_limit() {
    let mut r = setup(true, 4);
    idle(&mut r.worker, &r.workers);
    let (events, receiver) = mpsc::channel();
    for (value, count) in [(3, 1), (2, 1), (1, 1), (4, 1), (3, 2)] {
        r.worker
            .command(BusCommand::SetIdleAgentsAlert(value), &events)
            .unwrap();
        assert_eq!(alerts(&r.worker, r.room).len(), count);
        assert_eq!(
            settings::load(r.worker.settings_path.as_ref().unwrap())
                .unwrap()
                .idle_agents_alert,
            value
        );
        assert!(receiver.try_iter().any(|event| matches!(event,
            BusEvent::SettingsChanged(saved) if saved.idle_agents_alert == value)));
    }
    for value in [0, 11] {
        assert!(r
            .worker
            .command(BusCommand::SetIdleAgentsAlert(value), &events)
            .is_err());
    }
    assert_eq!(
        settings::load(r.worker.settings_path.as_ref().unwrap())
            .unwrap()
            .idle_agents_alert,
        3
    );
    // Another session's shared setting is also picked up on the next tick.
    limit(&mut r.worker, 4);
    refresh(&mut r.worker);
    limit(&mut r.worker, 2);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 3);
    cleanup(r);
}

#[test]
fn no_orchestrator_means_no_fallback_notice_to_developer_or_master() {
    let mut r = setup(false, 6);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert!(r.worker.state.requests().next().is_none());
    assert!(r.worker.state.rooms().all(|room| room.notices.is_empty()));
    assert!(r.worker.idle_alerted_rooms.is_empty());
    cleanup(r);
}

#[test]
fn deleting_the_bound_orchestrator_does_not_fall_back_to_the_developer() {
    let mut r = setup(true, 6);
    idle(&mut r.worker, &r.workers);
    let mut state = r.worker.state.clone();
    state.prepare_delete_agent(r.orchestrator.unwrap()).unwrap();
    r.worker.save(state).unwrap();
    refresh(&mut r.worker);
    assert!(r.worker.state.requests().next().is_none());
    assert!(r.worker.state.rooms().all(|room| room.notices.is_empty()));
    cleanup(r);
}

#[test]
fn idle_alert_episodes_are_per_room_and_in_memory_only() {
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    let mut state = r.worker.state.clone();
    let other = state.create_room("other").unwrap();
    let master = state.master_room().unwrap().id;
    let lead = state
        .create_agent(
            master,
            "other-lead",
            Provider::ClaudeCode,
            r.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(lead, other).unwrap();
    let workers: Vec<_> = (0..2)
        .map(|i| {
            state
                .create_agent(
                    other,
                    &format!("other-{i}"),
                    Provider::ClaudeCode,
                    r.dir.clone(),
                    None,
                )
                .unwrap()
        })
        .collect();
    r.worker.save(state).unwrap();
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert!(alerts(&r.worker, other).is_empty());
    idle(&mut r.worker, &workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, other)[0].recipient_ids, [lead].into());
    observe(&mut r.worker, &r.workers, RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 2);
    assert_eq!(alerts(&r.worker, other).len(), 1);
    let saved = std::fs::read_to_string(r.dir.join("state.json")).unwrap();
    assert!(!saved.contains("idle_alert"));
    r = reopen(r);
    assert!(r.worker.idle_alerted_rooms.is_empty());
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(
        alerts(&r.worker, r.room).len(),
        3,
        "restart starts fresh episodes"
    );
    cleanup(r);
}

#[test]
fn failed_notice_save_does_not_consume_the_crossing() {
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    idle(&mut r.worker, &r.workers);
    let path = r.dir.join("state.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(r.worker.refresh_setting_notices().is_err());
    assert!(r.worker.idle_alerted_rooms.is_empty());
    assert!(alerts(&r.worker, r.room).is_empty());
    std::fs::remove_dir(&path).unwrap();
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    cleanup(r);
}

#[test]
fn only_idle_workers_without_an_open_or_queued_message_count() {
    let mut r = setup(true, 9);
    limit(&mut r.worker, 1);
    let w = r.workers.clone();
    idle(&mut r.worker, &w);
    // w0 stays idle. Every other worker is excluded in its own way.
    observe(&mut r.worker, &[w[1]], RuntimeStatus::Working, 120_000);
    observe(&mut r.worker, &[w[2]], RuntimeStatus::Blocked, 120_000);
    observe(&mut r.worker, &[w[3]], RuntimeStatus::Launching, 120_000);
    observe(&mut r.worker, &[w[4]], RuntimeStatus::Unavailable, 120_000);
    let mut state = r.worker.state.clone();
    // Codex reports Idle under an approval dialog; it is shown Blocked.
    state.observe_dialog(w[5], true, 120_000).unwrap();
    state.prepare_delete_agent(w[6]).unwrap();
    let queued = state
        .submit_message_from(
            r.room,
            crate::messaging::model::Draft {
                text: "next task".into(),
                files: Vec::new(),
                recipient_ids: [w[7]].into(),
            },
            Author::Developer,
            1,
        )
        .unwrap()[0];
    let opened = state
        .submit_message_from(
            r.room,
            crate::messaging::model::Draft {
                text: "running task".into(),
                files: Vec::new(),
                recipient_ids: [w[8]].into(),
            },
            Author::Developer,
            1,
        )
        .unwrap()[0];
    let mut value = serde_json::to_value(&state).unwrap();
    value["agents"][w[8].0.to_string()]["current_request"] = json!(opened.0);
    value["queues"][w[8].0.to_string()] = json!([]);
    let state: BusState = serde_json::from_value(value).unwrap();
    assert_eq!(state.queued_requests(w[7]), [queued]);
    r.worker.save(state).unwrap();
    refresh(&mut r.worker);
    assert!(alerts(&r.worker, r.room).is_empty());

    observe(
        &mut r.worker,
        &[w[1]],
        RuntimeStatus::Idle,
        MIN_IDLE_FOR_ALERT_MS,
    );
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(posted.len(), 1);
    assert!(
        posted[0].text.starts_with("2 agents in room"),
        "{}",
        posted[0].text
    );
    cleanup(r);
}

#[test]
fn the_orchestrator_never_counts_as_an_idle_agent() {
    let mut r = setup(true, 1);
    limit(&mut r.worker, 1);
    let orchestrator = r.orchestrator.unwrap();
    idle(&mut r.worker, &[r.workers[0], orchestrator]);
    refresh(&mut r.worker);
    assert!(alerts(&r.worker, r.room).is_empty());
    let master = r.worker.state.master_room().unwrap().id;
    assert!(alerts(&r.worker, master).is_empty());
    cleanup(r);
}

#[test]
fn an_agent_counts_only_after_sixty_seconds_continuously_idle() {
    assert_eq!(MIN_IDLE_FOR_ALERT_MS, 60_000);
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    let w = r.workers.clone();
    observe(&mut r.worker, &w, RuntimeStatus::Idle, 59_000);
    refresh(&mut r.worker);
    assert!(
        alerts(&r.worker, r.room).is_empty(),
        "59 s idle does not count"
    );
    observe(&mut r.worker, &w, RuntimeStatus::Working, 0);
    observe(&mut r.worker, &w, RuntimeStatus::Idle, 60_000);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1, "60 s idle counts");
    cleanup(r);
}

#[test]
fn any_status_change_restarts_the_idle_timer() {
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    let w = r.workers.clone();
    observe(&mut r.worker, &w, RuntimeStatus::Idle, 120_000);
    observe(&mut r.worker, &[w[1]], RuntimeStatus::Working, 30_000);
    observe(&mut r.worker, &[w[1]], RuntimeStatus::Idle, 1_000);
    refresh(&mut r.worker);
    assert!(alerts(&r.worker, r.room).is_empty());
    cleanup(r);
}
