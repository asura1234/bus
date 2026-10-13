//! Settled requests and old notices move to `history.jsonl`: durably before
//! they leave `state.json`, once each after a crash, and kept for 7 days.
use super::*;
use crate::messaging::model::{ARCHIVE_AFTER_MS, HISTORY_RETENTION_MS};
use crate::messaging::storage::state_store::SaveStage;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;

/// A reply as long as a typical worker report.
fn reply(index: usize) -> String {
    format!("reply {index}: ") + &"done; tests pass; see the diff. ".repeat(120)
}

/// Sends and settles `count` messages to `agent`; returns their request ids.
fn settled(worker: &mut Worker, room: RoomId, agent: AgentId, count: usize) -> Vec<RequestId> {
    (0..count)
        .map(|index| {
            let request = queue(worker, room, agent, &format!("task {index}"));
            let mut state = worker.state.clone();
            state.finish_for_test(request, &reply(index), io::now_ms());
            worker.save(state).unwrap();
            request
        })
        .collect()
}

/// When the archive may take everything settled so far.
fn later() -> u64 {
    io::now_ms() + ARCHIVE_AFTER_MS + 1
}

fn state_json_bytes(dir: &Path) -> u64 {
    std::fs::metadata(dir.join("state.json")).unwrap().len()
}

fn state_json_requests(dir: &Path) -> Vec<u64> {
    let document: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("state.json")).unwrap()).unwrap();
    document["state"]["requests"]
        .as_object()
        .unwrap()
        .keys()
        .map(|id| id.parse().unwrap())
        .collect()
}

