use super::*;
use crate::messaging::model::MIN_IDLE_FOR_ALERT_MS;

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
        .filter(|p| p.text.contains("idle agents"))
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
fn one_refresh_persists_both_setting_notices_and_rearms_both_latches() {
    let mut r = setup(false, 2);
    limit(&mut r.worker, 1);
    idle(&mut r.worker, &r.workers);
    let agent = r.workers[0];
    let mut state = r.worker.state.clone();
    for _ in 0..5 {
        state.record_compaction(agent, io::now_ms()).unwrap();
    }
    r.worker.save(state).unwrap();

    refresh(&mut r.worker);
    let notices = &r.worker.state.room(r.room).unwrap().notices;
    assert_eq!(notices.len(), 2);
    assert!(notices[0].text.starts_with("w0 reached 5 compactions;"));
    assert!(notices[1]
        .text
        .starts_with("You have 2 idle agents (w0, w1)."));
    assert!(notices.iter().all(|notice| notice.author == Author::Bus));
    assert!(
        r.worker
            .state
            .agent(agent)
            .unwrap()
            .compactions
            .limit_notice_sent
    );
    assert!(r.worker.state.room(r.room).unwrap().idle_alert_sent);

    let revision = r.worker.revision;
    refresh(&mut r.worker);
    assert_eq!(
        r.worker.revision, revision,
        "an unchanged tick does not save"
    );
    r = reopen(r);
    assert_eq!(r.worker.state.room(r.room).unwrap().notices.len(), 2);
    assert!(
        r.worker
            .state
            .agent(agent)
            .unwrap()
            .compactions
            .limit_notice_sent
    );
    assert!(r.worker.state.room(r.room).unwrap().idle_alert_sent);

    std::fs::write(
        r.worker.settings_path.as_ref().unwrap(),
        json!({"max_compactions_per_agent": 6, "idle_agents_alert": 2}).to_string(),
    )
    .unwrap();
    refresh(&mut r.worker);
    r = reopen(r);
    assert!(
        !r.worker
            .state
            .agent(agent)
            .unwrap()
            .compactions
            .limit_notice_sent
    );
    assert!(!r.worker.state.room(r.room).unwrap().idle_alert_sent);
    assert_eq!(r.worker.state.room(r.room).unwrap().notices.len(), 2);
    cleanup(r);
}

#[test]
fn more_idle_workers_than_the_default_posts_one_bus_alert_to_the_orchestrator() {
    let mut r = setup(true, 8);
    idle(&mut r.worker, &r.workers[..7]);
    refresh(&mut r.worker);
    assert!(alerts(&r.worker, r.room).is_empty(), "7 is not more than 7");
    idle(&mut r.worker, &r.workers[7..]);
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].author, Author::Bus);
    assert_eq!(posted[0].recipient_ids, [r.orchestrator.unwrap()].into());
    assert_eq!(
        posted[0].text,
        "You have 8 idle agents (w0, w1, w2, w3, w4, w5, w6, w7). \
         Either put them to work in parallel on the open PRs, or close the ones \
         you no longer need to save resources. Your call."
    );
    assert_eq!(r.worker.state.room(r.room).unwrap().unread_count, 1);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room), posted);
    cleanup(r);
}

#[test]
fn a_room_without_an_orchestrator_alerts_the_developer() {
    let mut r = setup(false, 2);
    limit(&mut r.worker, 1);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].author, Author::Bus);
    assert!(posted[0].recipient_ids.is_empty());
    assert!(posted[0]
        .text
        .starts_with("You have 2 idle agents (w0, w1)."));
    assert!(r.worker.state.requests().next().is_none());
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
    state.observe_dialog(w[5], true).unwrap();
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
        posted[0]
            .text
            .starts_with("You have 2 idle agents (w0, w1)."),
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

