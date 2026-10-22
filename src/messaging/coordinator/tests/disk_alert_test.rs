use super::*;

const GB: u64 = 1_000_000_000;

#[path = "disk_sampling_test.rs"]
mod sampling;

struct NoAgents;
impl Transport for NoAgents {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        assert!(matches!(method, Method::AgentList(_)));
        Ok(ResponseResult::AgentList { agents: vec![] })
    }
}

fn setup() -> (Worker, RoomId, RoomId, AgentId, PathBuf) {
    let dir = crate::utils::test_temp::unique_temp_path("bus-disk-alert");
    let mut worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    worker.settings_path = Some(dir.join("settings.json"));
    let mut state = worker.state.clone();
    let room = state.create_room("orchestrated").unwrap();
    let plain = state.create_room("plain").unwrap();
    let master = state.master_room().unwrap().id;
    let lead = state
        .create_agent(master, "lead", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(lead, room).unwrap();
    worker.save(state).unwrap();
    (worker, room, plain, lead, dir)
}

fn alerts(worker: &Worker, room: RoomId) -> Vec<crate::messaging::model::Prompt> {
    worker
        .state
        .requests()
        .filter(|r| r.room_id == room)
        .map(|r| r.prompt.clone())
        .chain(worker.state.notices(room).cloned())
        .filter(|p| p.text.contains("Disk low:") || p.text.contains("URGENT: disk almost full"))
        .collect()
}

fn sample(worker: &mut Worker, bytes: u64) {
    let mut state = worker.state.clone();
    if state.notify_disk_space(bytes, 20, 5, io::now_ms()).unwrap() {
        worker.save(state).unwrap();
    }
}

fn cleanup(worker: Worker, dir: PathBuf) {
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn disk_crossings_post_exact_bus_notices_in_every_work_room_with_the_right_recipient() {
    let (mut worker, room, plain, lead, dir) = setup();
    sample(&mut worker, 20 * GB);
    assert!(alerts(&worker, room).is_empty());
    sample(&mut worker, 19 * GB + GB / 2);
    for id in [room, plain] {
        let posted = alerts(&worker, id);
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].author, Author::Bus);
        assert_eq!(posted[0].text, "Disk low: 19 GB free. Delete finished worktrees and cargo target dirs before starting new builds.");
        if id == room {
            assert_eq!(posted[0].recipient_ids, [lead].into());
        } else {
            assert!(posted[0].recipient_ids.is_empty(), "developer fallback");
        }
    }
    sample(&mut worker, 5 * GB);
    assert_eq!(alerts(&worker, room).len(), 1, "equal is not below");
    sample(&mut worker, 4 * GB);
    for id in [room, plain] {
        let posted = alerts(&worker, id);
        assert_eq!(posted.len(), 2);
        let critical = posted
            .iter()
            .find(|p| p.text.starts_with("URGENT:"))
            .unwrap();
        assert_eq!(critical.author, Author::Bus);
        assert_eq!(critical.text, "URGENT: disk almost full (4 GB free). Stop starting builds now and free space immediately: delete merged worktrees and target dirs.");
    }
    assert!(alerts(&worker, worker.state.master_room().unwrap().id).is_empty());
    cleanup(worker, dir);
}

#[test]
fn disk_warning_rearms_only_more_than_one_gb_above_its_threshold() {
    let (mut worker, room, _, _, dir) = setup();
    sample(&mut worker, 19 * GB);
    for recovery in [20 * GB, 20 * GB + 1, 20 * GB + GB / 2, 21 * GB] {
        sample(&mut worker, recovery);
        sample(&mut worker, 19 * GB);
        assert_eq!(
            alerts(&worker, room).len(),
            1,
            "recovery {recovery} must not rearm warning"
        );
    }
    sample(&mut worker, 21 * GB + 1);
    sample(&mut worker, 19 * GB);
    assert_eq!(
        alerts(&worker, room).len(),
        2,
        "one byte beyond the deadband rearms warning"
    );
    cleanup(worker, dir);
}

