use super::super::*;
use crate::messaging::model::MIN_IDLE_FOR_ALERT_MS;
use crate::messaging::native::TransportError;
use serde_json::json;

/// Reports each named agent idle and ready, as the native server does.
struct IdleAgents(Vec<String>);

impl Transport for IdleAgents {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(
            method,
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_)
        ));
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
    observe(
        worker,
        agents,
        RuntimeStatus::Idle,
        2 * MIN_IDLE_FOR_ALERT_MS,
    );
}

fn staff(worker: &mut Worker, agent: AgentId, ago_ms: u64, failure: Option<&str>) {
    let at = io::now_ms().saturating_sub(ago_ms);
    let room = worker.state.agent(agent).unwrap().room_id;
    let mut state = worker.state.clone();
    let request = state
        .submit_message_from(
            room,
            crate::messaging::model::Draft {
                text: "staffed work".into(),
                files: Vec::new(),
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            at,
        )
        .unwrap()[0];
    state.finish_for_test(request, "done", at);
    if let Some(reason) = failure {
        let mut value = serde_json::to_value(&state).unwrap();
        value["requests"][request.0.to_string()]["phase"] = json!("abandoned");
        value["requests"][request.0.to_string()]["completed_reply"] = json!(null);
        value["requests"][request.0.to_string()]["failure_reason"] = json!(reason);
        state = serde_json::from_value(value).unwrap();
    }
    worker.save(state).unwrap();
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
fn falling_to_the_limit_rearms_only_for_a_changed_roster() {
    let mut r = setup(true, 4);
    limit(&mut r.worker, 2);
    observe(&mut r.worker, &r.workers[3..], RuntimeStatus::Working, 0);
    idle(&mut r.worker, &r.workers[..3]);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    observe(&mut r.worker, &r.workers[2..3], RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    assert!(!r.worker.idle_alerted_rooms.contains(&r.room));
    idle(&mut r.worker, &r.workers[2..3]);
    refresh(&mut r.worker);
    assert_eq!(
        alerts(&r.worker, r.room).len(),
        1,
        "the same names do not alert twice"
    );
    observe(&mut r.worker, &r.workers, RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 2);
    cleanup(r);
}

#[test]
fn limit_changes_preserve_an_over_limit_episode_without_repeating_a_roster() {
    let mut r = setup(true, 4);
    idle(&mut r.worker, &r.workers);
    let (events, receiver) = mpsc::channel();
    for value in [3, 2, 1, 4, 3] {
        r.worker
            .command(BusCommand::SetIdleAgentsAlert(value), &events)
            .unwrap();
        assert_eq!(alerts(&r.worker, r.room).len(), 1);
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
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
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
fn idle_alert_episodes_are_per_room_and_survive_restart() {
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
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    assert_eq!(alerts(&r.worker, other).len(), 1);
    let saved = std::fs::read_to_string(r.dir.join("state.json")).unwrap();
    assert!(!saved.contains("idle_alert"));
    r = reopen(r);
    assert!(r.worker.idle_alerted_rooms.is_empty());
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(
        alerts(&r.worker, r.room).len(),
        1,
        "the durable roster suppresses a post-restart duplicate"
    );
    assert!(r.worker.idle_alerted_rooms.contains(&r.room));
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
        2 * MIN_IDLE_FOR_ALERT_MS,
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
fn a_new_agent_counts_only_after_two_minutes_continuously_idle() {
    assert_eq!(MIN_IDLE_FOR_ALERT_MS, 60_000);
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    let w = r.workers.clone();
    observe(&mut r.worker, &w, RuntimeStatus::Idle, 119_000);
    refresh(&mut r.worker);
    assert!(
        alerts(&r.worker, r.room).is_empty(),
        "the two-minute creation grace suppresses the alert"
    );
    observe(&mut r.worker, &w, RuntimeStatus::Working, 0);
    observe(&mut r.worker, &w, RuntimeStatus::Idle, 120_000);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    cleanup(r);
}

#[test]
fn staffing_in_the_last_two_minutes_does_not_count_as_spare() {
    let mut r = setup(true, 3);
    limit(&mut r.worker, 2);
    idle(&mut r.worker, &r.workers);
    staff(&mut r.worker, r.workers[2], 0, None);
    refresh(&mut r.worker);
    assert!(alerts(&r.worker, r.room).is_empty());
    cleanup(r);
}

#[test]
fn a_reply_not_captured_agent_does_not_count_as_spare() {
    let mut r = setup(true, 4);
    limit(&mut r.worker, 2);
    idle(&mut r.worker, &r.workers);
    staff(
        &mut r.worker,
        r.workers[3],
        180_000,
        Some("reply_not_captured"),
    );
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(posted.len(), 1);
    assert!(posted[0].text.starts_with("3 agents in room"));
    assert!(!posted[0].text.contains("w3"));
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
fn an_unavailable_flicker_of_still_idle_workers_does_not_repeat_the_reminder() {
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);
    for status in [RuntimeStatus::Unavailable, RuntimeStatus::Launching] {
        observe(&mut r.worker, &r.workers, status, 0);
        refresh(&mut r.worker);
        observe(&mut r.worker, &r.workers, RuntimeStatus::Idle, 0);
        refresh(&mut r.worker);
        idle(&mut r.worker, &r.workers);
        refresh(&mut r.worker);
        assert_eq!(
            alerts(&r.worker, r.room).len(),
            1,
            "workers idle throughout are the same episode"
        );
    }
    cleanup(r);
}

/// A reply_not_captured agent is not spare. Once the spare count is back at the
/// limit, a different spare roster has to alert.
#[test]
fn a_reply_not_captured_idle_agent_rearms_for_a_different_spare_roster() {
    let mut r = setup(true, 4);
    limit(&mut r.worker, 2);
    observe(&mut r.worker, &r.workers[3..], RuntimeStatus::Working, 0);
    idle(&mut r.worker, &r.workers[..3]);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);

    staff(
        &mut r.worker,
        r.workers[2],
        180_000,
        Some("reply_not_captured"),
    );
    refresh(&mut r.worker);
    assert!(
        !r.worker.idle_alerted_rooms.contains(&r.room),
        "spare count at the limit rearms"
    );

    idle(&mut r.worker, &r.workers[3..]);
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(
        posted.len(),
        2,
        "a different spare roster alerts after rearm"
    );
    assert!(posted[1].text.contains("w3"), "{}", posted[1].text);
    assert!(!posted[1].text.contains("w2"), "{}", posted[1].text);
    cleanup(r);
}

/// The saved alert records structured agent IDs even when a label contains a comma.
#[test]
fn a_roster_label_containing_a_comma_does_not_alert_twice() {
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    let mut value = serde_json::to_value(&r.worker.state).unwrap();
    value["agents"][r.workers[0].0.to_string()]["name"] = json!("a, b");
    r.worker
        .save(serde_json::from_value(value).unwrap())
        .unwrap();
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(posted.len(), 1);
    assert_eq!(
        posted[0].idle_agent_ids,
        r.workers.iter().copied().collect()
    );
    observe(&mut r.worker, &r.workers, RuntimeStatus::Working, 0);
    refresh(&mut r.worker);
    r = reopen(r);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(
        alerts(&r.worker, r.room).len(),
        1,
        "the same agent IDs do not alert twice"
    );
    cleanup(r);
}

/// Agents still launching are inside the creation window, so they are not spare.
/// Once the previous spare agents are working, a roster that later clears the
/// window has to alert.
#[test]
fn launching_agents_do_not_keep_a_new_spare_roster_from_alerting() {
    let mut r = setup(true, 2);
    limit(&mut r.worker, 1);
    idle(&mut r.worker, &r.workers);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);

    let mut state = r.worker.state.clone();
    let created: Vec<_> = (0..2)
        .map(|index| {
            state
                .create_agent(
                    r.room,
                    &format!("new{index}"),
                    Provider::ClaudeCode,
                    r.dir.clone(),
                    None,
                )
                .unwrap()
        })
        .collect();
    r.worker.save(state).unwrap();
    observe(&mut r.worker, &r.workers, RuntimeStatus::Working, 0);
    refresh(&mut r.worker);

    observe(
        &mut r.worker,
        &created,
        RuntimeStatus::Idle,
        2 * MIN_IDLE_FOR_ALERT_MS,
    );
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(
        posted.len(),
        2,
        "a spare roster that clears the creation window alerts once"
    );
    assert!(
        posted[1].text.contains(": new0, new1."),
        "{}",
        posted[1].text
    );
    cleanup(r);
}

/// A worker staffed moments ago is not spare, even if its runtime status stays
/// idle. A different roster that is over the limit has to alert.
#[test]
fn a_worker_staffed_inside_two_minutes_does_not_block_a_different_spare_roster() {
    let mut r = setup(true, 4);
    limit(&mut r.worker, 2);
    observe(&mut r.worker, &r.workers[3..], RuntimeStatus::Working, 0);
    idle(&mut r.worker, &r.workers[..3]);
    refresh(&mut r.worker);
    assert_eq!(alerts(&r.worker, r.room).len(), 1);

    staff(&mut r.worker, r.workers[2], 0, None);
    refresh(&mut r.worker);
    idle(&mut r.worker, &r.workers[3..]);
    refresh(&mut r.worker);
    let posted = alerts(&r.worker, r.room);
    assert_eq!(
        posted.len(),
        2,
        "a different spare roster alerts after a recent staffing returns the spare count to the limit"
    );
    assert!(posted[1].text.contains("w3"), "{}", posted[1].text);
    assert!(!posted[1].text.contains("w2"), "{}", posted[1].text);
    cleanup(r);
}
