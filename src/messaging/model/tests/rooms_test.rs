use super::*;

#[test]
fn master_room_is_added_once_without_consuming_ids() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    assert_eq!(state.ensure_master_room(), master);
    let masters = state
        .rooms()
        .filter(|room| room.kind == RoomKind::Master)
        .collect::<Vec<_>>();
    assert_eq!(masters.len(), 1);
    assert_eq!(masters[0].name, MASTER_ROOM_NAME);
    // Seeding the first work room still sees a pristine session.
    assert!(state.is_pristine());
    assert!(!state.has_work());
    let work = state.create_room("work").unwrap();
    assert_eq!(work, RoomId(1));
    assert_eq!(state.rooms().next().map(|room| room.id), Some(master));
    assert!(state.has_work());
}

#[test]
fn sound_rings_by_default_only_in_master_until_the_human_chooses() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    assert!(state.room(master).unwrap().sound_enabled());
    assert!(!state.room(work).unwrap().sound_enabled());

    state.set_room_sound(master, false).unwrap();
    state.set_room_sound(work, true).unwrap();
    assert!(!state.room(master).unwrap().sound_enabled());
    assert!(state.room(work).unwrap().sound_enabled());
    assert_eq!(
        state.set_room_sound(RoomId(999), true),
        Err(ModelError::UnknownRoom(RoomId(999)))
    );

    // A MASTER room saved before the sound field existed still rings.
    let mut document = serde_json::to_value(&state).unwrap();
    let rooms = document["rooms"].as_object_mut().unwrap();
    rooms.get_mut(&master.0.to_string()).unwrap()["sound"] = serde_json::Value::Null;
    rooms
        .get_mut(&work.0.to_string())
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("sound");
    let loaded: BusState = serde_json::from_value(document).unwrap();
    assert!(loaded.room(master).unwrap().sound_enabled());
    assert!(!loaded.room(work).unwrap().sound_enabled());
}

#[test]
fn room_sound_names_persist_and_old_sessions_load_with_the_default() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    state
        .set_room_sound_name(work, Some("Glass".into()))
        .unwrap();
    assert_eq!(
        state.set_room_sound_name(RoomId(999), None),
        Err(ModelError::UnknownRoom(RoomId(999)))
    );
    let mut document = serde_json::to_value(&state).unwrap();
    let loaded: BusState = serde_json::from_value(document.clone()).unwrap();
    assert_eq!(
        loaded.room(work).unwrap().sound_name.as_deref(),
        Some("Glass")
    );
    assert_eq!(loaded.room(master).unwrap().sound_name, None);

    // Sessions saved before sound names existed play Bus's own ding.
    document["rooms"][work.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("sound_name");
    let loaded: BusState = serde_json::from_value(document).unwrap();
    assert_eq!(loaded.room(work).unwrap().sound_name, None);
}

#[test]
fn master_room_has_no_notes_and_work_rooms_keep_theirs() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    assert_eq!(
        state.set_room_notes(master, "status"),
        Err(ModelError::MasterRoomHasNoNotes)
    );
    assert_eq!(state.room(master).unwrap().notes, "");
    state.set_room_notes(work, "status").unwrap();
    assert_eq!(state.room(work).unwrap().notes, "status");
}

#[test]
fn master_room_cannot_be_renamed_or_deleted_and_its_name_is_reserved() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    assert_eq!(
        state.rename_room(master, "other"),
        Err(ModelError::MasterRoomFixed)
    );
    assert_eq!(
        state.prepare_delete_room(master),
        Err(ModelError::MasterRoomFixed)
    );
    assert_eq!(state.delete_room(master), Err(ModelError::MasterRoomFixed));
    assert!(!state.room(master).unwrap().deletion_pending);
    for name in ["MASTER", "master", " Master "] {
        assert_eq!(state.create_room(name), Err(ModelError::ReservedRoomName));
    }
    let work = state.create_room("work").unwrap();
    assert_eq!(
        state.rename_room(work, "master"),
        Err(ModelError::ReservedRoomName)
    );
    assert_eq!(
        ModelError::MasterRoomFixed.to_string(),
        "The MASTER room cannot be renamed or deleted"
    );
}

