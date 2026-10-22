use super::*;

fn bound(worker: &mut Worker, room: RoomId, dir: &std::path::Path, name: &str) -> AgentId {
    let master = worker.state.master_room().unwrap().id;
    let id = worker
        .state
        .create_agent(master, name, Provider::ClaudeCode, dir.into(), None)
        .unwrap();
    worker.state.bind_orchestrator(id, room).unwrap();
    id
}

fn assert_resolution(selector: impl FnOnce(AgentId) -> String) {
    let (mut worker, room, author, dir) = fixture();
    let orchestrator = bound(&mut worker, room, &dir, "libtv-app master");
    let sent = call(
        &mut worker,
        "worker-send",
        "message.send",
        json!({"room":room.0.to_string(),"as":author.0.to_string(),
            "to":[selector(orchestrator)],"text":"ready for verify","queue":true}),
    );
    assert!(sent.ok, "{sent:?}");
    let request = worker.state.requests().next().unwrap();
    assert_eq!(request.agent_id, orchestrator);
    assert_eq!(request.room_id, room);
    assert_eq!(request.prompt.author, Author::Agent(author));
    assert_eq!(request.phase, RequestPhase::Queued);
    assert!(request.queue_only);
    assert_eq!(
        worker.state.agent(orchestrator).unwrap().room_id,
        worker.state.master_room().unwrap().id
    );
    let history = call(
        &mut worker,
        "history",
        "room.history",
        json!({"room":"test"}),
    );
    assert_eq!(
        history.result["messages"][0]["prompt"]["text"],
        "ready for verify"
    );
    assert_eq!(
        history.result["messages"][0]["prompt"]["author"],
        json!({"agent":author})
    );
    assert_eq!(
        history.result["messages"][0]["prompt"]["recipient_ids"],
        json!([orchestrator])
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_to_orchestrator_by_name() {
    assert_resolution(|_| "libtv-app master".into());
}

#[test]
fn worker_to_orchestrator_by_id() {
    assert_resolution(|id| id.0.to_string());
}

#[test]
fn worker_to_orchestrator_by_alias() {
    assert_resolution(|_| "orchestrator".into());
}

#[test]
fn worker_to_orchestrator_without_binding_has_clear_error() {
    let (mut worker, _, _, dir) = fixture();
    let sent = call(
        &mut worker,
        "unbound",
        "message.send",
        json!({"room":"test","as":"codex1","to":["orchestrator"],"text":"blocked"}),
    );
    assert_eq!(error_message(&sent), "This room has no bound orchestrator");
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_to_orchestrator_never_exposes_other_master_agents_or_broadcasts() {
    let (mut worker, room, _, dir) = fixture();
    let own = bound(&mut worker, room, &dir, "own-orch");
    let other_room = worker.state.create_room("other").unwrap();
    let other = bound(&mut worker, other_room, &dir, "other-orch");
    let master = worker.state.master_room().unwrap().id;
    let unbound = worker
        .state
        .create_agent(master, "unbound", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    for selector in [
        "other-orch".into(),
        other.0.to_string(),
        "unbound".into(),
        unbound.0.to_string(),
    ] {
        let sent = call(
            &mut worker,
            &selector,
            "message.send",
            json!({"room":"test","as":"codex1","to":[selector],"text":"blocked"}),
        );
        assert_eq!(
            error_message(&sent),
            "No matching room or agent",
            "{sent:?}"
        );
    }
    let peer = worker
        .state
        .create_agent(room, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let sent = call(
        &mut worker,
        "broadcast",
        "message.send",
        json!({"room":"test","as":"codex1","to":["all"],"text":"hello"}),
    );
    assert!(sent.ok, "{sent:?}");
    assert_eq!(
        worker
            .state
            .requests()
            .map(|r| r.agent_id)
            .collect::<Vec<_>>(),
        [peer]
    );
    assert_ne!(peer, own);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_to_orchestrator_member_names_and_ambiguity_take_precedence() {
    for name in ["orchestrator", "same-name"] {
        let (mut worker, room, _, dir) = fixture();
        bound(&mut worker, room, &dir, name);
        let member = worker
            .state
            .create_agent(room, name, Provider::Codex, dir.clone(), None)
            .unwrap();
        let sent = call(
            &mut worker,
            "clash",
            "message.send",
            json!({"room":"test","as":"codex1","to":[name],"text":"hi"}),
        );
        assert!(sent.ok, "{sent:?}");
        assert_eq!(worker.state.requests().next().unwrap().agent_id, member);
        worker
            .state
            .create_agent(room, name, Provider::Codex, dir.clone(), None)
            .unwrap();
        let ambiguous = call(
            &mut worker,
            "ambiguous",
            "message.send",
            json!({"room":"test","as":"codex1","to":[name],"text":"hi"}),
        );
        assert!(
            error_message(&ambiguous).contains("Ambiguous"),
            "{ambiguous:?}"
        );
        assert_eq!(worker.state.requests().count(), 1);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
