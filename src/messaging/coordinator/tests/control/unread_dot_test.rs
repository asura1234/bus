use super::*;
use serde_json::Value;

fn unseen(worker: &mut Worker, room: RoomId) -> Value {
    let response = call(worker, "dot-state", "state", json!({}));
    assert!(response.ok, "{response:?}");
    response.result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(room))
        .unwrap()["unseen_to_developer"]
        .clone()
}

fn post(worker: &mut Worker, room: RoomId, author: AgentId, to: &str, key: &str) {
    let id = format!("{key}-{}", room.0);
    let response = call(
        worker,
        &id,
        "message.send",
        json!({"room":room.0.to_string(),"as":author.0.to_string(),"to":[to],"text":key}),
    );
    assert!(response.ok, "{response:?}");
}

fn orchestrator(worker: &mut Worker, room: RoomId, dir: &std::path::Path) -> AgentId {
    let master = worker.state.master_room().unwrap().id;
    let id = worker
        .state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.into(), None)
        .unwrap();
    worker.state.bind_orchestrator(id, room).unwrap();
    id
}

#[test]
fn unread_dot_tracks_only_developer_recipients_and_clears_on_open() {
    let (mut worker, room, author, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let orch = orchestrator(&mut worker, room, &dir);
    let peer = worker
        .state
        .create_agent(room, "peer", Provider::Codex, dir.clone(), None)
        .unwrap();
    for (target, author, recipient) in [(room, author, "you"), (master, orch, "human")] {
        worker
            .state
            .select_room(if target == room { master } else { room })
            .unwrap();
        post(&mut worker, target, author, recipient, "dot-post");
        assert_eq!(unseen(&mut worker, target), json!(true));
        assert_eq!(worker.state.room(target).unwrap().unread_count, 1);
        let (events, _) = mpsc::channel();
        worker
            .command(BusCommand::SelectRoom(target), &events)
            .unwrap();
        assert_eq!(unseen(&mut worker, target), json!(false));
        assert_eq!(worker.state.room(target).unwrap().unread_count, 0);
        post(&mut worker, target, author, recipient, "dot-visible");
        assert_eq!(unseen(&mut worker, target), json!(false));
    }
    worker.state.leave_room_view();
    post(&mut worker, room, author, &peer.0.to_string(), "dot-peer");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    assert_eq!(unseen(&mut worker, room), json!(false));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unread_dot_survives_server_reopen_and_seen_state_stays_cleared() {
    let (mut worker, room, author, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let orch = orchestrator(&mut worker, room, &dir);
    post(&mut worker, room, author, "human", "dot-work-restart");
    post(&mut worker, master, orch, "you", "dot-master-restart");
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    for target in [master, room] {
        assert_eq!(unseen(&mut worker, target), json!(true));
    }
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::SelectRoom(room), &events)
        .unwrap();
    drop(worker);
    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    assert_eq!(unseen(&mut worker, room), json!(false));
    assert_eq!(unseen(&mut worker, master), json!(true));
    assert_eq!(worker.state.visible_room(), None);
    post(
        &mut worker,
        room,
        author,
        "you",
        "dot-after-restart-no-client",
    );
    assert_eq!(unseen(&mut worker, room), json!(true));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unread_dot_defaults_to_clear_for_legacy_rooms_without_reclassifying_unread() {
    let (mut worker, room, _, dir) = fixture();
    let mut legacy = serde_json::to_value(&worker.state).unwrap();
    let saved_room = &mut legacy["rooms"][room.0.to_string()];
    saved_room
        .as_object_mut()
        .unwrap()
        .remove("unseen_to_developer");
    saved_room["unread_count"] = json!(7);
    worker.state = serde_json::from_value(legacy).unwrap();
    assert_eq!(unseen(&mut worker, room), json!(false));
    assert_eq!(worker.state.room(room).unwrap().unread_count, 7);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