#[test]
fn each_work_room_has_at_most_one_orchestrator_bound_for_life() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let pr = state.create_room("pr-123").unwrap();
    let other = state.create_room("pr-456").unwrap();
    let first = master_agent(&mut state, "claude-orch");
    let second = master_agent(&mut state, "codex-orch");

    state.bind_orchestrator(first, pr).unwrap();
    assert_eq!(state.orchestrator_of(pr).map(|agent| agent.id), Some(first));
    let taken = state.bind_orchestrator(second, pr);
    assert_eq!(
        taken,
        Err(ModelError::RoomAlreadyOrchestrated {
            room: pr,
            agent: first
        })
    );
    assert!(taken
        .unwrap_err()
        .to_string()
        .contains("at most one orchestrator"));
    // A bound orchestrator never moves, not even to its own room again.
    for room in [other, pr] {
        assert_eq!(
            state.bind_orchestrator(first, room),
            Err(ModelError::OrchestratorAlreadyBound(first))
        );
    }
    assert_eq!(state.agent(first).unwrap().orchestrates, Some(pr));

    assert_eq!(
        state.bind_orchestrator(second, master),
        Err(ModelError::NotOrchestratable(master))
    );
    assert_eq!(
        state.bind_orchestrator(second, RoomId(999)),
        Err(ModelError::NotOrchestratable(RoomId(999)))
    );
    state.prepare_delete_room(other).unwrap();
    assert_eq!(
        state.bind_orchestrator(second, other),
        Err(ModelError::NotOrchestratable(other))
    );
}