#[test]
fn disk_critical_rearms_only_more_than_one_gb_above_its_threshold() {
    let (mut worker, room, _, _, dir) = setup();
    sample(&mut worker, 19 * GB);
    sample(&mut worker, 4 * GB);
    for recovery in [5 * GB, 5 * GB + 1, 5 * GB + GB / 2, 6 * GB] {
        sample(&mut worker, recovery);
        sample(&mut worker, 4 * GB);
        assert_eq!(
            alerts(&worker, room).len(),
            2,
            "recovery {recovery} must not rearm critical"
        );
    }
    sample(&mut worker, 6 * GB + 1);
    sample(&mut worker, 4 * GB);
    let posted = alerts(&worker, room);
    assert_eq!(posted.len(), 3, "only critical rearmed");
    assert_eq!(
        posted
            .iter()
            .filter(|p| p.text.starts_with("Disk low:"))
            .count(),
        1
    );
    cleanup(worker, dir);
}

#[test]
fn disk_direct_drop_posts_only_critical_and_persists_both_latches_and_rearming() {
    let (mut worker, room, plain, lead, dir) = setup();
    sample(&mut worker, 4 * GB);
    for id in [room, plain] {
        let posted = alerts(&worker, id);
        assert_eq!(posted.len(), 1, "a direct drop posts only critical");
        assert!(posted[0].text.starts_with("URGENT:"));
        assert_eq!(posted[0].author, Author::Bus);
        assert_eq!(
            posted[0].recipient_ids,
            if id == room {
                [lead].into()
            } else {
                Default::default()
            }
        );
    }
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    sample(&mut worker, 4 * GB);
    assert_eq!(
        alerts(&worker, room).len(),
        1,
        "critical stays sent after restart"
    );
    for bytes in [19 * GB, 4 * GB, 21 * GB, 4 * GB] {
        sample(&mut worker, bytes);
    }
    let posted = alerts(&worker, room);
    assert_eq!(
        posted.len(),
        3,
        "critical rearmed at 19 and 21 GB; warning stayed latched"
    );
    assert!(posted.iter().all(|p| p.text.starts_with("URGENT:")));
    sample(&mut worker, 21 * GB + 1);
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(NoAgents)).unwrap();
    sample(&mut worker, 19 * GB);
    sample(&mut worker, 4 * GB);
    assert_eq!(
        alerts(&worker, room).len(),
        5,
        "both rearmed latches were saved too"
    );
    cleanup(worker, dir);
}

#[test]
fn disk_alerts_skip_deleting_rooms_and_fall_back_from_a_deleting_orchestrator() {
    let (mut worker, room, plain, lead, dir) = setup();
    let mut state = worker.state.clone();
    state.prepare_delete_room(plain).unwrap();
    state.prepare_delete_agent(lead).unwrap();
    worker.save(state).unwrap();
    sample(&mut worker, 4 * GB);
    assert!(alerts(&worker, plain).is_empty());
    let posted = alerts(&worker, room);
    assert_eq!(posted.len(), 1);
    assert!(posted.iter().all(|p| p.recipient_ids.is_empty()));
    cleanup(worker, dir);
}

#[test]
fn disk_settings_commands_save_publish_and_reject_invalid_pairs_without_changing_the_file() {
    let (mut worker, _, _, _, dir) = setup();
    let (events, receiver) = mpsc::channel();
    for command in [
        BusCommand::SetDiskWarningGb(30),
        BusCommand::SetDiskCriticalGb(10),
    ] {
        worker.command(command, &events).unwrap();
    }
    let changed: Vec<_> = receiver
        .try_iter()
        .filter_map(|event| match event {
            BusEvent::SettingsChanged(s) => Some((s.disk_warning_gb, s.disk_critical_gb)),
            _ => None,
        })
        .collect();
    assert_eq!(changed, [(30, 5), (30, 10)]);
    let path = dir.join("settings.json");
    let saved = std::fs::read(&path).unwrap();
    for command in [
        BusCommand::SetDiskWarningGb(10),
        BusCommand::SetDiskCriticalGb(30),
        BusCommand::SetDiskWarningGb(10_001),
        BusCommand::SetDiskCriticalGb(0),
    ] {
        assert!(worker.command(command, &events).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), saved);
    }
    cleanup(worker, dir);
}
