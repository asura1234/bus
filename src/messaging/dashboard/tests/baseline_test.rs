use super::*;
use crate::messaging::model::{BusState, LaunchKind, Provider, Request, RequestId};
use crate::messaging::storage::state_store::JsonStore;
use serde_json::Value;
use std::path::Path;

fn request_at(sent: u64, id: u64) -> Request {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/recorded.json")).unwrap();
    let mut record: Request = serde_json::from_value(cases[0]["record"].clone()).unwrap();
    record.id = RequestId(id);
    record.prompt.submitted_at_ms = sent;
    record.latency.accepted_at_ms = Some(sent);
    record.latency.picked_at_ms = Some(sent + 1_000);
    record
}

fn state_with(records: impl IntoIterator<Item = Request>) -> BusState {
    let mut document = serde_json::to_value(BusState::new()).unwrap();
    for record in records {
        document["requests"][record.id.0.to_string()] = serde_json::to_value(&record).unwrap();
    }
    serde_json::from_value(document).unwrap()
}

#[test]
fn metrics_keep_events_after_the_baseline_and_drop_earlier_ones() {
    let state = state_with([
        request_at(1_000, 1),
        request_at(3_000, 2),
        request_at(5_000, 3),
    ]);
    let report = metrics::snapshot_since(&state, metrics::local_hour, Some(3_000));
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["all"]["counts"]["attempts"], 1);
    assert_eq!(json["all"]["accepted_to_picked"]["n"], 1);
    let failures = json["failures"].as_array().unwrap();
    assert!(failures
        .iter()
        .all(|failure| failure["sent_at_ms"] == 5_000));
    let open = metrics::snapshot_since(&state, metrics::local_hour, None);
    let open_json = serde_json::to_value(&open).unwrap();
    assert_eq!(open_json["all"]["counts"]["attempts"], 3);

    let mut launches = BusState::new();
    let room = launches.create_room("dash").unwrap();
    launches.begin_launch_attempt("old", Provider::Grok, room, LaunchKind::Create, 1_000);
    launches.begin_launch_attempt("edge", Provider::Grok, room, LaunchKind::Create, 3_000);
    launches.begin_launch_attempt("new", Provider::Grok, room, LaunchKind::Create, 5_000);
    let launch = launch_metrics::snapshot_since(&launches, metrics::local_hour, Some(3_000));
    let launch_json = serde_json::to_value(&launch).unwrap();
    assert_eq!(launch_json["all"]["counts"]["attempts"], 1);
    assert_eq!(launch_json["failures"].as_array().unwrap().len(), 0);
}

#[test]
fn reset_persists_across_a_fresh_read_and_a_second_reset_advances_it() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-baseline");
    std::fs::create_dir_all(&root).unwrap();
    let state = state_with([request_at(1_000, 1), request_at(9_000, 2)]);
    JsonStore::new(root.join("state.json"))
        .save(&state)
        .unwrap();
    let archived = request_at(500, 9);
    let line = serde_json::to_string(&serde_json::json!({"request": archived})).unwrap();
    std::fs::write(root.join("history.jsonl"), format!("{line}\n")).unwrap();
    let before_state = std::fs::read(root.join("state.json")).unwrap();
    let before_history = std::fs::read(root.join("history.jsonl")).unwrap();

    assert_eq!(
        response_for(&root, "POST", "/reset-stats").status,
        "400 Bad Request"
    );
    assert!(!baseline::stats_path(&root).exists());

    let first = baseline::reset(&root, 4_000).unwrap();
    assert_eq!(first, 4_000);
    // A new read is what a restarted dashboard does: nothing is cached.
    assert_eq!(baseline::since_ms(&root), Some(4_000));
    let body: Value = serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    assert_eq!(body["stats_since_ms"], 4_000);
    assert_eq!(body["stats_since"], baseline::label(4_000));
    assert!(body["stats_since"].as_str().unwrap().starts_with("since "));
    assert_eq!(body["metrics"]["all"]["counts"]["attempts"], 1);
    assert_eq!(
        std::fs::read(root.join("state.json")).unwrap(),
        before_state
    );
    assert_eq!(
        std::fs::read(root.join("history.jsonl")).unwrap(),
        before_history
    );

    let same_clock = baseline::reset(&root, 4_000).unwrap();
    assert_eq!(same_clock, 4_001);
    let later = baseline::reset(&root, 8_000).unwrap();
    assert_eq!(later, 8_000);
    assert_eq!(baseline::since_ms(&root), Some(8_000));
    let again: Value = serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    assert_eq!(again["metrics"]["all"]["counts"]["attempts"], 1);
    assert_eq!(again["stats_since_ms"], 8_000);
    assert_eq!(
        std::fs::read(root.join("state.json")).unwrap(),
        before_state
    );
    assert_eq!(
        std::fs::read(root.join("history.jsonl")).unwrap(),
        before_history
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn header_renders_the_since_time_and_confirm_guards_reset() {
    let page =
        String::from_utf8(response_for(Path::new("/nonexistent-dashboard-page"), "GET", "/").body)
            .unwrap();
    let header = page.split_once("</header>").unwrap().0;
    assert!(header.contains("id=\"stats-since\""));
    assert!(header.contains("id=\"reset-stats\""));
    assert!(header.contains(">Reset stats<"));
    assert!(page.contains("latest?.stats_since"));
    assert!(page.contains("/reset-stats?confirm=1"));
    assert!(page.contains("Reset stats?"));
    assert!(page.contains("Messages, replies and history stay."));
}
