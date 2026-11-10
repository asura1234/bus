use super::latency::{elapsed, percentiles};
use crate::messaging::model::NativeSubmissionTimestamps;
use crate::messaging::model::{AgentId, BusState, DeliveryLatency, Provider, RequestId, RoomId};

#[test]
fn nearest_rank_and_backward_clocks_are_safe() {
    let values = percentiles(&[10, 2, 8, 4, 6, 1, 3, 5, 7, 9]);
    assert_eq!(values.n, 10);
    assert_eq!(values.p50, Some(5));
    assert_eq!(values.p90, Some(9));
    assert_eq!(values.p99, Some(10));
    assert_eq!(values.max, Some(10));
    assert_eq!(values.mean, Some(6));
    assert_eq!(elapsed(Some(100), Some(90)), None);
    assert_eq!(elapsed(Some(100), Some(100)), Some(0));
    assert_eq!(elapsed(Some(100), None), None);
    assert_eq!(elapsed(None, Some(100)), None);
}

fn add_request(
    state: &mut BusState,
    room: RoomId,
    agent: AgentId,
    accepted: u64,
    scale: u64,
    ready: bool,
) -> RequestId {
    state.set_draft_recipients(room, [agent]).unwrap();
    state.set_draft_text(room, "latency fixture").unwrap();
    let id = state.submit_draft(room, accepted).unwrap()[0];
    let latency = DeliveryLatency {
        accepted_at_ms: Some(accepted),
        queued_at_ms: Some(accepted + 10 * scale),
        picked_at_ms: Some(accepted + 20 * scale),
        native_requested_at_ms: Some(accepted + 30 * scale),
        native: NativeSubmissionTimestamps {
            server_accepted_at_ms: Some(accepted + 35 * scale),
            writer_queued_at_ms: Some(accepted + 40 * scale),
            paste_started_at_ms: Some(accepted + 45 * scale),
            text_written_at_ms: Some(accepted + 50 * scale),
            paste_completed_at_ms: Some(accepted + 60 * scale),
            composer_owned_at_ms: Some(accepted + 65 * scale),
            enter_sent_at_ms: Some(accepted + 70 * scale),
            enter_written_at_ms: Some(accepted + 75 * scale),
            native_confirmed_at_ms: Some(accepted + 80 * scale),
        },
        provider_started_at_ms: Some(accepted + 85 * scale),
        ready_at_enqueue: ready,
        agent_waited: !ready,
        ..DeliveryLatency::default()
    };
    let mut document = serde_json::to_value(&*state).unwrap();
    document["requests"][id.0.to_string()]["latency"] = serde_json::to_value(latency).unwrap();
    *state = serde_json::from_value(document).unwrap();
    id
}

#[test]
fn latency_routes_are_read_only_and_legacy_storage_stays_absent() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-latency-routes");
    let mut state = BusState::new();
    let room = state.create_room("legacy").unwrap();
    let agent = state
        .create_agent(room, "legacy", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    let request = add_request(&mut state, room, agent, 1_793_577_600_000, 1, true);
    let mut document = serde_json::to_value(&state).unwrap();
    document["requests"][request.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("latency");
    document["requests"][request.0.to_string()]["prompt"]["text"] =
        serde_json::Value::String("SENTINEL_LEGACY_CONTENT".into());
    let state = serde_json::from_value(document).unwrap();
    let path = root.join("state.json");
    crate::messaging::storage::state_store::JsonStore::new(path.clone())
        .save(&state)
        .unwrap();
    let before = std::fs::read(&path).unwrap();

    assert_eq!(
        super::response_for(&root, "GET", "/latency").status,
        "404 Not Found"
    );
    assert_eq!(
        super::response_for(&root, "GET", "/latency/data").status,
        "404 Not Found"
    );
    let data = super::response_for(&root, "GET", "/data");
    assert_eq!(data.status, "200 OK");
    let body: serde_json::Value = serde_json::from_slice(&data.body).unwrap();
    assert!(body["metrics"]["all"].get("end_to_end").is_none());
    assert_eq!(body["metrics"]["all"]["accepted_to_picked"]["n"], 0);
    assert!(body["metrics"]["all"]["accepted_to_picked"]["p50"].is_null());
    assert!(body["metrics"]["all"]["accepted_to_picked"]["mean"].is_null());
    assert!(body["metrics"]["providers"]["claude"]["accepted_to_picked"]["p99"].is_null());
    let raw = String::from_utf8(data.body).unwrap();
    assert!(!raw.contains("SENTINEL_LEGACY_CONTENT"));
    assert!(!raw.contains("/latency"));
    assert!(!raw.contains("reply_delivered_at"));
    assert!(!raw.contains("reply_captured_at"));
    assert!(!raw.contains("end_to_end"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(!root.join("state.journal.json").exists());
    assert_eq!(
        super::response_for(&root, "POST", "/data").status,
        "405 Method Not Allowed"
    );
}

#[test]
fn admission_selection_and_native_confirmation_are_independent_phase_samples() {
    let mut state = BusState::new();
    let room = state.create_room("phases").unwrap();
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    let first = add_request(&mut state, room, agent, 1_000, 1, true);
    state.record_eligible(first, 1_015);
    let second = add_request(&mut state, room, agent, 2_000, 1, false);
    // Legacy rows have no admission boundary. A reversed native anchor must
    // not become an accepted->confirmation sample from the wrong attempt.
    state.record_native_requested(second, 2_090).unwrap();
    let metrics = serde_json::to_value(super::metrics::snapshot(&state, |ms| {
        super::clock::hour_with_offset(ms, 0).unwrap()
    }))
    .unwrap();
    assert_eq!(metrics["all"]["accepted_to_eligible"]["n"], 1);
    assert_eq!(metrics["all"]["accepted_to_eligible"]["mean"], 15);
    assert_eq!(metrics["all"]["eligible_to_picked"]["n"], 1);
    assert_eq!(metrics["all"]["eligible_to_picked"]["mean"], 5);
    assert_eq!(metrics["all"]["accepted_to_native_confirmed"]["n"], 1);
    assert_eq!(metrics["all"]["accepted_to_native_confirmed"]["mean"], 80);
    assert_eq!(metrics["all"]["accepted_to_picked"]["n"], 2);
    assert_eq!(
        metrics["providers"]["codex"]["accepted_to_native_confirmed"]["mean"],
        80
    );
}
