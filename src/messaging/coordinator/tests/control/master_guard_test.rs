use super::*;

fn add(worker: &mut Worker, room: RoomId, dir: &std::path::Path, name: &str) -> AgentId {
    worker
        .state
        .create_agent(room, name, Provider::ClaudeCode, dir.into(), None)
        .unwrap()
}

fn refuse_worker(room_selector: &str, recipient: &str) {
    let (mut worker, home, sender, dir) = fixture();
    worker.state.rename_room(home, "app room name").unwrap();
    let master = worker.state.master_room().unwrap().id;
    let orch = add(&mut worker, master, &dir, "orch");
    worker.state.bind_orchestrator(orch, home).unwrap();
    worker.save(worker.state.clone()).unwrap();
    let before = worker.state.clone();
    let sent = call(
        &mut worker,
        "master-refused",
        "message.send",
        json!({"room":room_selector,"as":sender.0.to_string(),"to":[recipient],"text":"worker progress"}),
    );
    // Check the whole state and persisted journal before the error assertions:
    // even a refusal with the right wording must create no history or delivery.
    assert_eq!(worker.state, before, "refused send changed domain state");
    assert_eq!(worker.store.load().unwrap().unwrap(), before);
    worker.submit_ready().unwrap(); // NoTransport panics if anything is delivered.
    let history = call(
        &mut worker,
        "master-history",
        "room.history",
        json!({"room":"master"}),
    );
    assert_eq!(history.result["messages"], json!([]));
    assert!(!sent.ok, "{sent:?}");
    let error = sent.error.unwrap();
    assert_eq!(error.code, "master_reserved");
    assert_eq!(
        error.message,
        format!("MASTER (room 0) is reserved for orchestrators and the developer. Send from your own room instead: --room {} (app room name).", home.0)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn master_guard_worker_to_room_zero_is_refused_without_history_or_delivery() {
    refuse_worker("0", "orch");
}

#[test]
fn master_guard_worker_to_master_alias_is_refused_without_history_or_delivery() {
    for selector in ["master", "MASTER", "MaStEr"] {
        refuse_worker(selector, "orch");
    }
}

#[test]
fn master_guard_worker_to_developer_is_also_refused() {
    refuse_worker("master", "you");
}

#[test]
fn master_guard_bound_orchestrator_and_developer_can_send_in_master() {
    let (mut worker, home, recipient, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let orch = add(&mut worker, master, &dir, "orch");
    worker.state.bind_orchestrator(orch, home).unwrap();
    for (id, author) in [("orch", Some(orch)), ("developer", None)] {
        let mut params = json!({"room":"master","to":[recipient.0.to_string()],"text":"question"});
        if let Some(author) = author {
            params["as"] = json!(author.0.to_string());
        }
        let sent = call(&mut worker, id, "message.send", params);
        assert!(sent.ok, "{sent:?}");
        let request = worker.state.requests().last().unwrap();
        assert_eq!(request.room_id, master);
        assert_eq!(
            request.prompt.author,
            author.map_or(Author::Developer, Author::Agent)
        );
    }
    let sent = call(
        &mut worker,
        "orch-to-developer",
        "message.send",
        json!({"room":"master","as":orch.0.to_string(),"to":["you"],"text":"release decision"}),
    );
    assert!(sent.ok, "{sent:?}");
    assert_eq!(sent.result["stage"], "posted");
    worker
        .state
        .post_bus_notice(master, None, "Bus notice".into(), 1)
        .unwrap();
    assert_eq!(
        worker.state.notices(master).last().unwrap().author,
        Author::Bus
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn master_guard_master_residence_alone_does_not_make_an_orchestrator() {
    let (mut worker, _, recipient, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let unbound = add(&mut worker, master, &dir, "unbound");
    let sent = call(
        &mut worker,
        "unbound-master",
        "message.send",
        json!({"room":"master","as":unbound.0.to_string(),"to":[recipient.0.to_string()],"text":"new message"}),
    );
    assert!(!sent.ok, "{sent:?}");
    assert_eq!(sent.error.unwrap().code, "master_reserved");
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn master_guard_worker_can_send_in_own_and_other_work_rooms() {
    let (mut worker, home, sender, dir) = fixture();
    let other = worker.state.create_room("other").unwrap();
    let peer = add(&mut worker, other, &dir, "peer");
    for room in [home, other] {
        let sent = call(
            &mut worker,
            &format!("work-{}", room.0),
            "message.send",
            json!({"room":room.0.to_string(),"as":sender.0.to_string(),"to":[peer.0.to_string()],"text":"peer question"}),
        );
        assert!(sent.ok, "{sent:?}");
        let request = worker.state.requests().last().unwrap();
        assert_eq!(request.room_id, room);
        assert_eq!(request.agent_id, peer);
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
