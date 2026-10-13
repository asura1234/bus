//! A version 1 `state.json` still loads; what it already spent must stay spent.
use super::*;

/// The fixture's saved document, rewritten as a version 1 build saved it.
fn as_version_one(dir: &Path, callback_ids: &[&str], fingerprints: &[&str]) -> serde_json::Value {
    let mut document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("state.json")).unwrap()).unwrap();
    document["version"] = json!(1);
    let state = document["state"].as_object_mut().unwrap();
    state.remove("consumed_callbacks");
    state.remove("spent_dialog_answers");
    state.insert("consumed_callback_ids".into(), json!(callback_ids));
    state.insert("consumed_dialog_fingerprints".into(), json!(fingerprints));
    document
}

#[test]
fn a_version_one_state_keeps_its_consumed_callback_ids() {
    let (worker, _, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let document = as_version_one(&dir, &["applied-callback"], &[]);
    drop(worker);
    let (worker, upgraded) = open_saved_document(document);
    drop(worker);
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(upgraded.join("state.json")).unwrap()).unwrap();
    assert!(
        saved["state"]["consumed_callbacks"]
            .get("applied-callback")
            .is_some(),
        "an applied callback id is forgotten on upgrade, so a re-spooled duplicate applies again: {}",
        saved["state"]["consumed_callbacks"]
    );
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(upgraded).unwrap();
}

#[test]
fn a_version_one_state_keeps_its_spent_dialog_fingerprints() {
    let (worker, _, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let document = as_version_one(&dir, &[], &["d1.payload.spentdigest"]);
    drop(worker);
    let (worker, upgraded) = open_saved_document(document);
    assert!(
        worker.state.dialog_answer_spent("spentdigest"),
        "a fingerprint spent before the upgrade answers a dialog again after it"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(upgraded).unwrap();
}

#[test]
fn a_reply_that_settled_minutes_ago_stays_in_history_however_long_it_queued() {
    use crate::messaging::model::{ARCHIVE_AFTER_MS, HISTORY_RETENTION_MS};
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let request = queue(&mut worker, room, agent, "long task");
    // The agent worked on it for more than a week, then replied.
    let replied_at = io::now_ms() + HISTORY_RETENTION_MS + 60_000;
    let mut state = worker.state.clone();
    state.finish_for_test(request, "finally done", replied_at);
    worker.save(state).unwrap();
    worker
        .archive_history(replied_at + ARCHIVE_AFTER_MS + 1)
        .unwrap();
    assert!(
        worker.state.request(request).is_some(),
        "a reply received {} minutes ago vanished from history",
        (ARCHIVE_AFTER_MS + 1) / 60_000
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
