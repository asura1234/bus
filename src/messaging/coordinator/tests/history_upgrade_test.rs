use super::*;

fn incident_cohort(history: &str, cutoff: u64) -> Vec<Request> {
    history
        .lines()
        .filter_map(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap()
                .get("request")
                .cloned()
        })
        .map(|v| serde_json::from_value::<Request>(v).unwrap())
        .filter(|r| {
            r.phase == RequestPhase::Completed
                && matches!(r.prompt.author, Author::Agent(_))
                // This fixed incident archive predates canonical completed_reply.
                && r.pending_final.is_some()
                && r.completed_at_ms.is_some_and(|at| at < cutoff)
        })
        .collect()
}

#[test]
fn incident_cohort_selects_the_old_archives_saved_result_field() {
    let f = Fixture::new();
    let mut state = f.worker.state.clone();
    let mut archive = String::new();
    let mut expected = None;
    for (author, completed_at) in [
        (Author::Agent(f.orch), 10),
        (Author::Developer, 10),
        (Author::Agent(f.orch), 30),
    ] {
        let id = state
            .submit_message_from(
                f.room,
                Draft {
                    text: "incident work".into(),
                    files: vec![],
                    recipient_ids: [f.agent].into(),
                },
                author.clone(),
                1,
            )
            .unwrap()[0];
        state.finish_for_test(id, "archived answer", completed_at);
        let mut saved = serde_json::to_value(state.request(id).unwrap()).unwrap();
        saved["pending_final"] = saved["completed_reply"]["content"].take();
        saved.as_object_mut().unwrap().remove("completed_reply");
        archive.push_str(&json!({"request":saved}).to_string());
        archive.push('\n');
        if author == Author::Agent(f.orch) && completed_at == 10 {
            expected = Some(id);
        }
    }
    let cohort = incident_cohort(&archive, 20);
    assert_eq!(
        cohort.iter().map(|r| r.id).collect::<Vec<_>>(),
        [expected.unwrap()]
    );
    assert_eq!(
        cohort[0].pending_final.as_ref().unwrap().text,
        "archived answer"
    );
}

#[test]
fn routing_floor_blocks_inline_event_backfill_but_keeps_recent_and_open_work() {
    let f = Fixture::new();
    let now = io::now_ms();
    let floor = now - 7 * 24 * 60 * 60_000;
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, floor - 20_000)
        .unwrap();
    let peer = state
        .create_agent(f.room, "peer", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    let mut ids = Vec::new();
    for completed_at in [Some(floor - 1), Some(floor), None] {
        let id = state
            .submit_message_from(
                f.room,
                Draft {
                    text: "peer work".into(),
                    files: vec![],
                    recipient_ids: [f.agent].into(),
                },
                Author::Agent(peer),
                floor - 10_000,
            )
            .unwrap()[0];
        if let Some(at) = completed_at {
            state.finish_for_test(id, "peer verdict", at);
        }
        ids.push(id);
    }
    // Keep every source inline: the general routing live-source exception must
    // not authorize reconstructing expired events from it.
    let mut saved = serde_json::to_value(&state).unwrap();
    saved["routing_before_ms"] = json!(floor);
    let mut state: BusState = serde_json::from_value(saved).unwrap();
    state.retain_worker_outcomes(now);
    let events = serde_json::to_value(&state).unwrap()["worker_events"].clone();
    // Peer finals belong to the peer: none are copied to the orchestrator.
    assert!(events.get(format!("turn-final:{}", ids[0].0)).is_none());
    assert!(events.get(format!("turn-final:{}", ids[1].0)).is_none());
    assert_eq!(
        state.request(ids[0]).unwrap().reply().unwrap().text,
        "peer verdict"
    );
}

