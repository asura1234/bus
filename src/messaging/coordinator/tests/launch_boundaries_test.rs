use super::launch_metrics_tests::{ledger, seed_launch};
use super::*;

#[test]
fn launch_metrics_startup_deadline_follows_command_start_after_slow_preparation() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, 0);
    worker
        .watch_agent_startup(agent, vec!["codex".into()])
        .unwrap();
    let expected = worker
        .state
        .agent(agent)
        .unwrap()
        .startup
        .as_ref()
        .unwrap()
        .started_at_ms
        + 30_000;
    let saved = ledger(&worker);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(
        saved["1"]["started_at_ms"], 1,
        "request-hour attribution stays unchanged"
    );
    assert_eq!(saved["1"]["deadline_at_ms"], expected);
}
#[test]
fn launch_metrics_unbound_resume_expires_on_authoritative_absence() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let mut doc = serde_json::to_value(&worker.state).unwrap();
    doc["launch_attempts"]["1"]["launch_id"] = serde_json::Value::Null;
    doc["launch_attempts"]["1"]["terminal_id"] = serde_json::Value::Null;
    doc["launch_attempts"]["1"]["kind"] = json!("resume");
    let mut state: BusState = serde_json::from_value(doc.clone()).unwrap();
    let previous = state.agent(agent).unwrap().clone();
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, None, 100)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&state).unwrap()["launch_attempts"]["1"]["outcome"],
        "pending"
    );
    doc["launch_attempts"]["1"]["deadline_at_ms"] = json!(0);
    let mut state: BusState = serde_json::from_value(doc).unwrap();
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, None, 100)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["launch_attempts"]["1"]["outcome"], "failed");
    assert_eq!(saved["launch_attempts"]["1"]["cause"], "startup_timeout");
}

#[test]
fn launch_metrics_listed_previous_terminal_expires_unbound_resume() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let mut doc = serde_json::to_value(&worker.state).unwrap();
    doc["launch_attempts"]["1"]["launch_id"] = serde_json::Value::Null;
    doc["launch_attempts"]["1"]["terminal_id"] = serde_json::Value::Null;
    doc["launch_attempts"]["1"]["kind"] = json!("resume");
    let mut state: BusState = serde_json::from_value(doc.clone()).unwrap();
    let previous = state.agent(agent).unwrap().clone();
    let mut info = native_info("pane", "codex", "session");
    info.terminal_id = "terminal".into();
    info.native_ready = true;
    info.interactive_ready = true;
    info.agent_status = schema::AgentStatus::Idle;
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, Some(&info), 100)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&state).unwrap()["launch_attempts"]["1"]["outcome"],
        "pending",
        "a listed previous terminal must not settle before the deadline"
    );
    doc["launch_attempts"]["1"]["deadline_at_ms"] = json!(0);
    let mut state: BusState = serde_json::from_value(doc).unwrap();
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, Some(&info), 100)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["launch_attempts"]["1"]["outcome"], "failed");
    assert_eq!(saved["launch_attempts"]["1"]["cause"], "startup_timeout");
    assert!(saved["launch_attempts"]["1"]["terminal_id"].is_null());
    assert_eq!(
        saved["agents"][agent.0.to_string()]["runtime_identity"]["terminal_id"],
        "terminal"
    );
}

