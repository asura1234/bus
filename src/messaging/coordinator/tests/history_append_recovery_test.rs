//! An archive append cut short (a full disk, a crash) must not leave
//! `history.jsonl` unreadable once later appends land after it.
use super::*;
use crate::messaging::model::ARCHIVE_AFTER_MS;
use crate::messaging::storage::state_store::SaveStage;

/// Sends and settles `count` messages to `agent`; returns their request ids.
fn settled(worker: &mut Worker, room: RoomId, agent: AgentId, count: usize) -> Vec<RequestId> {
    (0..count)
        .map(|index| {
            let request = queue(worker, room, agent, &format!("task {index}"));
            let mut state = worker.state.clone();
            state.finish_for_test(request, &format!("reply {index}"), io::now_ms());
            worker.save(state).unwrap();
            request
        })
        .collect()
}

/// When the archive may take everything settled so far.
fn later() -> u64 {
    io::now_ms() + ARCHIVE_AFTER_MS + 1
}

/// Leaves the start of an archive line on disk, as a write cut short by a full
/// disk or a crash does.
fn leave_partial_line(dir: &Path) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("history.jsonl"))
        .unwrap();
    file.write_all(br#"{"request":{"id":"#).unwrap();
}

#[test]
fn an_archive_append_cut_short_by_a_full_disk_still_loads_after_the_retry() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let requests = settled(&mut worker, room, agent, 3);
    // write_all wrote part of the batch, then the disk filled up.
    leave_partial_line(&dir);
    worker.store.fail_history_once();
    assert!(worker.archive_history(later()).is_err());
    // Space frees up: the paused store recovers and the next pass archives.
    let (events, _) = mpsc::channel();
    worker.retry_storage(
        std::time::Instant::now() + std::time::Duration::from_secs(60),
        &events,
    );
    assert!(worker.storage_pause.is_none(), "storage recovered");
    worker.archive_history(later()).unwrap();
    drop(worker);
    let reloaded = JsonStore::new(dir.join("state.json")).load();
    assert!(
        reloaded.is_ok(),
        "the Bus must still open after a retried append: {:?}",
        reloaded.err()
    );
    let reloaded = reloaded.unwrap().unwrap();
    assert!(requests
        .iter()
        .all(|request| reloaded.request(*request).is_some()));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_torn_archive_line_left_by_a_crash_survives_a_failed_startup_compaction() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    settled(&mut worker, room, agent, 2);
    worker.archive_history(later()).unwrap();
    drop(worker);
    // The process died mid-append.
    leave_partial_line(&dir);
    let mut reopened = reopen_saved(&dir);
    // Worker::run only logs a failed compaction and keeps serving.
    reopened.store.fail_once_at(SaveStage::Replace);
    assert!(reopened.store.compact_history().is_err());
    let more = settled(&mut reopened, room, agent, 2);
    reopened.archive_history(later()).unwrap();
    drop(reopened);
    let reloaded = JsonStore::new(dir.join("state.json")).load();
    assert!(
        reloaded.is_ok(),
        "the Bus must still open after appending past a torn line: {:?}",
        reloaded.err()
    );
    let reloaded = reloaded.unwrap().unwrap();
    assert!(more
        .iter()
        .all(|request| reloaded.request(*request).is_some()));
    std::fs::remove_dir_all(dir).unwrap();
}
