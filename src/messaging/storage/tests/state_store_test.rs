use std::{fs, path::PathBuf};

use super::*;
use crate::messaging::model::{
    AgentRuntimeIdentity, CallbackDisposition, CallbackRejection, Provider, ProviderCallback,
    RequestPhase, SubmissionOutcome,
};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bus-store-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn atomic_store_round_trips_versioned_state_and_ignores_interrupted_temp_file() {
    let dir = temp_dir("roundtrip");
    let path = dir.join("state.json");
    let store = JsonStore::new(path.clone());
    let mut state = BusState::new();
    state.create_room("durable").expect("room");
    store.save(&state).expect("save");
    fs::write(dir.join("state.json.tmp-interrupted"), b"{not json").expect("interrupted temp");

    let loaded = store.load().expect("load").expect("state");
    assert_eq!(loaded, state);
    let document: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("read")).expect("json");
    assert_eq!(document["version"], STORE_VERSION);
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn corrupt_or_unknown_version_state_is_explicit_and_never_overwritten() {
    let dir = temp_dir("corrupt");
    let path = dir.join("state.json");
    fs::write(&path, b"{broken").expect("corrupt");
    let store = JsonStore::new(path.clone());
    assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
    assert!(matches!(
        store.save(&BusState::new()),
        Err(StoreError::ExistingStateUnreadable { .. })
    ));
    assert_eq!(fs::read(&path).expect("unchanged"), b"{broken");

    fs::write(&path, br#"{"version":999,"state":{}}"#).expect("version");
    assert!(matches!(
        store.load(),
        Err(StoreError::UnsupportedVersion { found: 999, .. })
    ));
    assert!(matches!(
        store.save(&BusState::new()),
        Err(StoreError::ExistingStateUnreadable { .. })
    ));
    assert_eq!(
        fs::read(&path).expect("unchanged version"),
        br#"{"version":999,"state":{}}"#
    );
    for invalid in [
        br#"{"state":{}}"#.as_slice(),
        br#"{"version":1,"state":null}"#.as_slice(),
    ] {
        fs::write(&path, invalid).expect("invalid document");
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
        assert!(matches!(
            store.save(&BusState::new()),
            Err(StoreError::ExistingStateUnreadable { .. })
        ));
        assert_eq!(
            fs::read(&path).expect("unchanged invalid document"),
            invalid
        );
    }
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn unreadable_existing_state_is_not_replaced_or_given_a_temp_file() {
    let dir = temp_dir("unreadable");
    let path = dir.join("state.json");
    fs::create_dir(&path).unwrap();
    let store = JsonStore::new(path.clone());
    assert!(matches!(store.load(), Err(StoreError::Io { .. })));
    assert!(matches!(
        store.save(&BusState::new()),
        Err(StoreError::ExistingStateUnreadable { .. })
    ));
    assert!(path.is_dir());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn state_store_refuses_real_and_dangling_symlinks() {
    let dir = temp_dir("symlinks");
    let target = dir.join("target.json");
    JsonStore::new(target.clone())
        .save(&BusState::new())
        .unwrap();
    let original = fs::read(&target).unwrap();
    for (name, target) in [
        ("real-link", target.clone()),
        ("dangling-link", dir.join("missing.json")),
    ] {
        let link = dir.join(name);
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(matches!(
            JsonStore::new(link.clone()).save(&BusState::new()),
            Err(StoreError::UnsafePath(_))
        ));
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    assert_eq!(fs::read(&target).unwrap(), original);
    assert!(!dir.join("missing.json").exists());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reload_preserves_uncertain_ownership_queue_and_consumed_callbacks() {
    let dir = temp_dir("uncertain");
    let store = JsonStore::new(dir.join("state.json"));
    let mut state = BusState::new();
    let room = state.create_room("durable").expect("room");
    let agent = state
        .create_agent(
            room,
            "builder",
            Provider::Codex,
            PathBuf::from("/repo"),
            Some("main".into()),
        )
        .expect("agent");
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch-codex".into()),
                terminal_id: None,
                pane_id: None,
                session_id: None,
            },
        )
        .expect("identity");
    state.set_draft_text(room, "first").expect("first text");
    state
        .set_draft_recipients(room, [agent])
        .expect("first recipient");
    let first = state.submit_draft(room, 1).expect("first submit")[0];
    state.set_draft_text(room, "second").expect("second text");
    state
        .set_draft_recipients(room, [agent])
        .expect("second recipient");
    let second = state.submit_draft(room, 2).expect("second submit")[0];
    state
        .begin_submission(first, "launch-codex", 10)
        .expect("begin first");
    state
        .record_submission(
            first,
            SubmissionOutcome::Uncertain {
                message: "response lost".into(),
            },
        )
        .expect("uncertain");
    let consumed = ProviderCallback::final_event(
        "consumed-before-save",
        11,
        agent,
        "launch-codex",
        "provider-session",
        "turn-1",
        "first",
        "unbound",
    );
    assert_eq!(
        state.accept_callback(consumed.clone()),
        CallbackDisposition::Rejected(CallbackRejection::UnboundFinal)
    );
    store.save(&state).expect("save");

    let mut loaded = store.load().expect("load").expect("state");
    assert_eq!(
        loaded.agent(agent).expect("agent").current_request,
        Some(first)
    );
    let request = loaded.request(first).expect("first request");
    assert_eq!(request.phase, RequestPhase::Submitting);
    assert!(request.uncertain_outcome);
    assert_eq!(loaded.next_queued_request(agent), None);
    assert_eq!(loaded.queued_requests(agent), &[second]);
    assert_eq!(
        loaded.accept_callback(consumed),
        CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback)
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn saved_sound_orchestrator_compactions_notes_and_draft_reload() {
    use crate::messaging::model::RoomKind;

    let dir = temp_dir("compat-fields");
    let store = JsonStore::new(dir.join("state.json"));
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").expect("room");
    state.set_room_notes(work, "keep-notes").expect("notes");
    state.set_draft_text(work, "keep-draft").expect("draft");
    state.set_room_sound(master, false).expect("master sound");
    state.set_room_sound(work, true).expect("work sound");
    let agent = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            PathBuf::from("/repo"),
            None,
        )
        .expect("agent");
    state.bind_orchestrator(agent, work).expect("assign");
    state.record_compaction(agent, 50).expect("compaction");
    store.save(&state).expect("save");

    let mut loaded = store.load().expect("load").expect("state");
    assert_eq!(loaded.room(work).expect("work").notes, "keep-notes");
    assert_eq!(loaded.room(work).expect("work").draft.text, "keep-draft");
    assert!(!loaded.room(master).expect("master").sound_enabled());
    assert!(loaded.room(work).expect("work").sound_enabled());
    assert_eq!(loaded.agent(agent).expect("agent").orchestrates, Some(work));
    assert_eq!(loaded.agent(agent).expect("agent").compactions.count, 1);
    assert_eq!(
        loaded.agent(agent).expect("agent").compactions.last_at_ms,
        Some(50)
    );
    loaded.ensure_master_room();
    assert_eq!(loaded.agent(agent).expect("agent").orchestrates, Some(work));
    assert_eq!(
        loaded
            .rooms()
            .filter(|room| room.kind == RoomKind::Master)
            .count(),
        1
    );
    fs::remove_dir_all(dir).expect("cleanup");
}

fn room_with_recipient(state: &mut BusState) -> crate::messaging::model::RoomId {
    let room = state.create_room("work").expect("room");
    let agent = state
        .create_agent(room, "author", Provider::Codex, "/project".into(), None)
        .expect("agent");
    state
        .set_draft_recipients(room, [agent])
        .expect("recipients");
    room
}

#[test]
fn journaled_drafts_and_sends_survive_reload_without_rewriting_state() {
    let dir = temp_dir("journal");
    let path = dir.join("state.json");
    let store = JsonStore::new(path.clone());
    let mut state = BusState::new();
    let room = room_with_recipient(&mut state);
    store.save(&state).expect("save");
    let saved = fs::read(&path).expect("state");

    for (text, send) in [("first", true), ("sec", false), ("second", false)] {
        state.set_draft_text(room, text).expect("draft");
        store
            .journal(JournalOp::DraftText {
                room,
                text: text.into(),
            })
            .expect("journal draft");
        if send {
            state.submit_draft(room, 7).expect("send");
            store
                .journal(JournalOp::Submit {
                    room,
                    queued: false,
                    now_ms: 7,
                })
                .expect("journal send");
        }
    }

    assert_eq!(
        fs::read(&path).expect("state"),
        saved,
        "state.json untouched"
    );
    let reloaded = JsonStore::new(path.clone())
        .load()
        .expect("load")
        .expect("state");
    assert_eq!(reloaded, state);
    assert_eq!(reloaded.room(room).expect("room").draft.text, "second");
    assert_eq!(reloaded.requests().count(), 1);
    let mut base = BusState::new();
    room_with_recipient(&mut base);
    assert_eq!(JsonStore::new(path).load_base().expect("base"), Some(base));
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn a_full_save_folds_the_journal_and_a_stale_journal_is_never_replayed() {
    let dir = temp_dir("journal-fold");
    let path = dir.join("state.json");
    let journal = dir.join("state.journal.json");
    let store = JsonStore::new(path.clone());
    let mut state = BusState::new();
    let room = room_with_recipient(&mut state);
    store.save(&state).expect("save");
    state.set_draft_text(room, "sent once").expect("draft");
    store
        .journal(JournalOp::DraftText {
            room,
            text: "sent once".into(),
        })
        .expect("journal draft");
    state.submit_draft(room, 7).expect("send");
    store
        .journal(JournalOp::Submit {
            room,
            queued: false,
            now_ms: 7,
        })
        .expect("journal send");
    let stale = fs::read(&journal).expect("journal");

    store.save(&state).expect("fold");
    assert!(!journal.exists());
    assert!(!store.has_unfolded_journal());
    // A crash can leave the journal behind the save that folded it.
    fs::write(&journal, stale).expect("stale journal");
    let reloaded = JsonStore::new(path).load().expect("load").expect("state");
    assert_eq!(reloaded, state, "folded entries replay at most once");
    assert_eq!(reloaded.requests().count(), 1);
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn a_failed_journal_write_is_not_recorded() {
    let dir = temp_dir("journal-fail");
    let path = dir.join("state.json");
    let store = JsonStore::new(path.clone());
    let mut state = BusState::new();
    let room = room_with_recipient(&mut state);
    store.save(&state).expect("save");
    store.fail_once_at(SaveStage::TempSync);
    let op = JournalOp::DraftText {
        room,
        text: "lost".into(),
    };
    assert!(store.journal(op).unwrap_err().retryable_io());
    assert!(!store.has_unfolded_journal());
    assert_eq!(JsonStore::new(path).load().expect("load"), Some(state));
    fs::remove_dir_all(dir).expect("cleanup");
}

#[test]
fn a_state_file_changed_by_someone_else_is_checked_again_before_saving() {
    let dir = temp_dir("fingerprint");
    let path = dir.join("state.json");
    let store = JsonStore::new(path.clone());
    store.save(&BusState::new()).expect("save");
    fs::write(&path, b"{broken by another writer").expect("corrupt");
    assert!(matches!(
        store.save(&BusState::new()),
        Err(StoreError::ExistingStateUnreadable { .. })
    ));
    assert_eq!(
        fs::read(&path).expect("unchanged"),
        b"{broken by another writer"
    );
    fs::remove_dir_all(dir).expect("cleanup");
}
