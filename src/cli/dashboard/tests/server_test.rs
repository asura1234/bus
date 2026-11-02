use super::*;
use crate::messaging::model::{BusState, Request};
use crate::messaging::storage::state_store::JsonStore;
use serde_json::{json, Value};

fn persisted_fixture(root: &Path) {
    let cases: Vec<Value> = serde_json::from_str(include_str!("fixtures/requests.json")).unwrap();
    let mut doc = serde_json::to_value(BusState::new()).unwrap();
    for (index, case) in cases.iter().enumerate().take(5) {
        doc["requests"][(100 + index).to_string()] = case["record"].clone();
    }
    let state: BusState = serde_json::from_value(doc).unwrap();
    JsonStore::new(root.join("state.json"))
        .save(&state)
        .unwrap();
    let archive: Request = serde_json::from_value(cases[15]["record"].clone()).unwrap();
    let line = serde_json::to_string(&json!({"request":archive})).unwrap();
    std::fs::write(
        root.join("history.jsonl"),
        format!("{line}\n{line}\n{{unfinished"),
    )
    .unwrap();
}

fn files(root: &Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|item| {
            let path = item.unwrap().path();
            let data = std::fs::read(&path).unwrap();
            (path, data)
        })
        .collect();
    out.sort();
    out
}

// Catches dropping archived attempts, duplicate archive counts, writing migration
// or registry files, and leaking message/reply bodies through the HTTP boundary.
#[test]
fn data_route_reads_live_and_archived_requests_without_writes_or_message_content() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-data");
    std::fs::create_dir_all(&root).unwrap();
    persisted_fixture(&root);
    let before = files(&root);
    let response = response_for(&root, "GET", "/data");
    assert_eq!(response.status, "200 OK");
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    assert_eq!(body["metrics"]["all"]["counts"]["attempts"], 6);
    assert_eq!(body["metrics"]["all"]["counts"]["external"], 1);
    assert!(body["timezone"].as_str().unwrap().contains("local"));
    let text = String::from_utf8(response.body).unwrap();
    assert!(!text.contains("redacted request"));
    assert!(!text.contains("redacted reply"));
    assert_eq!(files(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_or_corrupt_storage_serves_empty_data_without_creating_state() {
    let root = crate::utils::test_temp::unique_temp_path("dashboard-empty");
    let missing = response_for(&root, "GET", "/data");
    assert!(!root.exists());
    let body: Value = serde_json::from_slice(&missing.body).unwrap();
    assert!(body["metrics"].is_null());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("state.json"), "broken").unwrap();
    let before = files(&root);
    let corrupt = response_for(&root, "GET", "/data");
    let body: Value = serde_json::from_slice(&corrupt.body).unwrap();
    assert!(body["metrics"].is_null());
    assert_eq!(files(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn http_only_serves_known_get_routes_and_does_not_read_arbitrary_paths() {
    let root = Path::new("/nonexistent-dashboard-test");
    assert_eq!(
        response_for(root, "POST", "/data").status,
        "405 Method Not Allowed"
    );
    assert_eq!(
        response_for(root, "GET", "/../state.json").status,
        "404 Not Found"
    );
    let page = response_for(root, "GET", "/");
    assert_eq!(page.status, "200 OK");
    assert_eq!(page.content_type, "text/html; charset=utf-8");
}

#[test]
fn dashboard_is_dev_only_and_session_selection_does_not_write_the_registry() {
    use crate::cli::session_pick::parse_invocation;
    let args = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(parse_invocation(&args(&["dashboard"])).is_err());
    let invocation = parse_invocation(&args(&["--dev", "dashboard"])).unwrap();
    assert_eq!(format!("{:?}", invocation.action), "Dashboard");
    assert!(parse_invocation(&args(&["--dev", "dashboard", "extra"])).is_err());
    let root = crate::utils::test_temp::unique_temp_path("dashboard-registry");
    let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(root.clone());
    let session = registry.create().unwrap();
    std::fs::remove_file(root.join("session-registry.lock")).unwrap();
    let before = std::fs::metadata(root.join("last-session"))
        .unwrap()
        .modified()
        .unwrap();
    let selected = crate::cli::last_session(&registry, &invocation.action).unwrap();
    assert_eq!(selected.id, session.id);
    assert_eq!(
        std::fs::metadata(root.join("last-session"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    assert!(!root.join("session-registry.lock").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn data_refresh_replays_newer_submit_journal_entries() {
    use crate::messaging::model::Provider;
    use crate::messaging::storage::state_store::JournalOp;
    let root = crate::utils::test_temp::unique_temp_path("dashboard-journal");
    std::fs::create_dir_all(&root).unwrap();
    let mut state = BusState::new();
    let room = state.create_room("journal").unwrap();
    let agent = state
        .create_agent(room, "codex", Provider::Codex, "/scratch".into(), None)
        .unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let store = JsonStore::new(root.join("state.json"));
    store.save(&state).unwrap();
    let first: Value = serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    assert_eq!(first["metrics"]["all"]["counts"]["attempts"], 0);
    store
        .journal(JournalOp::DraftText {
            room,
            text: "never expose this draft".into(),
        })
        .unwrap();
    store
        .journal(JournalOp::Submit {
            room,
            queued: true,
            now_ms: 1793577600000,
        })
        .unwrap();
    let before = files(&root);
    let refreshed: Value =
        serde_json::from_slice(&response_for(&root, "GET", "/data").body).unwrap();
    assert_eq!(refreshed["metrics"]["all"]["counts"]["attempts"], 1);
    assert_eq!(
        refreshed["metrics"]["providers"]["codex"]["counts"]["open"],
        1
    );
    assert_eq!(files(&root), before);
    std::fs::remove_dir_all(root).unwrap();
}