#[test]
fn legacy_formats_initialize_the_floor_before_any_worker_pass_and_never_lower_it() {
    for version in [1, 2] {
        for existing_floor in [0, io::now_ms() + 60_000] {
            let f = Fixture::new();
            let path = f.dir.join("state.json");
            let mut saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            saved["version"] = json!(version);
            saved["state"]["routing_before_ms"] = json!(existing_floor);
            std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
            let before = io::now_ms() - 7 * 24 * 60 * 60_000;
            let loaded = JsonStore::new(path).load().unwrap().unwrap();
            let floor = serde_json::to_value(&loaded).unwrap()["routing_before_ms"]
                .as_u64()
                .unwrap();
            assert!(floor >= before && floor >= existing_floor);
            if existing_floor != 0 {
                assert_eq!(floor, existing_floor);
            }
        }
    }
}

#[test]
#[ignore = "requires private incident copy via DELETE_PURGE_SNAPSHOT; run explicitly"]
fn real_format_2_snapshot_first_start_preserves_archive_without_old_wakes() {
    let source = PathBuf::from(
        std::env::var_os("DELETE_PURGE_SNAPSHOT")
            .expect("set DELETE_PURGE_SNAPSHOT to the read-only 17:29 snapshot"),
    );
    let dir = crate::utils::test_temp::unique_temp_path("bus-history-upgrade");
    std::fs::create_dir(&dir).unwrap();
    for name in ["state.json", "history.jsonl"] {
        std::fs::copy(source.join(name), dir.join(name)).unwrap();
    }
    let history = std::fs::read(dir.join("history.jsonl")).unwrap();
    let cutoff = io::now_ms() - 7 * 24 * 60 * 60_000;
    // The incident's 22-record cohort is fixed at capture time, rather than
    // growing as newer rows age. The loaded routing floor below still uses now.
    // Captured at 2026-10-23T17:29:16+08:00.
    let incident_cutoff = 1_792_747_756_000_u64 - 7 * 24 * 60 * 60_000;
    let old = incident_cohort(std::str::from_utf8(&history).unwrap(), incident_cutoff);
    assert_eq!(old.len(), 22, "verify the actual incident fixture");
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut worker = Worker::open(dir.clone(), Box::new(Accepting(calls.clone()))).unwrap();
    assert!(!worker.store.compact_history().unwrap());
    assert!(
        serde_json::to_value(&worker.state).unwrap()["routing_before_ms"]
            .as_u64()
            .unwrap()
            >= cutoff
    );
    assert_eq!(std::fs::read(dir.join("history.jsonl")).unwrap(), history);
    // The first tick's expiry pass precedes archive_when_due. Do not advance
    // archiving here: the fixture also has recent inline work that legitimately
    // appends new records. No provider or live directory is contacted.
    worker.expire_queued_now().unwrap();
    let events = serde_json::to_value(&worker.state).unwrap()["worker_events"].clone();
    for request in old {
        let group = request.group.unwrap_or(request.id);
        assert!(events.get(format!("turn-final:{}", group.0)).is_none());
        assert_eq!(worker.state.request(request.id).unwrap(), &request);
    }
    assert!(
        calls.lock().unwrap().is_empty(),
        "first start must emit zero native wakes"
    );
    assert_eq!(std::fs::read(dir.join("history.jsonl")).unwrap(), history);
    assert_eq!(
        std::fs::read(source.join("history.jsonl")).unwrap(),
        history
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn legacy_idle_alert_latch_loads_and_drops_without_losing_other_room_state() {
    let f = Fixture::new();
    let path = f.dir.join("state.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["state"]["rooms"][f.room.0.to_string()]["idle_alert_sent"] = json!(true);
    saved["state"]["rooms"][f.room.0.to_string()]["notes"] = json!("preserve these notes");
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let store = JsonStore::new(path.clone());
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.room(f.room).unwrap().notes, "preserve these notes");
    assert_eq!(loaded.agent(f.agent).unwrap().name, "builder");
    store.save(&loaded).unwrap();
    let rewritten: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(rewritten["state"]["rooms"][f.room.0.to_string()]
        .get("idle_alert_sent")
        .is_none());
    assert_eq!(store.load().unwrap().unwrap(), loaded);
}