#[test]
fn falling_to_the_setting_rearms_and_the_latch_survives_a_restart() {
    let mut r = setup(true, 3);
    limit(&mut r.worker, 2);
    let w = r.workers.clone();
    idle(&mut r.worker, &w);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);

    // Reopening marks every agent Unavailable until polled; that is not the
    // room falling back to its limit, so the episode stays latched.
    r = reopen(r);
    refresh(&mut r.worker);
    assert!(r.worker.state.room(r.room).unwrap().idle_alert_sent);
    idle(&mut r.worker, &w);
    refresh(&mut r.worker);
    assert_eq!(
        alerts(&r.worker, r.room).len(),
        1,
        "restart does not refire"
    );

    observe(&mut r.worker, &[w[2]], RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    assert!(!r.worker.state.room(r.room).unwrap().idle_alert_sent);
    r = reopen(r);
    assert!(!r.worker.state.room(r.room).unwrap().idle_alert_sent);
    idle(&mut r.worker, &w);
    refresh(&mut r.worker);
    assert_eq!(
        alerts(&r.worker, r.room).len(),
        2,
        "rearmed across a restart"
    );
    cleanup(r);
}

#[test]
fn an_agent_idle_under_a_minute_holds_the_episode_open() {
    let mut r = setup(true, 3);
    limit(&mut r.worker, 2);
    let w = r.workers.clone();
    idle(&mut r.worker, &w);
    refresh(&mut r.worker);
    observe(&mut r.worker, &[w[2]], RuntimeStatus::Idle, 0);
    let mut state = r.worker.state.clone();
    state
        .observe_status(w[2], RuntimeStatus::Working, io::now_ms())
        .unwrap();
    state
        .observe_status(w[2], RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    r.worker.save(state).unwrap();
    refresh(&mut r.worker);
    assert!(r.worker.state.room(r.room).unwrap().idle_alert_sent);
    // Deleting a worker does bring the room back to its limit.
    let mut state = r.worker.state.clone();
    state.prepare_delete_agent(w[2]).unwrap();
    r.worker.save(state).unwrap();
    refresh(&mut r.worker);
    assert!(!r.worker.state.room(r.room).unwrap().idle_alert_sent);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    cleanup(r);
}

#[test]
fn every_coordinator_tick_checks_the_idle_count() {
    let mut r = setup_with(true, 2, |names| Box::new(IdleAgents(names.to_vec())));
    limit(&mut r.worker, 1);
    let w = r.workers.clone();
    let mut state = r.worker.state.clone();
    for agent in &w {
        let name = format!("bus-r{}-a{}", r.room.0, agent.0);
        state.confirm_hook_setup(*agent).unwrap();
        state
            .set_agent_runtime_identity(
                *agent,
                AgentRuntimeIdentity {
                    terminal_id: Some(format!("terminal-{name}")),
                    pane_id: Some(format!("pane-{name}")),
                    ..AgentRuntimeIdentity::default()
                },
            )
            .unwrap();
    }
    r.worker.save(state).unwrap();
    idle(&mut r.worker, &w);
    r.worker.tick().unwrap();
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    cleanup(r);
}

#[test]
fn settings_command_saves_publishes_and_applies_the_idle_agents_alert() {
    let mut r = setup(true, 3);
    idle(&mut r.worker, &r.workers.clone());
    let path = r.worker.settings_path.clone().unwrap();
    let (events, receiver) = mpsc::channel();
    r.worker
        .command(BusCommand::SetIdleAgentsAlert(2), &events)
        .unwrap();
    assert!(
        matches!(receiver.recv().unwrap(), BusEvent::SettingsChanged(settings) if settings.idle_agents_alert == 2)
    );
    assert_eq!(
        crate::messaging::prefs::settings::load(&path)
            .unwrap()
            .idle_agents_alert,
        2
    );
    assert_eq!(alerts(&r.worker, r.room).len(), 1, "applied at once");
    let before = r.worker.state.clone();
    let config = std::fs::read(&path).unwrap();
    for invalid in [0, 11, u32::MAX] {
        assert!(r
            .worker
            .command(BusCommand::SetIdleAgentsAlert(invalid), &events)
            .is_err());
        assert_eq!(r.worker.state, before);
        assert_eq!(std::fs::read(&path).unwrap(), config);
    }
    cleanup(r);
}
