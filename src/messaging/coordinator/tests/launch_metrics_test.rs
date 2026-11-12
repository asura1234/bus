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
fn launch_metrics_dialog_wait_stays_pending_then_becomes_usable_after_deadline() {
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let (mut worker, agent, _, dir, _) = fixture(provider, vec![]);
        seed_launch(&mut worker, agent, 30_001);
        let previous = worker.state.agent(agent).unwrap().clone();
        let mut info = native_info("pane", provider.label(), "session");
        info.native_ready = false;
        info.interactive_ready = false;
        info.launch_pending = true;
        info.agent_status = schema::AgentStatus::Blocked;
        info.dialog_id = Some("folder-trust".into());
        for now in [10_000, 31_000, 44_000] {
            let before = worker.state.clone();
            super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), now)
                .unwrap();
            assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
            assert!(ledger(&worker)["1"]["cause"].is_null());
            if now > 10_000 {
                assert!(!worker.state.launch_records_changed_from(&before));
            }
        }
        // Pending dialogs survive a save/reload and keep their original clock.
        worker.save(worker.state.clone()).unwrap();
        worker.state = JsonStore::new(dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        info.dialog_id = None;
        info.launch_pending = false;
        info.agent_status = schema::AgentStatus::Idle;
        // Dismissing the dialog can precede prompt readiness by another poll.
        super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 44_500)
            .unwrap();
        assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
        info.native_ready = true;
        info.interactive_ready = true;
        super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 45_001)
            .unwrap();
        let record = ledger(&worker)["1"].clone();
        assert_eq!(record["outcome"], "usable");
        assert!(record["cause"].is_null());
        assert_eq!(record["dialog_wait_ms"], 34_500);
        assert!(record["dialog_wait_started_at_ms"].is_null());
        assert_eq!(
            record["settled_at_ms"].as_u64().unwrap() - record["started_at_ms"].as_u64().unwrap(),
            45_000
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn launch_metrics_dialog_does_not_hide_a_definite_launch_input_error() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, 30_001);
    let previous = worker.state.agent(agent).unwrap().clone();
    let mut info = native_info("pane", "codex", "session");
    info.dialog_id = Some("folder-trust".into());
    info.launch_input_error = Some("launch input rejected".into());
    super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 31_000)
        .unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "failed");
    assert_eq!(ledger(&worker)["1"]["cause"], "launch_input_failed");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_metrics_after_dialog_dismissal_still_bounds_a_stalled_launch() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, 30_001);
    let previous = worker.state.agent(agent).unwrap().clone();
    let mut info = native_info("pane", "codex", "session");
    info.dialog_id = Some("folder-trust".into());
    super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 44_000)
        .unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
    info.dialog_id = None;
    super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 45_000)
        .unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "pending");
    super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 75_001)
        .unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "failed");
    assert_eq!(ledger(&worker)["1"]["cause"], "startup_timeout");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_metrics_dialog_on_another_terminal_cannot_defer_the_deadline() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, 30_001);
    let previous = worker.state.agent(agent).unwrap().clone();
    let mut info = native_info("another-pane", "codex", "session");
    info.terminal_id = "another-terminal".into();
    info.dialog_id = Some("folder-trust".into());
    super::super::launch_metrics::observe(&mut worker.state, &previous, Some(&info), 44_000)
        .unwrap();
    assert_eq!(ledger(&worker)["1"]["outcome"], "failed");
    assert_eq!(ledger(&worker)["1"]["deadline_at_ms"], 30_001);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn launch_metrics_failed_native_observations_do_not_fabricate_failure() {
    let replies = (0..3)
        .map(|_| {
            Err(TransportError {
                timestamps: Default::default(),
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
        RuntimeStatus::Idle
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

#[test]
fn launch_metrics_invalid_arguments_do_not_count_as_launch_attempts() {
    let _providers = provider_binaries();
    let (mut worker, _, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let before = ledger(&worker);
    assert!(before.as_object().unwrap().is_empty());
    let result = worker.add_agent(
        AddAgent {
            room,
            name: "invalid-mode".into(),
            provider: Provider::Codex,
            cwd: dir.to_string_lossy().into(),
            extra_args: "exec".into(),
            consent_project_hooks: true,
        },
        None,
        &mpsc::channel().0,
    );
    let saved = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    let after = serde_json::to_value(&saved).unwrap()["launch_attempts"].clone();
    let native_calls = calls.lock().unwrap().clone();
    let roster_count = saved.agents().count();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert!(
        result
            .as_ref()
            .is_err_and(|e| e.contains("launch argument exec disables")),
        "the intended preflight rejection must be reached: {result:?}"
    );
    assert!(native_calls.is_empty(), "invalid mode reached native input");
    assert_eq!(roster_count, 1, "invalid input created a roster entry");
    assert_eq!(
        after, before,
        "known invalid arguments must not create failed launch attempts"
    );
}
