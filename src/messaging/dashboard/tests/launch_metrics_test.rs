use super::*;
use crate::messaging::model::BusState;
use serde_json::{json, Value};

fn launch_fixture() -> BusState {
    let fixture: Value =
        serde_json::from_str(include_str!("../../model/tests/fixtures/launches.json")).unwrap();
    let mut doc = serde_json::to_value(BusState::new()).unwrap();
    doc["launch_attempts"] = fixture["attempts"].clone();
    doc["launch_tracking_started_at_ms"] = fixture["tracking_started_at_ms"].clone();
    serde_json::from_value(doc).unwrap()
}

#[test]
fn launch_metrics_data_route_counts_every_attempt_and_keeps_failure_evidence() {
    let root = crate::utils::test_temp::unique_temp_path("launch-dashboard-data");
    std::fs::create_dir_all(&root).unwrap();
    JsonStore::new(root.join("state.json"))
        .save(&launch_fixture())
        .unwrap();
    let before = super::tests::files(&root);
    let response = response_for(&root, "GET", "/data");
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    let data = &body["launches"];
    let after = super::tests::files(&root);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(response.status, "200 OK");
    assert_eq!(data["all"]["counts"]["attempts"], 7);
    for (key, n) in [
        ("usable", 3),
        ("failed", 2),
        ("pending", 1),
        ("cancelled", 1),
    ] {
        assert_eq!(data["all"]["counts"][key], n, "{key}");
    }
    assert_eq!(
        data["all"]["rates"]["launch_success"],
        json!({"numerator":3,"denominator":5,"percent":60.0})
    );
    assert_eq!(data["providers"].as_object().unwrap().len(), 6);
    assert_eq!(data["providers"]["claude"]["counts"]["usable"], 2);
    assert_eq!(data["providers"]["claude-ds"]["counts"]["attempts"], 0);
    assert_eq!(data["providers"]["kimi"]["counts"]["failed"], 1);
    assert!(data["providers"]["cursor"]["rates"]["launch_success"]["percent"].is_null());
    let failures = data["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 2);
    let missing = failures.iter().find(|f| f["id"] == 4).unwrap();
    assert_eq!(missing["name"], "missing-kimi");
    assert!(missing["agent_id"].is_null());
    assert_eq!(missing["cause"], "missing_executable");
    assert!(missing["log_reference"]
        .as_str()
        .unwrap()
        .contains("attempt_id=4"));
    assert!(missing["record_reference"]
        .as_str()
        .unwrap()
        .contains("launch_attempts"));
    assert_eq!(missing["inspect_command"], "bus --paths");
    assert_eq!(before, after);
}

#[test]
fn launch_metrics_data_route_never_backfills_legacy_successes_or_writes_files() {
    let root = crate::utils::test_temp::unique_temp_path("launch-dashboard-legacy");
    std::fs::create_dir_all(&root).unwrap();
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    state
        .create_agent(
            room,
            "existing",
            crate::messaging::model::Provider::Codex,
            "/tmp".into(),
            None,
        )
        .unwrap();
    JsonStore::new(root.join("state.json"))
        .save(&state)
        .unwrap();
    let before = super::tests::files(&root);
    let response = response_for(&root, "GET", "/data");
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    let after = super::tests::files(&root);
    std::fs::remove_dir_all(root).unwrap();
    assert_eq!(body["launches"]["all"]["counts"]["attempts"], 0);
    assert!(body["launches"]["tracking_started_at_ms"].is_null());
    assert!(body["launches"]["all"]["rates"]["launch_success"]["percent"].is_null());
    assert_eq!(before, after);
}

// Hard-coded fixture outcomes are the oracle; use the already-tested local clock
// only to make expected bucket labels independent of the host's timezone.
#[test]
fn launch_metrics_hourly_counts_use_local_attempt_starts_instead_of_settlement_times() {
    let root = crate::utils::test_temp::unique_temp_path("launch-dashboard-hours");
    std::fs::create_dir_all(&root).unwrap();
    JsonStore::new(root.join("state.json"))
        .save(&launch_fixture())
        .unwrap();
    let body: Value = serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    std::fs::remove_dir_all(root).unwrap();
    let mut expected = std::collections::BTreeMap::<String, [u64; 4]>::new();
    for (at, outcomes) in [
        (1793581200000, [1, 0, 0, 0]),
        (1793581800000, [1, 0, 0, 0]),
        (1793584799000, [1, 0, 0, 0]),
        (1793583000000, [0, 1, 0, 0]),
        (1793584800000, [0, 1, 0, 0]),
        (1793586600000, [0, 0, 1, 0]),
        (1793588399000, [0, 0, 0, 1]),
    ] {
        let bucket = expected.entry(metrics::local_hour(at).label).or_default();
        for (count, n) in bucket.iter_mut().zip(outcomes) {
            *count += n;
        }
    }
    let hours = body["launches"]["all"]["hours"]
        .as_array()
        .expect("hourly launch buckets");
    assert_eq!(hours.len(), expected.len());
    for hour in hours {
        let counts = expected.get(hour["hour"].as_str().unwrap()).unwrap();
        for (key, n) in ["usable", "failed", "pending", "cancelled"]
            .into_iter()
            .zip(counts)
        {
            assert_eq!(hour["counts"][key], *n, "{key}");
        }
        assert_eq!(hour["counts"]["attempts"], counts.iter().sum::<u64>());
        assert_eq!(hour["rates"]["launch_success"]["numerator"], counts[0]);
        assert_eq!(
            hour["rates"]["launch_success"]["denominator"],
            counts[0] + counts[1]
        );
    }
}

#[test]
fn launch_dashboard_record_reference_resolves_in_saved_state_json() {
    let root = crate::utils::test_temp::unique_temp_path("launch-dashboard-pointer");
    std::fs::create_dir_all(&root).unwrap();
    let mut state = BusState::new();
    let room = state.create_room("work").unwrap();
    let id = state.begin_launch_attempt(
        "missing",
        crate::messaging::model::Provider::Kimi,
        room,
        crate::messaging::model::LaunchKind::Create,
        1_700_000_000_000,
    );
    assert!(state.settle_launch_attempt(
        id,
        &crate::messaging::model::AgentRuntimeIdentity::default(),
        crate::messaging::model::LaunchOutcome::Failed,
        Some("missing_executable"),
        1_700_000_000_100,
    ));
    JsonStore::new(root.join("state.json"))
        .save(&state)
        .unwrap();
    let response = response_for(&root, "GET", "/data");
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    let file: Value =
        serde_json::from_slice(&std::fs::read(root.join("state.json")).unwrap()).unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(response.status, "200 OK");
    let reference = body["launches"]["failures"][0]["record_reference"]
        .as_str()
        .unwrap();
    let pointer = reference
        .strip_prefix("state.json#")
        .expect("record reference should be a state.json JSON pointer");
    assert!(
        file.pointer(pointer).is_some_and(|record| {
            record["id"].as_u64() == Some(id) && record["cause"] == "missing_executable"
        }),
        "record reference {reference} did not resolve to the failed attempt in state.json"
    );
}

#[test]
fn launch_metrics_legacy_dialog_timeouts_are_inconclusive_without_rewriting_history() {
    let root = crate::utils::test_temp::unique_temp_path("launch-legacy-dialog-timeout");
    std::fs::create_dir_all(&root).unwrap();
    let mut saved = serde_json::to_value(launch_fixture()).unwrap();
    saved["launch_attempts"]["3"]["outcome"] = json!("failed");
    saved["launch_attempts"]["3"]["cause"] = json!("dialog_timeout");
    let store = JsonStore::new(root.join("state.json"));
    store.save(&serde_json::from_value(saved).unwrap()).unwrap();
    let before = super::tests::files(&root);
    let body: Value = serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    let after = super::tests::files(&root);
    let retained = serde_json::to_value(store.load().unwrap().unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();

    let launches = &body["launches"];
    assert_eq!(launches["all"]["counts"]["attempts"], 7);
    assert_eq!(launches["all"]["counts"]["usable"], 2);
    assert_eq!(launches["all"]["counts"]["failed"], 2);
    assert_eq!(launches["all"]["counts"]["pending"], 2);
    assert_eq!(launches["all"]["rates"]["launch_success"]["denominator"], 4);
    let codex = &launches["providers"]["codex"];
    assert_eq!(codex["counts"]["failed"], 0);
    assert_eq!(codex["counts"]["pending"], 1);
    assert!(codex["rates"]["launch_success"]["percent"].is_null());
    for series in [&launches["all"], codex] {
        assert!(series["counts"]["failure_reasons"]
            .get("dialog_timeout")
            .is_none());
        for hour in series["hours"].as_array().unwrap() {
            assert!(hour["counts"]["failure_reasons"]
                .get("dialog_timeout")
                .is_none());
        }
    }
    assert!(launches["failures"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["id"] != 3));
    assert_eq!(before, after);
    assert_eq!(retained["launch_attempts"]["3"]["outcome"], "failed");
    assert_eq!(retained["launch_attempts"]["3"]["cause"], "dialog_timeout");
}
