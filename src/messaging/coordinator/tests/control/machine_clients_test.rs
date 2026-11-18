use super::*;

#[test]
fn machine_client_commands_keep_events_and_navigation_private() {
    let (mut worker, room, _, dir) = fixture();
    worker.dev_enabled = false;
    let before = worker.state.visible_room();
    let response = call(
        &mut worker,
        "view-a-1",
        "client.command",
        json!({
            "client_id": "view-a", "command_id": 1,
            "command": {"CreateRoom":"from-viewer"}
        }),
    );
    assert!(response.ok, "{response:?}");
    assert!(response.result["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event.get("RoomCreated").is_some()));
    let other = call(
        &mut worker,
        "view-b-poll",
        "client.snapshot",
        json!({"client_id":"view-b"}),
    );
    assert!(other.ok, "{other:?}");
    assert_eq!(other.result["events"], json!([]));
    assert_eq!(worker.state.visible_room(), before);
    let count = worker.state.rooms().count();
    let replay = call(
        &mut worker,
        "view-a-1",
        "client.command",
        json!({
            "client_id":"view-a", "command_id":1, "command":{"CreateRoom":"from-viewer"}
        }),
    );
    assert!(replay.ok);
    assert_eq!(worker.state.rooms().count(), count);
    let bad = call(
        &mut worker,
        "view-a-draft",
        "client.command",
        json!({
            "client_id":"view-a", "command_id":2,
            "command":{"SetDraftText":[room,"private"]}
        }),
    );
    assert!(!bad.ok);
    assert_eq!(worker.state.room(room).unwrap().draft.text, "");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_worker_can_address_a_master_agent_without_a_role_gate() {
    let (mut worker, room, _, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let target = worker
        .state
        .create_agent(
            master,
            "remote-recipient",
            Provider::Codex,
            dir.clone(),
            None,
        )
        .unwrap();
    let result = call(
        &mut worker,
        "remote-send",
        "message.send",
        json!({
            "room":room.0.to_string(), "to":[target.0.to_string()], "text":"hello",
            "external_author":{"machine":"laptop","name":"worker","agent_id":7}
        }),
    );
    assert!(result.ok, "{result:?}");
    let id = RequestId(result.result["request_ids"][0].as_u64().unwrap());
    assert!(matches!(
        worker.state.request(id).unwrap().prompt.author,
        Author::ExternalAgent { .. }
    ));
    let payload = worker.state.render_request_payload(id).unwrap();
    assert!(payload.contains("worker@laptop"), "{payload}");
    assert!(!payload.contains("from you"), "{payload}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_master_posts_keep_existing_room_rule() {
    let (mut worker, _, _, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let target = worker
        .state
        .create_agent(master, "orchestrator", Provider::Codex, dir.clone(), None)
        .unwrap();
    let params = json!({"room":master.0.to_string(),"to":[target.0.to_string()],"text":"hello",
        "external_author":{"machine":"remote","name":"worker","agent_id":7,"home_room":2,"home_is_master":false}});
    let denied = call(
        &mut worker,
        "remote-worker-master",
        "message.send",
        params.clone(),
    );
    assert!(!denied.ok);
    assert_eq!(denied.error.unwrap().code, "master_reserved");
    let mut params = params;
    params["external_author"]["home_is_master"] = json!(true);
    let allowed = call(&mut worker, "remote-master-master", "message.send", params);
    assert!(allowed.ok, "{allowed:?}");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exact_remote_submission_does_not_overwrite_another_clients_draft() {
    let (mut worker, room, agent, dir) = fixture();
    worker
        .state
        .set_draft_text(room, "another client's draft")
        .unwrap();
    let response = call(
        &mut worker,
        "private-send",
        "client.command",
        json!({
            "client_id":"view-a", "command_id":4,
            "command":{"Submit":[room,{"text":"this client's message","files":[],"recipient_ids":[agent]}]}
        }),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(
        worker.state.room(room).unwrap().draft.text,
        "another client's draft"
    );
    assert_eq!(
        response.result["snapshot"]["state"]["rooms"][room.0.to_string()]["draft"]["text"],
        ""
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_attachment_is_validated_on_the_receiving_coordinator() {
    let (mut worker, room, agent, dir) = fixture();
    let missing = dir.join("attachment-not-present-here.png");
    let before = worker.state.clone();
    let response = call(
        &mut worker,
        "missing-attachment",
        "client.command",
        json!({
            "client_id":"viewer", "command_id":1,
            "command":{"Submit":[room,{"text":"test","files":[missing],"recipient_ids":[agent]}]}
        }),
    );
    assert!(response.ok);
    assert!(response.result["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["CommandFinished"]["result"].get("Err").is_some()));
    assert_eq!(worker.state, before);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn connected_viewer_routes_worker_between_two_buses_without_target_lists() {
    use crate::messaging::machine_routes::{self, Advertisement, Completion};
    use std::sync::{atomic::AtomicBool, Arc};
    let (mut source, source_room, source_agent, source_dir) = fixture();
    let (mut destination, _, _, destination_dir) = fixture();
    let receiving_room = destination.state.create_room("receiving-room").unwrap();
    let master = destination.state.master_room().unwrap().id;
    let recipient = destination
        .state
        .create_agent(
            master,
            "master-recipient",
            Provider::Codex,
            destination_dir.clone(),
            None,
        )
        .unwrap();
    let sender = call(
        &mut source,
        "resolve-source",
        "message.sender",
        json!({"as":source_agent.0.to_string()}),
    );
    assert!(sender.ok);
    assert_eq!(sender.result["external_author"]["room_id"], source_room.0);
    assert_eq!(
        sender.result["external_author"]["machine_id"]
            .as_str()
            .unwrap()
            .len(),
        32
    );
    let label = format!("C-{}", crate::utils::time::now_ns());
    let viewer = format!("viewer-{label}");
    let advert = Advertisement {
        label: label.clone(),
        identity: machine_routes::machine_identity(&destination_dir).unwrap(),
    };
    let (_lease, jobs) = machine_routes::register(label.clone(), Arc::new(AtomicBool::new(true)));
    machine_routes::identify_route(&label, &advert.identity);
    machine_routes::exchange(&viewer, vec![advert.clone()], vec![]);
    let inner = ControlRequest {
        id: format!("send-{label}"),
        method: "message.send".into(),
        params: json!({
            "room":receiving_room.0.to_string(),"to":[recipient.0.to_string()],"text":"cross two existing viewer links",
            "external_author":sender.result["external_author"],
        }),
    };
    let outer = ControlRequest {
        id: format!("forward-{label}"),
        method: "machine.forward".into(),
        params: json!({"machine":label,"request":inner}),
    };
    let receipt = machine_routes::forward(&outer).unwrap();
    let (completed, completion_rx) = std::sync::mpsc::channel();
    for job in machine_routes::exchange(&viewer, vec![advert.clone()], vec![]) {
        machine_routes::dispatch(job, &completed);
    }
    let routed = jobs.recv_timeout(Duration::from_secs(1)).unwrap();
    let response = destination.control_response(&routed.job.request);
    assert!(response.ok, "{response:?}");
    let request_id = RequestId(response.result["request_ids"][0].as_u64().unwrap());
    let request = destination.state.request(request_id).unwrap();
    assert_eq!(request.room_id, receiving_room);
    assert!(
        matches!(&request.prompt.author, Author::ExternalAgent {room_id:Some(room),..} if *room==source_room.0)
    );
    routed
        .complete
        .send(Completion {
            id: routed.job.id,
            response,
        })
        .unwrap();
    machine_routes::exchange(
        &viewer,
        vec![advert],
        vec![completion_rx.recv_timeout(Duration::from_secs(1)).unwrap()],
    );
    assert!(
        receipt.recv_timeout(Duration::from_secs(1)).unwrap().result["ok"]
            .as_bool()
            .unwrap()
    );
    assert!(machine_routes::forward(&outer).is_err());
    machine_routes::exchange(&viewer, vec![], vec![]);
    let mut missing = outer;
    missing.id.push_str("-missing");
    missing.params["request"]["id"] = json!(format!("next-{label}"));
    assert_eq!(
        machine_routes::forward(&missing)
            .unwrap_err()
            .error
            .unwrap()
            .code,
        "machine_unavailable"
    );
    assert_eq!(source.state.requests().count(), 0);
    assert_eq!(destination.state.requests().count(), 1);
    drop(source);
    drop(destination);
    std::fs::remove_dir_all(source_dir).unwrap();
    std::fs::remove_dir_all(destination_dir).unwrap();
}

#[test]
fn machine_service_reports_the_live_tui_build_independently() {
    let (mut worker, _, _, dir) = fixture();
    let live = crate::platform::process_instance(std::process::id()).unwrap();
    let binary = dir.join("independent-tui");
    crate::utils::restart_record::write_json(
        &crate::utils::restart_record::tui_build_path(&dir),
        &json!({
            "sha":"a".repeat(40),"binary":binary,"backup_binary":binary,
            "process":{"pid":live.pid,"birth":live.birth}
        }),
    )
    .unwrap();
    let response = call(&mut worker, "tui-build", "state", json!({}));
    assert_eq!(response.result["build"]["client"]["sha"], "a".repeat(40));
    assert_eq!(response.result["build"]["client"]["binary"], json!(binary));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
