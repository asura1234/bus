use super::latency::{elapsed, hour_with_offset, percentiles, snapshot, HourKey};
use crate::messaging::model::{AgentId, BusState, DeliveryLatency, Provider, RequestId, RoomId};
use crate::protocol::api::schema::NativeSubmissionTimestamps;

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
    assert_eq!(elapsed(Some(100), Some(90)), Some(0));
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
        reply_captured_at_ms: Some(accepted + 100 * scale),
        reply_delivered_at_ms: Some(accepted + 110 * scale),
        ready_at_enqueue: ready,
        agent_waited: !ready,
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
    let state = BusState::new();
    let report = snapshot(&state, singapore_hour);
    assert!(report.all.hours.is_empty());
    assert!(report.slowest.is_empty());
    assert_eq!(report.all.phases["accepted_to_box"].n, 0);
    assert_eq!(report.all.phases["accepted_to_box"].p50, None);
}

#[test]
fn latency_routes_are_read_only_and_legacy_storage_stays_absent() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-latency-routes");
    let page = super::response_for(&root, "GET", "/latency");
    assert_eq!(page.status, "200 OK");
    assert!(String::from_utf8(page.body)
        .unwrap()
        .contains("color-scheme:dark"));
    let data = super::response_for(&root, "GET", "/latency/data");
    assert_eq!(data.status, "200 OK");
    let body: serde_json::Value = serde_json::from_slice(&data.body).unwrap();
    assert!(body["metrics"].is_null());
    assert!(!root.exists());
    assert_eq!(
        super::response_for(&root, "POST", "/latency/data").status,
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
