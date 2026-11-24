use super::*;

#[test]
fn bot_identity_survives_restart_and_keeps_message_attribution_after_delete() {
    let (mut worker, room, agent, dir) = fixture();
    worker.dev_enabled = false;
    let added = call(
        &mut worker,
        "add-bot",
        "bot.add",
        json!({"room":"test","name":" inbox-watcher "}),
    );
    assert!(added.ok, "{added:?}");
    drop(worker);

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    let bots = call(&mut worker, "list-bots", "bot.list", json!({}));
    assert!(bots.ok, "{bots:?}");
    assert_eq!(
        bots.result["bots"],
        json!([{"name":"inbox-watcher","room_id":room}]),
    );
    let sent = call(
        &mut worker,
        "bot-send",
        "message.send",
        json!({"room":"test","as":"inbox-watcher","to":["codex1"],"text":"Three new reports"}),
    );
    assert!(sent.ok, "{sent:?}");
    let id = sent.result["message_id"].as_u64().unwrap();
    let request = RequestId(sent.result["request_ids"][0].as_u64().unwrap());
    assert_eq!(worker.state.request(request).unwrap().agent_id, agent);

    let deleted = call(
        &mut worker,
        "delete-bot",
        "bot.delete",
        json!({"name":"inbox-watcher"}),
    );
    assert!(deleted.ok, "{deleted:?}");
    drop(worker);

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    let bots = call(&mut worker, "list-deleted", "bot.list", json!({}));
    assert_eq!(bots.result["bots"], json!([]));
    let sent_again = call(
        &mut worker,
        "bot-send-after-delete",
        "message.send",
        json!({"room":"test","as":"inbox-watcher","to":["codex1"],"text":"Removed"}),
    );
    assert!(!sent_again.ok, "{sent_again:?}");
    let history = call(
        &mut worker,
        "history",
        "room.history",
        json!({"room":"test"}),
    );
    assert!(history.ok, "{history:?}");
    let row = &history.result["messages"][0];
    assert_eq!(row["author_name"], "inbox-watcher");
    assert_eq!(row["prompt"]["author"], json!({"bot":"inbox-watcher"}));
    let status = call(
        &mut worker,
        "status",
        "message.status",
        json!({"message":id.to_string()}),
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.result["author_name"], "inbox-watcher");
    assert_eq!(status.result["prompt"], row["prompt"]);
    let delivered = worker.state.render_request_payload(request).unwrap();
    assert_eq!(
        delivered,
        format!(
            "inbox-watcher:\n[Bus message {id} from inbox-watcher (room {})]\nThree new reports",
            room.0
        ),
    );
    let mut state = worker.state.clone();
    let finished_at = crate::messaging::storage::io::now_ms();
    state.finish_for_test(request, "Noted", finished_at);
    assert!(state.archive(finished_at + crate::messaging::model::ARCHIVE_AFTER_MS + 1));
    worker.save(state).unwrap();
    drop(worker);

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    assert!(worker.state.history().items().any(|item| {
        matches!(item, crate::messaging::model::Archived::Request(row) if row.id == request)
    }));
    let archived = call(
        &mut worker,
        "archived-status",
        "message.status",
        json!({"message":id.to_string()}),
    );
    assert!(archived.ok, "{archived:?}");
    assert_eq!(archived.result["author_name"], "inbox-watcher");
    assert_eq!(archived.result["prompt"], row["prompt"]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bot_identity_cannot_receive_or_enter_the_agent_lifecycle() {
    let (mut worker, room, agent, dir) = fixture();
    let master = worker.state.ensure_master_room();
    let orchestrator = worker
        .state
        .create_agent(master, "orch", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let added = call(
        &mut worker,
        "add-bot",
        "bot.add",
        json!({"room":"test","name":"inbox-watcher"}),
    );
    assert!(added.ok, "{added:?}");

    let before = worker.state.clone();
    let refused = call(
        &mut worker,
        "bot-target",
        "message.send",
        json!({"room":"test","to":["codex1","inbox-watcher"],"text":"Do not queue"}),
    );
    assert!(!refused.ok, "{refused:?}");
    let error = error_message(&refused);
    assert!(
        error.contains("inbox-watcher") && error.contains("cannot receive"),
        "{error}"
    );
    assert_eq!(worker.state, before);

    let state = call(&mut worker, "state", "state", json!({}));
    let bots = state.result["bots"].as_array().unwrap();
    assert_eq!(bots, &[json!({"name":"inbox-watcher","room_id":room})]);
    let agents = state.result["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2);
    assert!(agents.iter().all(|agent| agent["queue_depth"] == 0));
    let mut alerted = BTreeSet::new();
    assert!(!worker
        .state
        .notify_idle_agents(0, 1, 5 * 60_000, &mut alerted)
        .unwrap());
    assert!(alerted.is_empty());

    let broadcast = call(
        &mut worker,
        "bot-broadcast",
        "message.send",
        json!({"room":"test","as":"inbox-watcher","to":["all"],"text":"One real recipient"}),
    );
    assert!(broadcast.ok, "{broadcast:?}");
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, agent);

    let alias_bot = call(&mut worker, "alias-bot", "bot.add", json!({"name":"all"}));
    assert!(alias_bot.ok, "{alias_bot:?}");
    let before = worker.state.clone();
    let refused = call(
        &mut worker,
        "alias-target",
        "message.send",
        json!({"room":"test","to":["all"],"text":"Do not broadcast"}),
    );
    assert!(!refused.ok, "{refused:?}");
    assert!(error_message(&refused).contains("cannot receive"));
    assert_eq!(worker.state, before);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bot_identity_names_share_the_session_namespace_with_agents() {
    let (mut worker, _, agent, dir) = fixture();
    let other = worker.state.create_room("other").unwrap();
    let agent_conflict = call(
        &mut worker,
        "conflict-with-agent",
        "bot.add",
        json!({"room":"other","name":" codex1 "}),
    );
    assert!(!agent_conflict.ok, "{agent_conflict:?}");
    assert!(
        error_message(&agent_conflict).contains("already used by agent"),
        "{agent_conflict:?}"
    );
    let added = call(
        &mut worker,
        "add-bot",
        "bot.add",
        json!({"name":"inbox-watcher"}),
    );
    assert!(added.ok, "{added:?}");
    let bot_conflict = call(
        &mut worker,
        "duplicate-bot",
        "bot.add",
        json!({"room":"other","name":" inbox-watcher "}),
    );
    assert!(!bot_conflict.ok, "{bot_conflict:?}");
    assert!(
        error_message(&bot_conflict).contains("already used by bot"),
        "{bot_conflict:?}"
    );
    let create_error = worker
        .state
        .create_agent(other, "inbox-watcher", Provider::Codex, dir.clone(), None)
        .unwrap_err();
    assert!(create_error.to_string().contains("already used by bot"));
    let rename = call(
        &mut worker,
        "rename-conflict",
        "agent.rename",
        json!({"agent":"codex1","name":"inbox-watcher"}),
    );
    assert!(!rename.ok, "{rename:?}");
    assert!(
        error_message(&rename).contains("already used by bot"),
        "{rename:?}"
    );
    assert_eq!(worker.state.agent(agent).unwrap().name, "codex1");
    worker
        .state
        .create_agent(other, "Inbox-watcher", Provider::Codex, dir.clone(), None)
        .unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