// The existing submission revision is causal even when clocks tie. A new
// message can also join an older running turn, whose lead remains current.
fn bound_turn(fresh_member: bool) -> (BusState, AgentId, Worker, PathBuf) {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    seed_launch(&mut worker, agent, u64::MAX);
    let mut state = worker.state.clone();
    let lead = state
        .submit_message_from(
            room,
            Draft {
                text: "old work".into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            Author::Developer,
            if fresh_member { 99 } else { 100 },
        )
        .unwrap()[0];
    let fresh = if fresh_member {
        Some(
            state
                .submit_message_from(
                    room,
                    Draft {
                        text: "new work".into(),
                        files: vec![],
                        recipient_ids: [agent].into(),
                    },
                    Author::Developer,
                    101,
                )
                .unwrap()[0],
        )
    } else {
        None
    };
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["launch_attempts"]["1"]["started_at_ms"] = json!(100);
    doc["launch_attempts"]["1"]["submission_revision_floor"] = json!(2);
    doc["agents"][agent.0.to_string()]["current_request"] = json!(lead.0);
    doc["agents"][agent.0.to_string()]["status"] = json!("working");
    doc["agents"][agent.0.to_string()]["status_revision"] = json!(if fresh_member { 2 } else { 1 });
    doc["next_status_revision"] = json!(if fresh_member { 3 } else { 2 });
    doc["queues"][agent.0.to_string()] = json!([]);
    for (id, revision) in std::iter::once((lead, 1)).chain(fresh.map(|id| (id, 2))) {
        let r = &mut doc["requests"][id.0.to_string()];
        r["phase"] = json!("active");
        r["group"] = json!(lead.0);
        r["expected_launch_id"] = json!("launch");
        r["trusted_start_bound"] = json!(true);
        r["submission_status_revision"] = json!(revision);
        r["submission_boundary"] = json!(1);
        r["provider_session_id"] = json!("session");
        r["provider_turn_id"] = json!("turn");
        r["provider_prompt_id"] = json!(format!("prompt-{}", id.0));
        r["steered"] = json!(id != lead);
    }
    (serde_json::from_value(doc).unwrap(), agent, worker, dir)
}
fn working_info(state: &BusState, agent: AgentId) -> schema::AgentInfo {
    let mut info = native_info("pane", "codex", "session");
    info.name = Some(crate::messaging::identity::managed_name(
        state.agent(agent).unwrap().room_id,
        agent,
    ));
    info.native_ready = true;
    info.interactive_ready = true;
    info.agent_status = schema::AgentStatus::Working;
    info
}
#[test]
fn launch_metrics_old_bound_turn_in_the_same_millisecond_is_not_fresh_readiness() {
    let (mut state, agent, worker, dir) = bound_turn(false);
    let previous = state.agent(agent).unwrap().clone();
    let info = working_info(&state, agent);
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, Some(&info), 200)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["launch_attempts"]["1"]["outcome"], "pending");
}
#[test]
fn launch_metrics_fresh_message_joining_an_old_turn_proves_usability() {
    let (mut state, agent, worker, dir) = bound_turn(true);
    let previous = state.agent(agent).unwrap().clone();
    let info = working_info(&state, agent);
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, Some(&info), 200)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["launch_attempts"]["1"]["outcome"], "usable");
}

#[test]
fn launch_metrics_confirmed_join_does_not_require_its_own_submit_hook() {
    let (state, agent, worker, dir) = bound_turn(true);
    let current = state.agent(agent).unwrap().current_request.unwrap();
    let child = state
        .requests()
        .find(|r| r.agent_id == agent && r.id != current)
        .unwrap()
        .id;
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["requests"][child.0.to_string()]["trusted_start_bound"] = json!(false);
    let mut state: BusState = serde_json::from_value(doc).unwrap();
    assert!(state.request(current).unwrap().trusted_start_bound);
    assert!(!state.request(child).unwrap().uncertain_outcome);
    let previous = state.agent(agent).unwrap().clone();
    let info = working_info(&state, agent);
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, Some(&info), 200)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["launch_attempts"]["1"]["outcome"], "usable");
}
#[test]
fn launch_metrics_unconfirmed_join_is_not_usable_evidence() {
    let (state, agent, worker, dir) = bound_turn(true);
    let current = state.agent(agent).unwrap().current_request.unwrap();
    let child = state
        .requests()
        .find(|r| r.agent_id == agent && r.id != current)
        .unwrap()
        .id;
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["requests"][child.0.to_string()]["trusted_start_bound"] = json!(false);
    doc["requests"][child.0.to_string()]["uncertain_outcome"] = json!(true);
    let mut state: BusState = serde_json::from_value(doc).unwrap();
    let previous = state.agent(agent).unwrap().clone();
    let info = working_info(&state, agent);
    crate::messaging::coordinator::launch_metrics::observe(&mut state, &previous, Some(&info), 200)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    assert_eq!(saved["launch_attempts"]["1"]["outcome"], "pending");
}
