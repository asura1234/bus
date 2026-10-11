use super::*;

#[test]
fn dev_room_notes_replace_and_clear_the_room_notes() {
    let (mut worker, room, _agent, dir) = fixture();
    let set = call(
        &mut worker,
        "notes-1",
        "room.notes",
        json!({"room":"test","text":"Goal\nNon-goals"}),
    );
    assert!(set.ok, "{set:?}");
    assert_eq!(worker.state.room(room).unwrap().notes, "Goal\nNon-goals");
    let state = call(&mut worker, "state-1", "state", json!({}));
    // MASTER is listed first; the fixture room follows.
    assert_eq!(state.result["rooms"][1]["notes"], "Goal\nNon-goals");
    let cleared = call(
        &mut worker,
        "notes-2",
        "room.notes",
        json!({"room":room.0.to_string(),"text":""}),
    );
    assert!(cleared.ok, "{cleared:?}");
    assert_eq!(worker.state.room(room).unwrap().notes, "");
    let missing = call(&mut worker, "notes-3", "room.notes", json!({"room":"test"}));
    assert!(!missing.ok);
    let unknown = call(
        &mut worker,
        "notes-4",
        "room.notes",
        json!({"room":"nope","text":"x"}),
    );
    assert!(!unknown.ok);
    let master = call(
        &mut worker,
        "notes-5",
        "room.notes",
        json!({"room":"MASTER","text":"x"}),
    );
    assert!(!master.ok, "{master:?}");
    assert_eq!(
        master.error.unwrap().message,
        "The MASTER room has no notes"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_orchestrator_core_human_abandon_idle_request_preserves_the_agent_and_queue() {
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let sent = call(
        &mut worker,
        "send-wedged",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"wedged"}),
    );
    let request = RequestId(sent.result["request_ids"][0].as_u64().unwrap());
    let message = sent.result["message_id"].clone();
    worker
        .state
        .begin_submission(request, "launch", 10)
        .unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn-1".into()),
            },
        )
        .unwrap();
    assert_eq!(
        worker.state.accept_callback(ProviderCallback {
            callback_id: "start".into(),
            sequence: 11,
            occurred_at_ms: 11,
            agent_id: agent,
            launch_id: "launch".into(),
            provider_session_id: Some("session".into()),
            provider_turn_id: Some("turn-1".into()),
            provider_prompt_id: None,
            prompt_payload: Some("wedged".into()),
            kind: CallbackEventKind::PromptStarted,
        }),
        CallbackDisposition::AcceptedBinding
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 12)
        .unwrap();

    let recovered = call(
        &mut worker,
        "recover-wedged",
        "request.recover",
        json!({"request":request.0.to_string(),"confirm":true}),
    );
    assert!(recovered.ok, "{recovered:?}");
    assert_eq!(recovered.result["stage"], "abandoned");
    assert_eq!(recovered.result["agent_id"], agent.0);
    assert_eq!(worker.state.agent(agent).unwrap().current_request, None);

    let status = call(
        &mut worker,
        "status-after-recovery",
        "message.status",
        json!({"message":message.to_string()}),
    );
    assert!(status.ok, "{status:?}");
    assert_eq!(status.result["complete"], true);
    assert_eq!(status.result["requests"][0]["stage"], "abandoned");
    assert!(status.result["requests"][0]["reply"].is_null());

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_master_room_is_selectable_listed_and_fixed() {
    let (mut worker, room, _agent, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let state = call(&mut worker, "state-master", "state", json!({}));
    assert_eq!(state.result["master_room"], json!(master));
    assert_eq!(state.result["rooms"][0]["kind"], "master");
    assert_eq!(state.result["rooms"][1]["kind"], "work");
    assert_eq!(state.result["rooms"][1]["id"], json!(room));

    for (id, selector, on) in [("focus-a", "master", false), ("focus-b", "MASTER", true)] {
        let sound = call(
            &mut worker,
            id,
            "room.sound",
            json!({"room": selector, "on": on}),
        );
        assert!(sound.ok, "{sound:?}");
        assert_eq!(worker.state.room(master).unwrap().sound, Some(on));
    }

    let rename = call(
        &mut worker,
        "rename-master",
        "room.rename",
        json!({"room":"master","name":"other"}),
    );
    assert!(!rename.ok);
    assert_eq!(
        error_message(&rename),
        "The MASTER room cannot be renamed or deleted"
    );
    let delete = call(
        &mut worker,
        "delete-master",
        "room.delete",
        json!({"room":"master","confirm":true}),
    );
    assert!(!delete.ok);
    assert_eq!(
        error_message(&delete),
        "The MASTER room cannot be renamed or deleted"
    );
    assert!(worker.state.master_room().is_some());
    let reserved = call(
        &mut worker,
        "create-master",
        "room.create",
        json!({"name":"Master"}),
    );
    assert!(
        error_message(&reserved).contains("reserved"),
        "{reserved:?}"
    );

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_agent_rename_and_room_seen_match_tui_commands() {
    let (mut worker, room, agent, dir) = fixture();
    let renamed = call(
        &mut worker,
        "rename-1",
        "agent.rename",
        json!({"agent":"codex1","name":"Reviewer"}),
    );
    assert!(renamed.ok, "{renamed:?}");
    assert_eq!(worker.state.agent(agent).unwrap().name, "Reviewer");
    let blank = call(
        &mut worker,
        "rename-2",
        "agent.rename",
        json!({"agent":"Reviewer","name":"  "}),
    );
    assert_eq!(blank.error.unwrap().code, "command_failed");

    // Replies arrive through the full callback path; inject the count directly.
    let mut saved = serde_json::to_value(&worker.state).unwrap();
    saved["rooms"][room.0.to_string()]["unread_count"] = json!(3);
    worker.state = serde_json::from_value(saved).unwrap();
    let state = call(&mut worker, "state-1", "state", json!({}));
    let reported = state.result["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == json!(room))
        .unwrap();
    assert_eq!(reported["unread_count"], 3);
    assert_eq!(state.result["visible_room"], serde_json::Value::Null);

    let seen = call(&mut worker, "seen-1", "room.seen", json!({"room":"test"}));
    assert!(seen.ok, "{seen:?}");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 0);
    // Marking seen is not navigation: the human's view is unchanged.
    assert_eq!(worker.state.visible_room(), None);

    worker.state.select_room(room).unwrap();
    let state = call(&mut worker, "state-2", "state", json!({}));
    assert_eq!(state.result["visible_room"], json!(room));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_sounds_lists_system_sounds_and_room_sound_picks_one_by_name() {
    let (mut worker, room, _agent, dir) = fixture();
    let sounds = dir.join("sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    for name in ["Glass.aiff", "Windows Notify.wav", "notes.txt"] {
        std::fs::write(sounds.join(name), b"").unwrap();
    }
    worker.sound_dirs = Some(vec![sounds.clone()]);

    let listed = call(&mut worker, "sounds", "sounds", json!({}));
    assert!(listed.ok, "{listed:?}");
    assert_eq!(
        listed.result["sounds"],
        json!([
            {"name": "Default", "path": null},
            {"name": "Glass", "path": sounds.join("Glass.aiff")},
            {"name": "Windows Notify", "path": sounds.join("Windows Notify.wav")},
        ])
    );
    let state = call(&mut worker, "sound-name-state-1", "state", json!({}));
    assert_eq!(state.result["rooms"][1]["sound_name"], "Default");

    let pick = call(
        &mut worker,
        "sound-pick",
        "room.sound",
        json!({"room":"test","on":true,"sound":"glass"}),
    );
    assert!(pick.ok, "{pick:?}");
    let saved = worker.store.load().unwrap().unwrap();
    let saved = saved.room(room).unwrap();
    assert_eq!(
        saved.sound_name.as_deref(),
        Some("Glass"),
        "stored by its listed name"
    );
    assert!(saved.sound_enabled());
    let state = call(&mut worker, "sound-name-state-2", "state", json!({}));
    assert_eq!(state.result["rooms"][1]["sound_name"], "Glass");

    // An unknown name changes nothing, not even the on/off choice.
    let unknown = call(
        &mut worker,
        "sound-unknown",
        "room.sound",
        json!({"room":"test","on":false,"sound":"Sosumi"}),
    );
    assert!(!unknown.ok);
    assert!(worker.state.room(room).unwrap().sound_enabled());
    assert_eq!(
        worker.state.room(room).unwrap().sound_name.as_deref(),
        Some("Glass")
    );

    // Without --sound the choice stays; Default restores Bus's own ding.
    let off = call(
        &mut worker,
        "sound-keep",
        "room.sound",
        json!({"room":"test","on":false}),
    );
    assert!(off.ok, "{off:?}");
    assert_eq!(
        worker.state.room(room).unwrap().sound_name.as_deref(),
        Some("Glass")
    );
    let reset = call(
        &mut worker,
        "sound-default",
        "room.sound",
        json!({"room":"test","on":true,"sound":"default"}),
    );
    assert!(reset.ok, "{reset:?}");
    assert_eq!(worker.state.room(room).unwrap().sound_name, None);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn opening_a_session_with_a_legacy_work_room_named_master_keeps_the_name_unique() {
    let dir = std::env::temp_dir().join(format!(
        "bus-control-legacy-master-{}-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    crate::messaging::storage::io::private_dir(&dir).unwrap();
    // Before MASTER existed, any room name was accepted.
    let mut legacy = BusState::default();
    let room = legacy.create_room("plans").unwrap();
    let mut saved = serde_json::to_value(&legacy).unwrap();
    saved["rooms"][room.0.to_string()]["name"] = json!("Master");
    let legacy: BusState = serde_json::from_value(saved).unwrap();
    JsonStore::new(dir.join("state.json"))
        .save(&legacy)
        .unwrap();

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = true;
    let named_master = worker
        .state
        .rooms()
        .filter(|r| r.name.eq_ignore_ascii_case(MASTER_ROOM_NAME))
        .count();
    assert_eq!(
        named_master,
        1,
        "only the MASTER room may carry its name: {:?}",
        worker
            .state
            .rooms()
            .map(|r| (&r.name, r.kind))
            .collect::<Vec<_>>()
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deleting_a_room_deletes_its_orchestrator_from_master() {
    let (mut worker, room, codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let next = worker.state.create_room("next").unwrap();
    let orchestrator = worker
        .state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let other = worker
        .state
        .create_agent(master, "next-orch", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(other, next).unwrap();

    let deleted = call(
        &mut worker,
        "delete-orchestrated",
        "room.delete",
        json!({"room":"test","confirm":true}),
    );
    assert!(deleted.ok, "{deleted:?}");
    assert!(worker.state.room(room).is_none());
    assert!(worker.state.agent(codex).is_none());
    assert!(worker.state.agent(orchestrator).is_none());
    // Another room's orchestrator is untouched.
    assert_eq!(
        worker.state.orchestrator_of(next).map(|a| a.id),
        Some(other)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn saved_work_room_named_master_keeps_its_notes_when_addressed_by_name() {
    let dir = std::env::temp_dir().join(format!(
        "bus-control-named-master-{}-{}-{}",
        std::process::id(),
        crate::messaging::storage::io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    crate::messaging::storage::io::private_dir(&dir).unwrap();
    let mut legacy = BusState::default();
    let room = legacy.create_room("plans").unwrap();
    legacy.set_room_notes(room, "keep-notes").unwrap();
    legacy.set_draft_text(room, "keep-draft").unwrap();
    let mut saved = serde_json::to_value(&legacy).unwrap();
    saved["rooms"][room.0.to_string()]["name"] = json!("Master");
    saved["rooms"][room.0.to_string()]["unread_count"] = json!(4);
    let legacy: BusState = serde_json::from_value(saved).unwrap();
    JsonStore::new(dir.join("state.json"))
        .save(&legacy)
        .unwrap();

    let mut worker = Worker::open(dir.clone(), Box::new(NoTransport)).unwrap();
    worker.dev_enabled = true;
    assert_eq!(worker.state.room(room).unwrap().notes, "keep-notes");
    assert_eq!(worker.state.room(room).unwrap().draft.text, "keep-draft");
    assert_eq!(worker.state.room(room).unwrap().unread_count, 4);
    assert_eq!(worker.state.room(room).unwrap().kind, RoomKind::Work);
    assert_eq!(
        worker
            .state
            .rooms()
            .filter(|r| r.name.eq_ignore_ascii_case(MASTER_ROOM_NAME))
            .count(),
        1,
        "a saved work room named Master must not share that name with MASTER: {:?}",
        worker
            .state
            .rooms()
            .map(|r| (&r.name, r.kind))
            .collect::<Vec<_>>()
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_long_session_of_mutations_never_hits_a_permanent_receipt_limit() {
    let (mut worker, _room, _agent, dir) = fixture();
    // Retries within the retention window replay; older receipts make room.
    worker.control_receipt_retention = Duration::ZERO;
    for index in 0..1100 {
        let seen = call(
            &mut worker,
            &format!("seen-{index}"),
            "room.seen",
            json!({"room":"test"}),
        );
        assert!(seen.ok, "{index}: {seen:?}");
    }
    assert!(worker.control_receipts.len() <= 1);

    worker.control_receipt_retention = Duration::from_secs(600);
    let first = call(&mut worker, "kept", "room.seen", json!({"room":"test"}));
    assert!(first.ok, "{first:?}");
    let conflict = call(&mut worker, "kept", "room.seen", json!({"room":"other"}));
    assert!(
        error_message(&conflict).contains("already used"),
        "{conflict:?}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
