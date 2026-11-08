use super::*;

fn orchestrator(worker: &mut Worker, room: RoomId) -> AgentId {
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let agent = state
        .create_agent(
            master,
            "priority-inbox",
            Provider::Codex,
            "/synthetic".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(agent, room).unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    worker.save(state).unwrap();
    agent
}

fn authored(
    worker: &mut Worker,
    room: RoomId,
    agent: AgentId,
    author: Author,
    text: &str,
) -> RequestId {
    let mut state = worker.state.clone();
    let id = state
        .submit_message_from(
            room,
            Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            author,
            io::now_ms(),
        )
        .unwrap()[0];
    worker.save(state).unwrap();
    id
}

#[test]
fn orchestrator_idle_submission_selects_original_developer_and_saves_a_clean_batch() {
    let (mut worker, sender, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Ok(ResponseResult::AgentPrompted {
            agent: native_info("pane", "codex", "session"),
            timestamps: Default::default(),
        })],
    );
    let orch = orchestrator(&mut worker, room);
    let n1 = authored(&mut worker, room, orch, Author::Agent(sender), "OLD_GO_ONE");
    let h1 = authored(
        &mut worker,
        room,
        orch,
        Author::Developer,
        "first developer",
    );
    let h2 = authored(
        &mut worker,
        room,
        orch,
        Author::Developer,
        "second developer",
    );
    let n2 = authored(&mut worker, room, orch, Author::Agent(sender), "OLD_GO_TWO");
    // Existing waited-queue behavior enables coalescing at the next idle boundary.
    let mut state = worker.state.clone();
    state
        .observe_status(orch, RuntimeStatus::Blocked, io::now_ms())
        .unwrap();
    worker.save(state).unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    let mut state = worker.state.clone();
    state
        .observe_status(orch, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    worker.save(state).unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["agent.prompt_if_idle"]);
    let saved = worker.store.load().unwrap().unwrap();
    assert_eq!(saved.agent(orch).unwrap().current_request, Some(h1));
    let text = saved
        .request(h1)
        .unwrap()
        .submitted_payload
        .as_ref()
        .unwrap();
    assert!(text.contains("first developer") && text.contains("second developer"));
    assert!(!text.contains("OLD_GO"));
    assert_eq!(saved.request(h2).unwrap().group, Some(h1));
    assert_eq!(saved.queued_requests(orch), &[n1, n2]);
    worker.submit_ready().unwrap();
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "unbound confirmed owner prevents another write"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn orchestrator_cross_room_developer_waits_behind_an_existing_uncertain_owner() {
    let (mut worker, sender, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let orch = orchestrator(&mut worker, room);
    let lead = authored(
        &mut worker,
        room,
        orch,
        Author::Agent(sender),
        "original owner",
    );
    let mut state = worker.state.clone();
    state.begin_submission(lead, "launch", 0).unwrap();
    state
        .record_submission(
            lead,
            SubmissionOutcome::Uncertain {
                message: "unknown".into(),
            },
        )
        .unwrap();
    state
        .observe_status(orch, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    worker.save(state).unwrap();
    let h = authored(&mut worker, room, orch, Author::Developer, "new developer");
    let before = worker.state.request(lead).unwrap().clone();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert_eq!(worker.state.request(lead).unwrap(), &before);
    assert_eq!(
        worker.state.agent(orch).unwrap().current_request,
        Some(lead)
    );
    assert_eq!(worker.state.request(h).unwrap().phase, RequestPhase::Queued);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
