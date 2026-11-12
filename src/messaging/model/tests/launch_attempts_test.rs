use super::BusState;
use crate::messaging::storage::state_store::JsonStore;
use serde_json::Value;

// Removing the launch ledger from the durable schema loses failed attempts
// that have no roster entry, so current agents cannot reconstruct them.
#[test]
fn launch_ledger_roundtrip_preserves_pre_agent_attempts() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/launches.json")).unwrap();
    let mut doc = serde_json::to_value(BusState::new()).unwrap();
    doc["launch_attempts"] = fixture["attempts"].clone();
    doc["launch_tracking_started_at_ms"] = fixture["tracking_started_at_ms"].clone();
    let state: BusState = serde_json::from_value(doc).unwrap();
    let root = crate::utils::test_temp::unique_temp_path("launch-ledger-roundtrip");
    std::fs::create_dir_all(&root).unwrap();
    let store = JsonStore::new(root.join("state.json"));
    store.save(&state).unwrap();
    let saved = serde_json::to_value(store.load().unwrap().unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(
        saved["launch_attempts"].as_object().map(|x| x.len()),
        Some(7)
    );
    assert_eq!(saved["launch_attempts"]["4"]["cause"], "missing_executable");
    assert!(saved["launch_attempts"]["4"]["agent_id"].is_null());
    assert_eq!(
        saved["launch_attempts"]["1"]["started_at_ms"],
        fixture["tracking_started_at_ms"]
    );
}

fn pending_model_launch() -> (BusState, super::AgentId) {
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let agent = state
        .create_agent(
            room,
            "before rename",
            super::Provider::Codex,
            "/tmp".into(),
            None,
        )
        .unwrap();
    let mut doc = serde_json::to_value(&state).unwrap();
    doc["agents"][agent.0.to_string()]["launch_attempt_id"] = serde_json::json!(1);
    doc["launch_attempts"] = serde_json::json!({"1":{
        "id":1,"name":"before rename","provider":"codex","room_id":room.0,
        "kind":"create","agent_id":agent.0,"launch_id":null,"terminal_id":null,
        "started_at_ms":100,"deadline_at_ms":30100,"settled_at_ms":null,
        "outcome":"pending","cause":null
    }});
    (serde_json::from_value(doc).unwrap(), agent)
}
#[test]
fn launch_metrics_deletion_cancels_pending_attempt_without_losing_its_identity() {
    let (mut state, agent) = pending_model_launch();
    state.rename_agent(agent, "renamed").unwrap();
    state.observe_launch_dialog(agent, true, 200);
    state.delete_agent(agent, 500).unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    let a = &saved["launch_attempts"]["1"];
    assert_eq!(a["outcome"], "cancelled");
    assert_eq!(a["cause"], "agent_deleted");
    assert_eq!(a["settled_at_ms"], 500);
    assert_eq!(a["dialog_wait_ms"], 300);
    assert!(a["dialog_wait_started_at_ms"].is_null());
    assert_eq!(a["name"], "before rename");
}
#[test]
fn launch_metrics_deliberate_interrupt_cancels_pending_attempt() {
    let (mut state, agent) = pending_model_launch();
    state.begin_interrupt(agent).unwrap();
    let room = state.agent(agent).unwrap().room_id;
    state
        .post_interrupt_notice(room, super::Author::Developer, agent, 550)
        .unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    let a = &saved["launch_attempts"]["1"];
    assert_eq!(a["outcome"], "cancelled");
    assert_eq!(a["cause"], "interrupted");
    assert_eq!(a["settled_at_ms"], 550);
}

use super::{AgentId, AgentRuntimeIdentity, LaunchKind, LaunchOutcome, Provider};

