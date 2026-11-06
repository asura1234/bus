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
        saved["launch_tracking_started_at_ms"],
        fixture["tracking_started_at_ms"]
    );
}
