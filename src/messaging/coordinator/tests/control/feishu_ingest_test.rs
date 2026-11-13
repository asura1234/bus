use super::*;
use crate::messaging::prefs::settings;
use crate::messaging::storage::state_store::SaveStage;

fn mapped_worker() -> (Worker, PathBuf, String) {
    let (mut worker, work, _, root) = fixture();
    worker.settings_path = Some(root.join("settings.json"));
    settings::save(
        worker.settings_path.as_ref().unwrap(),
        &settings::BusSettings {
            feishu_recipient: "ou_developer".into(),
            feishu_bridge_path: Some("/usr/bin/true".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut state = worker.state.clone();
    let master = state.ensure_master_room();
    let orch = state
        .create_agent(master, "orch", Provider::Codex, root.clone(), None)
        .unwrap();
    state.bind_orchestrator(orch, work).unwrap();
    let raising = state
        .create_agent(work, "raising-worker", Provider::Codex, root.clone(), None)
        .unwrap();
    let before = state.clone();
    state
        .post_to_developer(work, raising, "[waiting-on-you] Choose".into(), vec![], 10)
        .unwrap();
    state.prepare_feishu_alerts(&before, root.to_str().unwrap(), "ou_developer", 11);
    let code = state.feishu.outbox.keys().next().unwrap().clone();
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    worker.save(state).unwrap();
    (worker, root, code)
}

fn params() -> serde_json::Value {
    json!({"app_id":"cli_test", "event":{
        "app_id":"cli_test", "message_id":"om_in", "chat_id":"oc_dm", "chat_type":"p2p",
        "sender":{"open_id":"ou_developer","sender_type":"user"}, "text":"Proceed",
        "reply_to":"om_out", "root_id":null, "thread_id":null, "create_time":"20"
    }})
}

#[test]
fn feishu_ingest_acknowledges_only_the_durable_queue_and_deduplication_commit() {
    let (mut worker, root, _) = mapped_worker();
    worker.store.fail_once_at(SaveStage::TempSync);
    let failed = call(&mut worker, "failed-save", "feishu.ingest", params());
    assert!(!failed.ok, "sink must retry a failed commit: {failed:?}");
    drop(worker);
    let mut reopened = Worker::open(root.clone(), Box::new(NoTransport)).unwrap();
    reopened.settings_path = Some(root.join("settings.json"));
    assert_eq!(reopened.state.requests().count(), 0);
    let first = call(&mut reopened, "retry-save", "feishu.ingest", params());
    assert!(first.ok, "{first:?}");
    let stored = reopened.store.load().unwrap().unwrap();
    assert_eq!(stored.requests().count(), 1);
    assert_eq!(stored.feishu.inbox.len(), 1);
    drop(reopened);
    let mut reopened = Worker::open(root.clone(), Box::new(NoTransport)).unwrap();
    reopened.settings_path = Some(root.join("settings.json"));
    let replay = call(&mut reopened, "restart-replay", "feishu.ingest", params());
    assert!(replay.ok, "{replay:?}");
    assert_eq!(replay.result, first.result);
    assert_eq!(reopened.state.requests().count(), 1);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn feishu_ingest_cannot_acknowledge_an_uncommitted_batch() {
    let (mut worker, root, _) = mapped_worker();
    worker.begin_batch();
    let response = call(&mut worker, "batched", "feishu.ingest", params());
    assert!(
        !response.ok,
        "external acceptance must not precede finish_batch: {response:?}"
    );
    assert_eq!(worker.state.requests().count(), 0);
    worker.finish_batch().unwrap();
    assert_eq!(worker.store.load().unwrap().unwrap().requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn feishu_ingest_rejects_wrong_app_bot_and_group_before_queueing() {
    let (mut worker, root, _) = mapped_worker();
    for (field, value) in [("app_id", "cli_other"), ("chat_type", "group")] {
        let mut p = params();
        p["event"][field] = value.into();
        assert!(!call(&mut worker, field, "feishu.ingest", p).ok);
    }
    let mut p = params();
    p["event"]["sender"]["sender_type"] = "app".into();
    assert!(!call(&mut worker, "bot", "feishu.ingest", p).ok);
    assert_eq!(worker.state.requests().count(), 0);
    assert!(worker.state.feishu.inbox.is_empty());
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn feishu_reply_fans_out_to_persisted_repeats_and_a_late_alias_once() {
    let (mut worker, root, code) = mapped_worker();
    let original = worker.state.feishu.outbox[&code].source.prompt_id;
    let (events, _) = std::sync::mpsc::channel();
    worker.pump_feishu(&events, 11).unwrap();
    let post_repeat = |worker: &mut Worker, at| {
        let record = &worker.state.feishu.outbox[&code];
        let (room, raising) = (record.room, record.raising_agent.unwrap());
        let mut state = worker.state.clone();
        let source = state
            .post_to_developer(room, raising, "[waiting_on_you] Choose".into(), vec![], at)
            .unwrap();
        worker.save(state).unwrap();
        worker.pump_feishu(&events, at).unwrap();
        source
    };
    let repeated = post_repeat(&mut worker, 12);
    let accepted = call(&mut worker, "alias-first-reply", "feishu.ingest", params());
    assert!(accepted.ok, "{accepted:?}");
    assert_eq!(
        worker.state.requests().count(),
        2,
        "each original/alias request gets the reply"
    );
    drop(worker);
    let mut worker = Worker::open(root.clone(), Box::new(NoTransport)).unwrap();
    worker.settings_path = Some(root.join("settings.json"));
    let mut edited = params();
    edited["event"]["text"] = "CHANGED REPLAY".into();
    let replay = call(&mut worker, "alias-restart-replay", "feishu.ingest", edited);
    assert_eq!(replay.result, accepted.result);
    assert_eq!(worker.state.requests().count(), 2);
    worker.pump_feishu(&events, 20).unwrap();
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    state
        .delete_agent(state.feishu.outbox[&code].orchestrator.unwrap(), 20)
        .unwrap();
    let other = state
        .create_agent(master, "orch", Provider::Codex, root.clone(), None)
        .unwrap();
    state
        .bind_orchestrator(other, state.feishu.outbox[&code].room)
        .unwrap();
    worker.save(state).unwrap();
    let late = post_repeat(&mut worker, 21);
    assert_eq!(
        worker.state.requests().count(),
        3,
        "a late alias gets the cached reply without another Feishu message"
    );
    for source in [original, repeated, late] {
        let routes: Vec<_> = worker
            .state
            .requests()
            .filter(|request| {
                request
                    .prompt
                    .in_reply_to
                    .as_ref()
                    .is_some_and(|reply| reply.source.prompt_id == source)
            })
            .collect();
        assert_eq!(routes.len(), 1);
        assert!(routes[0].queue_only);
        assert_eq!(
            routes[0].prompt.author,
            crate::messaging::model::Author::Developer
        );
        assert_eq!(routes[0].prompt.text, "Proceed");
        if source == late {
            assert_eq!(routes[0].agent_id, other);
        }
    }
    let replay = call(&mut worker, "alias-late-replay", "feishu.ingest", params());
    assert_eq!(replay.result, accepted.result);
    assert_eq!(worker.state.requests().count(), 3);
    let mut later = params();
    later["event"]["message_id"] = "om_later_reply".into();
    let parked_root = call(&mut worker, "alias-partial-target", "feishu.ingest", later);
    assert_eq!(parked_root.result["stage"], "parked");
    let delivered: Vec<_> = worker
        .state
        .requests()
        .filter(|request| {
            request
                .prompt
                .in_reply_to
                .as_ref()
                .is_some_and(|reply| reply.feishu_message_id == "om_later_reply")
        })
        .collect();
    assert_eq!(
        delivered.len(),
        1,
        "unavailable earlier targets cannot block a live alias"
    );
    assert_eq!(delivered[0].agent_id, other);
    assert_eq!(
        delivered[0]
            .prompt
            .in_reply_to
            .as_ref()
            .unwrap()
            .source
            .prompt_id,
        late
    );
    let mut conflict = params();
    conflict["event"]["message_id"] = "om_alias_wrong_chat".into();
    conflict["event"]["chat_id"] = "oc_other".into();
    assert_eq!(
        call(
            &mut worker,
            "alias-chat-conflict",
            "feishu.ingest",
            conflict
        )
        .result["stage"],
        "parked"
    );
    assert_eq!(
        worker.state.requests().count(),
        4,
        "chat conflicts cannot fan out to any alias"
    );
    drop(worker);
    std::fs::remove_dir_all(root).unwrap();
}
