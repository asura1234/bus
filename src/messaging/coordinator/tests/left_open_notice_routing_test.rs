use super::*;

fn left_open_requests(worker: &Worker) -> Vec<crate::messaging::model::Request> {
    worker
        .state
        .requests()
        .filter(|request| request.prompt.text.contains("close pane"))
        .cloned()
        .collect()
}

#[test]
fn session_moved_left_open_details_are_command_results_without_chat() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Cursor,
        vec![
            identity_changed(),
            native_agents(Some("bus-r1-a2".into())),
            Ok(ResponseResult::AgentInfo {
                agent: native_info("pane", "cursor", "moved"),
            }),
        ],
    );
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert_eq!(worker.snapshot().error, None);
    assert_eq!(worker.state.notices(room).count(), 0);
    let terminals = received
        .try_iter()
        .find_map(|event| {
            if let BusEvent::TerminalsLeftOpen(terminals) = event {
                Some(terminals)
            } else {
                None
            }
        })
        .expect("structured deletion outcome");
    assert_eq!(terminals.len(), 1);
    assert!(terminals[0].message.contains("another provider session"));
    assert_eq!(terminals[0].pane_id, "pane");
    assert!(left_open_requests(&worker).is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_deletion_returns_left_open_details_without_chat() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![identity_changed(), native_agents(None), identity_changed()],
    );
    let mut state = worker.state.clone();
    let orchestrator = state
        .create_agent(
            RoomId(0),
            "orchestrator",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, room).unwrap();
    worker.save(state).unwrap();
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    assert_eq!(worker.snapshot().error, None);
    assert!(worker.state.agent(orchestrator).is_none());
    assert!(worker.state.agent(agent).is_none());
    assert!(left_open_requests(&worker).is_empty());
    assert_eq!(worker.state.notices(RoomId(0)).count(), 0);
    assert!(received
        .try_iter()
        .any(|event| matches!(event, BusEvent::TerminalsLeftOpen(ref t) if t.len()==1)));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn left_open_result_keeps_the_developer_draft_without_waking_orchestrator() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![identity_changed(), native_agents(None), identity_changed()],
    );
    let mut state = worker.state.clone();
    let orchestrator = state
        .create_agent(
            RoomId(0),
            "orchestrator",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, room).unwrap();
    let peer = state
        .create_agent(room, "peer", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.set_draft_text(room, "half typed").unwrap();
    state.set_draft_recipients(room, [peer]).unwrap();
    worker.save(state).unwrap();
    let (events, received) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    let requests = left_open_requests(&worker);
    assert!(requests.is_empty());
    assert!(received
        .try_iter()
        .any(|event| matches!(event, BusEvent::TerminalsLeftOpen(ref t) if t.len()==1)));
    let draft = &worker.state.room(room).unwrap().draft;
    assert_eq!(draft.text, "half typed");
    assert_eq!(
        draft.recipient_ids.iter().copied().collect::<Vec<_>>(),
        vec![peer]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_deletion_retry_returns_one_left_open_result_without_chat() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![
            Err(TransportError {
                message: "server busy".into(),
                code: None,
                definitely_rejected: false,
            }),
            identity_changed(),
            native_agents(None),
            identity_changed(),
        ],
    );
    let (events, received) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert_eq!(worker.state.notices(room).count(), 0);
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert_eq!(worker.snapshot().error, None);
    assert_eq!(worker.state.notices(room).count(), 0);
    let results: Vec<_> = received
        .try_iter()
        .filter_map(|event| {
            if let BusEvent::TerminalsLeftOpen(t) = event {
                Some(t)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