fn history_lines(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("history.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn archived_request_ids(dir: &Path) -> Vec<u64> {
    history_lines(dir)
        .iter()
        .filter_map(|line| line["request"]["id"].as_u64())
        .collect()
}

#[test]
fn state_json_stays_bounded_however_many_requests_finish() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let first = settled(&mut worker, room, agent, 20);
    worker.archive_history(later()).unwrap();
    let after_twenty = state_json_bytes(&dir);
    let rest = settled(&mut worker, room, agent, 80);
    worker.archive_history(later()).unwrap();
    let after_hundred = state_json_bytes(&dir);
    assert!(
        after_hundred <= after_twenty + 2_048,
        "state.json grew from {after_twenty} to {after_hundred} bytes with 80 more finished requests"
    );
    // The history is still there for status reads and the room view.
    let last = *rest.last().unwrap();
    assert_eq!(
        worker.state.request(last).unwrap().phase,
        RequestPhase::Completed
    );
    let message = worker.state.request(first[0]).unwrap().prompt.id;
    assert_eq!(worker.control_message(message).unwrap()["complete"], true);
    assert_eq!(
        worker
            .state
            .requests()
            .filter(|request| request.room_id == room)
            .count(),
        100
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn settled_requests_leave_state_json_only_after_the_archive_is_durable() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let requests = settled(&mut worker, room, agent, 3);
    let before = worker.state.clone();
    worker.store.fail_history_once();
    assert!(worker.archive_history(later()).is_err());
    for request in &requests {
        assert!(
            state_json_requests(&dir).contains(&request.0),
            "an unarchived request stays in state.json"
        );
    }
    assert!(archived_request_ids(&dir).is_empty());
    assert_eq!(worker.state, before, "a failed archive changes nothing");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_crash_between_the_archive_and_the_save_recovers_each_request_once() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let requests = settled(&mut worker, room, agent, 3);
    // The archive append lands, then state.json cannot be written: on disk the
    // requests are now in both files.
    worker.store.fail_once_at(SaveStage::TempWrite);
    assert!(worker.archive_history(later()).is_err());
    let archived = archived_request_ids(&dir);
    assert!(requests.iter().all(|request| archived.contains(&request.0)));
    drop(worker);
    let mut reopened = reopen_saved(&dir);
    for request in &requests {
        assert_eq!(
            reopened
                .state
                .requests()
                .filter(|seen| seen.id == *request)
                .count(),
            1,
            "request {} is loaded exactly once",
            request.0
        );
    }
    reopened.archive_history(later()).unwrap();
    drop(reopened);
    let reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    for request in &requests {
        assert_eq!(
            reloaded
                .requests()
                .filter(|seen| seen.id == *request)
                .count(),
            1
        );
        assert!(!state_json_requests(&dir).contains(&request.0));
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// Moves the archived requests' send time `days` back, on disk only.
fn age_in_history(dir: &Path, requests: &[RequestId], days: u64) {
    let lines: Vec<String> = history_lines(dir)
        .into_iter()
        .map(|mut line| {
            let id = line["request"]["id"].as_u64();
            if id.is_some_and(|id| requests.iter().any(|request| request.0 == id)) {
                let sent = line["request"]["prompt"]["submitted_at_ms"]
                    .as_u64()
                    .unwrap();
                line["request"]["prompt"]["submitted_at_ms"] = json!(sent - days * DAY_MS);
            }
            line.to_string()
        })
        .collect();
    std::fs::write(dir.join("history.jsonl"), lines.join("\n") + "\n").unwrap();
}

#[test]
fn history_older_than_a_week_or_of_deleted_rooms_is_dropped_on_open() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let old = settled(&mut worker, room, agent, 2);
    let recent = settled(&mut worker, room, agent, 2);
    let mut state = worker.state.clone();
    let gone_room = state.create_room("gone").unwrap();
    let gone_agent = state
        .create_agent(gone_room, "temp", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker.save(state).unwrap();
    let gone = settled(&mut worker, gone_room, gone_agent, 1);
    worker.archive_history(later()).unwrap();
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteRoom(gone_room), &events)
        .unwrap();
    drop(worker);
    age_in_history(&dir, &old, HISTORY_RETENTION_MS / DAY_MS + 1);
    age_in_history(&dir, &recent, 1);

    let reopened = reopen_saved(&dir);
    let kept: Vec<_> = reopened
        .state
        .requests()
        .map(|request| request.id)
        .collect();
    assert!(
        old.iter().all(|request| !kept.contains(request)),
        "older than 7 days"
    );
    assert!(
        gone.iter().all(|request| !kept.contains(request)),
        "deleted room"
    );
    assert!(
        recent.iter().all(|request| kept.contains(request)),
        "6 days old stays"
    );
    assert!(reopened.store.compact_history().unwrap());
    let archived = archived_request_ids(&dir);
    assert_eq!(
        archived.len(),
        recent.len(),
        "the file keeps only what was loaded"
    );
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_crash_while_pruning_history_keeps_every_recent_entry() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let old = settled(&mut worker, room, agent, 2);
    let recent = settled(&mut worker, room, agent, 3);
    worker.archive_history(later()).unwrap();
    drop(worker);
    age_in_history(&dir, &old, HISTORY_RETENTION_MS / DAY_MS + 1);
    let before = std::fs::read(dir.join("history.jsonl")).unwrap();

    let reopened = reopen_saved(&dir);
    reopened.store.fail_once_at(SaveStage::Replace);
    assert!(reopened.store.compact_history().is_err());
    drop(reopened);
    assert_eq!(
        std::fs::read(dir.join("history.jsonl")).unwrap(),
        before,
        "an interrupted prune leaves the old file whole"
    );
    let reloaded = JsonStore::new(dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert!(recent
        .iter()
        .all(|request| reloaded.request(*request).is_some()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_snapshot_shares_the_archive_instead_of_copying_it() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    settled(&mut worker, room, agent, 5);
    worker.archive_history(later()).unwrap();
    assert_eq!(worker.state.history().len(), 5);
    let snapshot = worker.snapshot();
    assert!(Arc::ptr_eq(
        snapshot.state.history(),
        worker.state.history()
    ));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
