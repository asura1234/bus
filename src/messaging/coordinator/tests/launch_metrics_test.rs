use super::*;
use crate::messaging::model::BusState;

pub(super) fn ledger(worker: &Worker) -> serde_json::Value {
    serde_json::to_value(&worker.state).unwrap()["launch_attempts"].clone()
}
pub(super) fn seed_launch(worker: &mut Worker, agent: AgentId, deadline: u64) {
    let a = worker.state.agent(agent).unwrap();
    let mut doc = serde_json::to_value(&worker.state).unwrap();
    doc["agents"][agent.0.to_string()]["launch_attempt_id"] = json!(1);
    doc["launch_attempts"] = json!({"1":{
        "id":1,"name":a.name,"provider":a.provider,"room_id":a.room_id,
        "kind":"create","agent_id":agent.0,
        "launch_id":a.runtime_identity.launch_id,"terminal_id":a.runtime_identity.terminal_id,
        "started_at_ms":1,"deadline_at_ms":deadline,"settled_at_ms":null,
        "outcome":"pending","cause":null
    }});
    doc["launch_tracking_started_at_ms"] = json!(1);
    worker
        .save(serde_json::from_value::<BusState>(doc).unwrap())
        .unwrap();
}

// This uses real add preflight and JsonStore, never a provider process or global PATH mutation.
#[test]
fn launch_metrics_missing_executable_is_durable_without_a_saved_agent() {
    let (mut worker, previous, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let bin = dir.join("empty-path");
    std::fs::create_dir(&bin).unwrap();
    let _path = crate::utils::env::PathOverride::set(&bin);
    let mut state = worker.state.clone();
    state.delete_agent(previous, 0).unwrap();
    worker.save(state).unwrap();
    let result = worker.add_agent(
        AddAgent {
            room,
            name: "missing".into(),
            provider: Provider::Codex,
            cwd: dir.to_string_lossy().into(),
            extra_args: String::new(),
            consent_project_hooks: true,
        },
        None,
        &mpsc::channel().0,
    );
    assert!(result.is_err());
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    let doc = serde_json::to_value(&saved).unwrap();
    let entries = doc["launch_attempts"].as_object().unwrap();
    assert_eq!(entries.len(), 1);
    let a = entries.values().next().unwrap();
    assert_eq!(a["name"], "missing");
    assert_eq!(a["outcome"], "failed");
    assert_eq!(a["cause"], "missing_executable");
    assert!(a["agent_id"].is_null());
    assert_eq!(saved.agents().count(), 0);
    assert!(calls.lock().unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_metrics_positive_readiness_settles_even_after_an_observation_gap() {
    for deadline in [u64::MAX, 0] {
        let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
        seed_launch(&mut worker, agent, deadline);
        let mut info = native_info("pane", "codex", "session");
        let a = worker.state.agent(agent).unwrap();
        info.name = Some(crate::messaging::identity::managed_name(a.room_id, agent));
        info.native_ready = true;
        info.interactive_ready = true;
        worker.transport = Box::new(FakeTransport {
            replies: vec![Ok(ResponseResult::AgentList {
                agents: vec![info.clone()],
            })]
            .into(),
            calls: Arc::default(),
            state_path: dir.join("state.json"),
        });
        worker.poll().unwrap();
        assert_eq!(ledger(&worker)["1"]["outcome"], "usable");
        let settled = ledger(&worker)["1"]["settled_at_ms"].clone();
        worker.transport = Box::new(FakeTransport {
            replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
            calls: Arc::default(),
            state_path: dir.join("state.json"),
        });
        worker.poll().unwrap();
        assert_eq!(ledger(&worker).as_object().unwrap().len(), 1);
        assert_eq!(ledger(&worker)["1"]["settled_at_ms"], settled);
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn launch_metrics_dialog_wait_expires_without_becoming_usable() {
    for (deadline, expected) in [(u64::MAX, "pending"), (0, "failed")] {
        let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
        seed_launch(&mut worker, agent, deadline);
        let a = worker.state.agent(agent).unwrap();
        let mut info = native_info("pane", "codex", "session");
        info.name = Some(crate::messaging::identity::managed_name(a.room_id, agent));
        info.native_ready = false;
        info.interactive_ready = false;
        info.launch_pending = true;
        info.agent_status = schema::AgentStatus::Blocked;
        info.dialog_id = Some("folder-trust".into());
        worker.transport = Box::new(FakeTransport {
            replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
            calls: Arc::default(),
            state_path: dir.join("state.json"),
        });
        worker.poll().unwrap();
        assert_eq!(ledger(&worker)["1"]["outcome"], expected);
        if expected == "failed" {
            assert_eq!(ledger(&worker)["1"]["cause"], "dialog_timeout");
        }
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn launch_metrics_failed_native_observations_do_not_fabricate_failure() {
    let replies = (0..3)
        .map(|_| {
            Err(TransportError {
                code: None,
                message: "lost list response".into(),
                definitely_rejected: false,
            })
        })
        .collect();
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, replies);
    seed_launch(&mut worker, agent, 0);
    for _ in 0..3 {
        assert!(worker.poll().unwrap().is_none());
    }
    assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
    assert_eq!(
        worker.state.agent(agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_metrics_saved_identity_and_idle_status_are_not_positive_process_evidence() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let a = worker.state.agent(agent).unwrap();
    let mut info = native_info("pane", "codex", "session");
    info.name = Some(crate::messaging::identity::managed_name(a.room_id, agent));
    info.native_ready = false;
    info.interactive_ready = true;
    worker.transport = Box::new(FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::default(),
        state_path: dir.join("state.json"),
    });
    worker.poll().unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
