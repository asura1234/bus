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
fn worker_to_orchestrator_broadcast_still_targets_members_only() {
    let (mut worker, room, _, dir) = fixture();
    let own = bound(&mut worker, room, &dir, "own-orch");
    let other_room = worker.state.create_room("other").unwrap();
    let _other = bound(&mut worker, other_room, &dir, "other-orch");
    let master = worker.state.master_room().unwrap().id;
    let _unbound = worker
        .state
        .create_agent(master, "unbound", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
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
fn worker_to_orchestrator_member_names_take_precedence_and_duplicates_are_rejected() {
    for name in ["orchestrator", "same-name"] {
        let (mut worker, room, _, dir) = fixture();
        bound(&mut worker, room, &dir, "bound-orch");
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
        assert!(worker
            .state
            .create_agent(room, name, Provider::Codex, dir.clone(), None)
            .is_err());
        assert_eq!(worker.state.requests().count(), 1);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn worker_to_orchestrator_alias_with_a_member_reaches_both_in_one_message() {
    let (mut worker, room, _, dir) = fixture();
    let orchestrator = bound(&mut worker, room, &dir, "own-orch");
    let peer = worker
        .state
        .create_agent(room, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    let sent = call(
        &mut worker,
        "mixed",
        "message.send",
        json!({"room":"test","as":"codex1","to":["peer","orchestrator","own-orch"],"text":"both"}),
    );
    assert!(sent.ok, "{sent:?}");
    let mut agents = worker
        .state
        .requests()
        .map(|r| r.agent_id)
        .collect::<Vec<_>>();
    agents.sort();
    let mut expected = vec![orchestrator, peer];
    expected.sort();
    assert_eq!(agents, expected);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn orchestrator_writing_as_itself_to_its_id_is_rejected_as_a_self_message() {
    let (mut worker, room, _, dir) = fixture();
    let orchestrator = bound(&mut worker, room, &dir, "own-orch");
    let sent = call(
        &mut worker,
        "self",
        "message.send",
        json!({"room":"test","as":orchestrator.0.to_string(),"to":[orchestrator.0.to_string()],"text":"me"}),
    );
    assert_eq!(
        error_message(&sent),
        "An agent cannot send a message to itself"
    );
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_to_orchestrator_being_deleted_is_refused_without_a_request() {
    let (mut worker, room, _, dir) = fixture();
    let orchestrator = bound(&mut worker, room, &dir, "own-orch");
    worker.state.prepare_delete_agent(orchestrator).unwrap();
    for selector in ["orchestrator", "own-orch"] {
        let sent = call(
            &mut worker,
            selector,
            "message.send",
            json!({"room":"test","as":"codex1","to":[selector],"text":"gone"}),
        );
        assert!(!sent.ok, "{sent:?}");
    }
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn worker_to_orchestrator_alias_ignores_case_like_the_human_alias() {
    let (mut worker, room, _, dir) = fixture();
    let orchestrator = bound(&mut worker, room, &dir, "own-orch");
    let sent = call(
        &mut worker,
        "case",
        "message.send",
        json!({"room":"test","as":"codex1","to":["Orchestrator"],"text":"hi"}),
    );
    assert!(sent.ok, "{sent:?}");
    assert_eq!(
        worker.state.requests().next().unwrap().agent_id,
        orchestrator
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