fn tracked_agent() -> (BusState, AgentId, u64) {
    let mut s = BusState::new();
    let room = s.create_room("work").unwrap();
    let agent = s
        .create_agent(room, "original", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let id = s.begin_launch_attempt("original", Provider::Codex, room, LaunchKind::Create, 100);
    s.attach_launch_attempt(id, agent).unwrap();
    (s, agent, id)
}
fn launch_identity(terminal: &str) -> AgentRuntimeIdentity {
    AgentRuntimeIdentity {
        launch_id: Some("launch".into()),
        terminal_id: Some(terminal.into()),
        ..Default::default()
    }
}

#[test]
fn launch_ledger_settlement_is_identity_bound_and_terminal() {
    let (mut s, agent, id) = tracked_agent();
    let identity = launch_identity("first");
    assert!(s.bind_launch_identity(id, &identity));
    assert!(!s.bind_launch_identity(id, &launch_identity("replacement")));
    assert!(!s.settle_launch_attempt(
        id,
        &launch_identity("replacement"),
        LaunchOutcome::Usable,
        None,
        200
    ));
    assert_eq!(
        s.current_launch_attempt(agent).unwrap().outcome,
        LaunchOutcome::Pending
    );
    assert!(s.settle_launch_attempt(
        id,
        &identity,
        LaunchOutcome::Failed,
        Some("command_failed"),
        210
    ));
    assert!(!s.settle_launch_attempt(id, &identity, LaunchOutcome::Usable, None, 220));
    assert!(!s.settle_launch_attempt(
        id,
        &identity,
        LaunchOutcome::Failed,
        Some("another_cause"),
        230
    ));
    let settled = s.current_launch_attempt(agent).unwrap();
    assert_eq!(settled.outcome, LaunchOutcome::Failed);
    assert_eq!(settled.cause.as_deref(), Some("command_failed"));
    assert_eq!(settled.settled_at_ms, Some(210));
}

#[test]
fn launch_ledger_pre_agent_failure_never_aliases_a_later_agent() {
    let mut s = BusState::new();
    let room = s.ensure_master_room();
    let before = s.next_id;
    let failed = s.begin_launch_attempt("missing", Provider::Kimi, room, LaunchKind::Create, 100);
    assert_eq!(s.next_id, before, "audit IDs must not consume roster IDs");
    assert!(s.settle_launch_attempt(
        failed,
        &Default::default(),
        LaunchOutcome::Failed,
        Some("missing_executable"),
        101
    ));
    let later = s
        .create_agent(room, "later", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    assert!(s.current_launch_attempt(later).is_none());
    assert!(s.launch_attempts().next().unwrap().agent_id.is_none());
    assert!(s.attach_launch_attempt(failed, later).is_err());
}

#[test]
fn launch_ledger_resume_has_fresh_identity_and_preserves_failed_predecessor() {
    let (mut s, agent, first) = tracked_agent();
    let identity = launch_identity("old");
    s.set_agent_runtime_identity(agent, identity.clone())
        .unwrap();
    s.bind_launch_identity(first, &identity);
    s.settle_launch_attempt(
        first,
        &identity,
        LaunchOutcome::Failed,
        Some("startup_timeout"),
        130,
    );
    let second = s.begin_launch_attempt(
        "original",
        Provider::Codex,
        s.agent(agent).unwrap().room_id,
        LaunchKind::Resume,
        200,
    );
    s.attach_launch_attempt(second, agent).unwrap();
    assert_eq!(s.current_launch_attempt(agent).unwrap().terminal_id, None);
    assert!(!s.settle_launch_attempt(second, &identity, LaunchOutcome::Usable, None, 210));
    let fresh = launch_identity("new");
    s.bind_launch_identity(second, &fresh);
    assert!(s.settle_launch_attempt(second, &fresh, LaunchOutcome::Usable, None, 220));
    let attempts: Vec<_> = s.launch_attempts().collect();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].outcome, LaunchOutcome::Failed);
    assert_eq!(attempts[1].outcome, LaunchOutcome::Usable);
    assert_eq!(attempts[1].kind, LaunchKind::Resume);
}

#[test]
fn launch_ledger_snapshots_are_shared_but_mutations_are_durable() {
    let (mut s, agent, id) = tracked_agent();
    let snapshot = s.clone();
    assert!(std::sync::Arc::ptr_eq(
        &s.launch_attempts,
        &snapshot.launch_attempts
    ));
    s.bind_launch_identity(id, &launch_identity("first"));
    assert!(s.durable_poll_change_from(&snapshot));
    assert!(snapshot
        .current_launch_attempt(agent)
        .unwrap()
        .terminal_id
        .is_none());
    let saved: BusState = serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
    assert_eq!(
        saved.current_launch_attempt(agent),
        s.current_launch_attempt(agent)
    );
}

#[test]
fn launch_ledger_keeps_original_name_and_provider_after_rename_and_deletion() {
    let (mut s, agent, id) = tracked_agent();
    s.bind_launch_identity(id, &launch_identity("first"));
    s.settle_launch_attempt(
        id,
        &launch_identity("first"),
        LaunchOutcome::Usable,
        None,
        200,
    );
    s.rename_agent(agent, "renamed").unwrap();
    s.delete_agent(agent, 300).unwrap();
    let attempt = s.launch_attempts().next().unwrap();
    assert_eq!(attempt.name, "original");
    assert_eq!(attempt.provider, Provider::Codex);
    assert_eq!(attempt.outcome, LaunchOutcome::Usable);
}

#[test]
fn launch_ledger_legacy_state_has_no_invented_history() {
    let mut doc = serde_json::to_value(BusState::new()).unwrap();
    doc.as_object_mut().unwrap().remove("launch_attempts");
    doc.as_object_mut()
        .unwrap()
        .remove("launch_tracking_started_at_ms");
    let mut s: BusState = serde_json::from_value(doc).unwrap();
    assert_eq!(s.launch_attempts().count(), 0);
    assert!(s.launch_tracking_started_at_ms().is_none());
    s.begin_launch_attempt(
        "first tracked",
        Provider::ClaudeCode,
        super::RoomId(1),
        LaunchKind::Create,
        900,
    );
    assert_eq!(s.launch_tracking_started_at_ms(), Some(900));
    assert!(!s.is_pristine());
}