#[test]
fn only_master_agents_orchestrate_rooms() {
    let mut state = BusState::new();
    state.ensure_master_room();
    let pr = state.create_room("pr-123").unwrap();
    let worker = state
        .create_agent(pr, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    assert_eq!(
        state.bind_orchestrator(worker, pr),
        Err(ModelError::OrchestratorOutsideMaster(worker))
    );
    assert_eq!(state.agent(worker).unwrap().orchestrates, None);
}

#[test]
fn deleting_a_room_deletes_its_orchestrator_too() {
    let mut state = BusState::new();
    state.ensure_master_room();
    let pr = state.create_room("pr-123").unwrap();
    let other = state.create_room("pr-456").unwrap();
    let orchestrator = master_agent(&mut state, "claude-orch");
    state.bind_orchestrator(orchestrator, pr).unwrap();
    let bystander = master_agent(&mut state, "codex-orch");
    state.bind_orchestrator(bystander, other).unwrap();
    let worker = state
        .create_agent(pr, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    assert_eq!(state.agents_deleted_with_room(pr), [orchestrator, worker]);

    state.prepare_delete_room(pr).unwrap();
    assert!(state.agent(orchestrator).unwrap().deletion_pending);
    assert!(!state.agent(bystander).unwrap().deletion_pending);
    state.delete_room(pr).unwrap();

    assert!(state.agent(orchestrator).is_none());
    assert!(state.agent(worker).is_none());
    assert_eq!(state.agent(bystander).unwrap().orchestrates, Some(other));
}

#[test]
fn ensure_master_room_drops_assignments_that_break_the_invariants() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let pr = state.create_room("pr-123").unwrap();
    let first = master_agent(&mut state, "first");
    let second = master_agent(&mut state, "second");
    let outsider = state
        .create_agent(pr, "builder", Provider::Codex, "/repo".into(), None)
        .unwrap();
    // Hand-edited or corrupt saved state can carry assignments the API refuses.
    state.agents.get_mut(&first).unwrap().orchestrates = Some(pr);
    state.agents.get_mut(&second).unwrap().orchestrates = Some(pr);
    state.agents.get_mut(&outsider).unwrap().orchestrates = Some(pr);
    let lost = master_agent(&mut state, "lost");
    state.agents.get_mut(&lost).unwrap().orchestrates = Some(master);

    state.ensure_master_room();

    assert_eq!(state.agent(first).unwrap().orchestrates, Some(pr));
    assert_eq!(state.agent(second).unwrap().orchestrates, None);
    assert_eq!(state.agent(outsider).unwrap().orchestrates, None);
    assert_eq!(state.agent(lost).unwrap().orchestrates, None);
}

#[test]
fn pending_deletion_rejects_new_work_even_with_an_empty_draft() {
    let (mut state, room, agent, _) = state_with_room_and_agents();
    let request = submit_text(&mut state, room, agent, "queued");
    state.prepare_delete_room(room).unwrap();
    assert_eq!(
        state.submit_draft(room, 20),
        Err(ModelError::DeletionPending)
    );
    assert_eq!(
        state.begin_submission(request, "launch-codex", 10),
        Err(ModelError::DeletionPending)
    );
    assert_eq!(state.next_queued_request(agent), None);
    assert_eq!(
        state.create_agent(room, "late", Provider::Codex, PathBuf::from("/repo"), None),
        Err(ModelError::DeletionPending)
    );
}

#[test]
fn room_notes_drafts_and_names_are_room_local_and_ids_survive_renames() {
    let (mut state, first, agent, _) = state_with_room_and_agents();
    let second = state.create_room("second").expect("second room");
    state.rename_room(first, "renamed").expect("rename room");
    state.rename_agent(agent, "new name").expect("rename agent");
    state
        .set_room_notes(first, "Goal\nNon-goals")
        .expect("notes");
    state.set_draft_text(first, "first draft").expect("draft");
    state.set_draft_text(second, "second draft").expect("draft");
    state
        .set_agent_details_disclosed(agent, true)
        .expect("disclosure");

    assert_eq!(state.room(first).expect("first").id, first);
    assert_eq!(state.room(first).expect("first").name, "renamed");
    assert_eq!(state.agent(agent).expect("agent").id, agent);
    assert_eq!(state.agent(agent).expect("agent").name, "new name");
    assert_eq!(state.room(first).expect("first").notes, "Goal\nNon-goals");
    assert_eq!(state.room(first).expect("first").draft.text, "first draft");
    assert_eq!(
        state.room(second).expect("second").draft.text,
        "second draft"
    );
    assert!(state.agent(agent).expect("agent").details_disclosed);
}

#[test]
fn deleting_the_visible_room_drops_its_orchestrators_master_requests_and_shows_master() {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let pr = state.create_room("pr-123").unwrap();
    let orchestrator = master_agent(&mut state, "claude-orch");
    state.bind_orchestrator(orchestrator, pr).unwrap();
    let queued = submit_text(&mut state, master, orchestrator, "status?");
    state.select_room(pr).unwrap();

    state.prepare_delete_room(pr).unwrap();
    state.delete_room(pr).unwrap();

    assert_eq!(state.visible_room(), Some(master));
    assert!(state.request(queued).is_none());
    assert!(!state.queues.contains_key(&orchestrator));
    assert!(state.orchestrator_of(pr).is_none());
    assert!(state
        .requests()
        .all(|request| request.agent_id != orchestrator));
}

#[test]
fn a_saved_work_room_named_master_is_renamed_past_existing_old_names() {
    let mut state = BusState::new();
    let old = state.create_room("Master (old)").unwrap();
    let clashing = state.create_room("placeholder").unwrap();
    // Sessions saved before MASTER existed could name a work room anything.
    state.rooms.get_mut(&clashing).unwrap().name = "master".into();

    let master = state.ensure_master_room();

    assert_eq!(state.room(old).unwrap().name, "Master (old)");
    assert_eq!(state.room(clashing).unwrap().name, "Master (old 2)");
    assert_eq!(state.room(master).unwrap().name, MASTER_ROOM_NAME);
    assert_eq!(state.room(clashing).unwrap().kind, RoomKind::Work);
}
