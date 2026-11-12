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
    let before = state.clone();
    state
        .post_to_developer(work, orch, "[waiting-on-you] Choose".into(), vec![], 10)
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
