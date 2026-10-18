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
fn session_moved_left_open_notice_lands_in_room_history_without_a_composer_error() {
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
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert_eq!(worker.snapshot().error, None);
    let notice = worker.state.notices(room).last().unwrap();
    assert!(
        notice.text.contains("author")
            && notice.text.contains("another provider session")
            && notice.text.contains("close pane pane yourself"),
        "{}",
        notice.text
    );
    assert!(left_open_requests(&worker).is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn room_deletion_reports_left_open_terminal_in_master_not_to_its_deleted_orchestrator() {
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
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteRoom(room), &events)
        .unwrap();
    assert_eq!(worker.snapshot().error, None);
    assert!(worker.state.agent(orchestrator).is_none());
    assert!(worker.state.agent(agent).is_none());
    assert!(left_open_requests(&worker).is_empty());
    let notice = worker.state.notices(RoomId(0)).last().unwrap();
    assert!(
        notice.text.contains("author") && notice.text.contains("close pane pane yourself"),
        "{}",
        notice.text
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn left_open_notice_to_orchestrator_keeps_the_developer_draft_in_the_room() {
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
    let (events, _) = mpsc::channel();
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    let requests = left_open_requests(&worker);
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, orchestrator);
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
fn failed_deletion_posts_no_left_open_notice_and_its_retry_posts_exactly_one() {
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
    let (events, _) = mpsc::channel();
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
    assert_eq!(
        worker
            .state
            .notices(room)
            .filter(|notice| notice.text.contains("close pane pane yourself"))
            .count(),
        1
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
