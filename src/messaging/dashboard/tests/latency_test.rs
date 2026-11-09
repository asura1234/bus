use super::latency::{elapsed, hour_with_offset, percentiles, snapshot, HourKey};
use crate::messaging::model::NativeSubmissionTimestamps;
use crate::messaging::model::{AgentId, BusState, DeliveryLatency, Provider, RequestId, RoomId};

fn singapore_hour(ms: u64) -> HourKey {
    hour_with_offset(ms, 8 * 3600).unwrap()
}

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
fn snapshot_aggregates_ready_bus_phases_by_provider_recipient_and_hour() {
    let mut state = BusState::new();
    let room = state.create_room("latency").unwrap();
    let claude = state
        .create_agent(
            room,
            "claude",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    let codex = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    let busy = state
        .create_agent(room, "busy", Provider::Kimi, "/scratch".into(), None)
        .unwrap();
    let base = 1_793_577_600_000;
    let first = add_request(&mut state, room, claude, base, 3, true);
    let second = add_request(&mut state, room, codex, base + 3_600_000, 1, true);
    add_request(&mut state, room, busy, base + 7_200_000, 10, false);

    let report = snapshot(&state, singapore_hour);
    let overall = &report.all.phases["accepted_to_box"];
    assert_eq!(
        (overall.n, overall.p50, overall.p90, overall.p99),
        (2, Some(50), Some(150), Some(150))
    );
    assert_eq!(report.all.phases["ready_queue"].p50, Some(10));
    assert_eq!(report.all.phases["native_total"].n, 3);
    assert!(!report.all.phases.contains_key("reply_delivery"));
    assert!(!report.all.phases.contains_key("end_to_end"));
    let picked = &report.all.phases["accepted_to_picked"];
    assert_eq!(
        (picked.n, picked.p50, picked.p90, picked.p99, picked.mean),
        (3, Some(60), Some(200), Some(200), Some(93))
    );
    assert_eq!(
        report.providers["claude"].phases["accepted_to_box"].p50,
        Some(150)
    );
    assert_eq!(
        report.providers["codex"].phases["accepted_to_box"].p50,
        Some(50)
    );
    assert_eq!(
        report.recipients[&claude.0.to_string()].series.phases["accepted_to_box"].p50,
        Some(150)
    );
    assert_eq!(
        report.recipients[&codex.0.to_string()].series.phases["accepted_to_box"].p50,
        Some(50)
    );
    assert_eq!(report.all.hours.len(), 3);
    assert_eq!(report.all.hours[0].phases["accepted_to_box"].p50, Some(150));
    assert_eq!(report.all.hours[1].phases["accepted_to_box"].p50, Some(50));
    assert_eq!(report.all.hours[2].phases["accepted_to_box"].n, 0);
    assert_eq!(report.slowest[0].request_id, first.0);
    assert_eq!(report.slowest[1].request_id, second.0);
    assert_eq!(report.slowest[0].accepted_to_box_ms, Some(150));
    assert_eq!(report.slowest[0].phases["ready_queue"], 30);
}

#[test]
fn legacy_requests_have_no_latency_samples() {
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
    let state = serde_json::from_value(document).unwrap();
    let report = snapshot(&state, singapore_hour);
    assert!(report.all.hours.is_empty());
    assert!(report.slowest.is_empty());
    assert_eq!(report.all.phases["accepted_to_box"].n, 0);
    assert_eq!(report.all.phases["accepted_to_box"].p50, None);
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
fn provider_slowest_drilldown_retains_requests_outside_global_top_fifty() {
    let mut state = BusState::new();
    let room = state.create_room("latency").unwrap();
    let claude = state
        .create_agent(
            room,
            "claude",
            Provider::ClaudeCode,
            "/scratch".into(),
            None,
        )
        .unwrap();
    let codex = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    let base = 1_793_577_600_000;
    for index in 0..50 {
        add_request(&mut state, room, claude, base + index * 1_000, 100, true);
    }
    let codex_request = add_request(&mut state, room, codex, base + 50_000, 1, true);
    let report = snapshot(&state, singapore_hour);
    assert_eq!(report.providers["codex"].phases["accepted_to_box"].n, 1);
    assert_eq!(report.slowest.len(), 50);
    assert_eq!(report.provider_slowest["codex"].len(), 1);
    assert_eq!(
        report.provider_slowest["codex"][0].request_id,
        codex_request.0
    );
}

#[test]
fn delivery_latency_native_total_excludes_composer_wait_between_attempts() {
    use crate::messaging::model::{AgentRuntimeIdentity, RuntimeStatus, SubmissionOutcome};

    let mut state = BusState::new();
    let room = state.create_room("latency retries").unwrap();
    let agent = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("latency-launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let accepted = 1_793_577_600_000;
    state
        .observe_status(agent, RuntimeStatus::Idle, accepted)
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    state
        .set_draft_text(room, "retry after the composer is available")
        .unwrap();
    let request = state.submit_draft(room, accepted).unwrap()[0];
    state.record_queued(request, accepted + 5).unwrap();

    state
        .begin_submission(request, "latency-launch", 0)
        .unwrap();
    state.record_picked(request, accepted + 10, false).unwrap();
    state
        .record_native_requested(request, accepted + 15)
        .unwrap();
    state
        .record_submission(request, SubmissionOutcome::ComposerBusy)
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Blocked, accepted + 20)
        .unwrap();

    let second_native_request = accepted + 60_000;
    state
        .observe_status(agent, RuntimeStatus::Idle, second_native_request)
        .unwrap();
    state
        .begin_submission(request, "latency-launch", 0)
        .unwrap();
    state
        .record_picked(request, second_native_request, true)
        .unwrap();
    state
        .record_native_requested(request, second_native_request)
        .unwrap();
    state
        .record_native_timestamps(
            request,
            NativeSubmissionTimestamps {
                server_accepted_at_ms: Some(second_native_request + 5),
                writer_queued_at_ms: Some(second_native_request + 10),
                paste_started_at_ms: Some(second_native_request + 15),
                text_written_at_ms: Some(second_native_request + 20),
                paste_completed_at_ms: Some(second_native_request + 30),
                composer_owned_at_ms: Some(second_native_request + 40),
                enter_sent_at_ms: Some(second_native_request + 50),
                enter_written_at_ms: Some(second_native_request + 60),
                native_confirmed_at_ms: Some(second_native_request + 100),
            },
        )
        .unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();

    let report = snapshot(&state, singapore_hour);
    assert_eq!(report.all.phases["native_total"].n, 1);
    assert_eq!(report.all.phases["native_total"].p50, Some(100));
}

#[test]
fn phases_pair_inside_one_native_attempt_and_drop_reversed_samples() {
    let mut state = BusState::new();
    let room = state.create_room("mixed attempts").unwrap();
    let agent = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    let accepted = 1_793_577_600_000;
    let anchor = accepted + 10_000;
    state.set_draft_recipients(room, [agent]).unwrap();
    state.set_draft_text(room, "mixed attempt fixture").unwrap();
    let id = state.submit_draft(room, accepted).unwrap()[0];
    let latency = DeliveryLatency {
        accepted_at_ms: Some(accepted),
        queued_at_ms: Some(accepted + 10),
        picked_at_ms: Some(accepted + 20),
        native_requested_at_ms: Some(anchor),
        native: NativeSubmissionTimestamps {
            server_accepted_at_ms: Some(accepted + 35),
            writer_queued_at_ms: Some(accepted + 40),
            paste_started_at_ms: Some(anchor + 10),
            text_written_at_ms: Some(anchor + 20),
            paste_completed_at_ms: Some(anchor + 5),
            composer_owned_at_ms: None,
            enter_sent_at_ms: None,
            enter_written_at_ms: None,
            native_confirmed_at_ms: Some(accepted + 80),
        },
        provider_started_at_ms: Some(accepted + 90),
        ready_at_enqueue: true,
        agent_waited: false,
        ..DeliveryLatency::default()
    };
    let mut document = serde_json::to_value(&state).unwrap();
    document["requests"][id.0.to_string()]["latency"] = serde_json::to_value(latency).unwrap();
    let state: BusState = serde_json::from_value(document).unwrap();
    let report = snapshot(&state, singapore_hour);
    assert_eq!(report.all.phases["writer_queue"].n, 0);
    assert_eq!(report.all.phases["native_transport"].n, 0);
    assert_eq!(report.all.phases["native_total"].n, 0);
    assert_eq!(report.all.phases["paste_pacing"].n, 0);
    assert_eq!(report.all.phases["text_write"].p50, Some(10));
    assert_eq!(
        report.all.phases["accepted_to_box"].p50,
        Some(anchor + 20 - accepted)
    );
    assert!(!report.all.phases.contains_key("end_to_end"));
    assert!(!report.all.phases.contains_key("reply_delivery"));
    assert!(report.slowest[0].phases.contains_key("text_write"));
    assert!(!report.slowest[0].phases.contains_key("writer_queue"));
    assert!(!report.slowest[0].phases.contains_key("paste_pacing"));
}
