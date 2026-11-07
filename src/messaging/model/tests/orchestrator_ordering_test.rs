use super::*;

fn inbox() -> (BusState, RoomId, AgentId, AgentId) {
    let (mut state, room, sender, _) = state_with_room_and_agents();
    let orch = master_agent(&mut state, "inbox");
    state.bind_orchestrator(orch, room).unwrap();
    state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state
        .observe_status(orch, RuntimeStatus::Idle, 100)
        .unwrap();
    (state, room, orch, sender)
}

fn send(
    state: &mut BusState,
    room: RoomId,
    to: AgentId,
    author: Author,
    text: &str,
    queue_only: bool,
) -> RequestId {
    state
        .submit_message_with(
            room,
            Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [to].into(),
            },
            author,
            100,
            queue_only,
        )
        .unwrap()[0]
}

#[test]
fn orchestrator_developers_pass_backlog_stably_without_changing_prompts() {
    let (mut state, room, orch, sender) = inbox();
    let n1 = send(
        &mut state,
        room,
        orch,
        Author::Agent(sender),
        "old GO one",
        false,
    );
    let n2 = send(
        &mut state,
        room,
        orch,
        Author::Bus,
        "from you: priority",
        false,
    );
    let h1 = send(
        &mut state,
        room,
        orch,
        Author::Developer,
        "first developer",
        false,
    );
    let h2 = send(
        &mut state,
        room,
        orch,
        Author::Developer,
        "second developer",
        true,
    );
    let n3 = send(
        &mut state,
        room,
        orch,
        Author::Agent(sender),
        "you: Blocked, needs help",
        false,
    );
    let originals: Vec<_> = [n1, n2, h1, h2, n3]
        .into_iter()
        .map(|id| state.request(id).unwrap().clone())
        .collect();
    assert!(state.prioritize_orchestrator_dialogs(orch));
    assert_eq!(state.queued_requests(orch), &[h1, h2, n1, n2, n3]);
    assert!(!state.prioritize_orchestrator_dialogs(orch));
    for original in originals {
        assert_eq!(state.request(original.id).unwrap(), &original);
    }
    assert_eq!(state.next_queued_request(orch), Some(h1));
    assert!(
        state.coalesce_queue(h1).is_none(),
        "explicit queue remains a batch boundary"
    );
}

#[test]
fn orchestrator_developer_batch_excludes_old_grants_after_rejection_and_reload() {
    let (mut state, room, orch, sender) = inbox();
    let n1 = send(
        &mut state,
        room,
        orch,
        Author::Agent(sender),
        "OLD_GO_ONE",
        false,
    );
    let h1 = send(
        &mut state,
        room,
        orch,
        Author::Developer,
        "human one",
        false,
    );
    let h2 = send(
        &mut state,
        room,
        orch,
        Author::Developer,
        "human two",
        false,
    );
    let n2 = send(
        &mut state,
        room,
        orch,
        Author::Agent(sender),
        "OLD_GO_TWO",
        false,
    );
    state.prioritize_orchestrator_dialogs(orch);
    let text = state.coalesce_queue(h1).expect("developers batch together");
    assert!(text.contains("human one") && text.contains("human two"));
    assert!(!text.contains("OLD_GO"));
    state.begin_submission(h1, "launch", 9).unwrap();
    state
        .record_submission(
            h1,
            SubmissionOutcome::DefinitelyRejected {
                message: "not ready".into(),
            },
        )
        .unwrap();
    let mut state: BusState = serde_json::from_value(serde_json::to_value(state).unwrap()).unwrap();
    state.prioritize_orchestrator_dialogs(orch);
    assert_eq!(state.queued_requests(orch), &[h1, h2, n1, n2]);
    assert_eq!(state.coalesce_queue(h1).unwrap(), text);
    state.begin_submission(h1, "launch", 10).unwrap();
    state
        .record_submission(
            h1,
            SubmissionOutcome::Uncertain {
                message: "unknown write".into(),
            },
        )
        .unwrap();
    let frozen = state.request(h1).unwrap().clone();
    let member = state.request(h2).unwrap().clone();
    let h3 = send(
        &mut state,
        room,
        orch,
        Author::Developer,
        "later human",
        false,
    );
    state.prioritize_orchestrator_dialogs(orch);
    assert_eq!(state.queued_requests(orch), &[h3, n1, n2]);
    assert_eq!(state.next_queued_request(orch), None);
    assert_eq!(state.request(h1).unwrap(), &frozen);
    assert_eq!(state.request(h2).unwrap(), &member);
    assert_eq!(state.agent(orch).unwrap().current_request, Some(h1));
}

#[test]
fn worker_fifo_and_mixed_author_coalescing_stay_unchanged() {
    let (mut state, room, worker, sender) = state_with_room_and_agents();
    let n = send(
        &mut state,
        room,
        worker,
        Author::Agent(sender),
        "worker backlog",
        false,
    );
    let h = send(
        &mut state,
        room,
        worker,
        Author::Developer,
        "developer",
        false,
    );
    assert!(!state.prioritize_orchestrator_dialogs(worker));
    assert_eq!(state.queued_requests(worker), &[n, h]);
    let text = state.coalesce_queue(n).unwrap();
    assert!(text.contains("worker backlog") && text.contains("developer"));
}

#[test]
fn real_blocked_notices_precede_developers_and_cannot_join_their_batch() {
    let (mut state, room, orch, sender) = inbox();
    let n = send(
        &mut state,
        room,
        orch,
        Author::Agent(sender),
        "backlog",
        false,
    );
    let h = send(
        &mut state,
        room,
        orch,
        Author::Developer,
        "developer",
        false,
    );
    let notice = send(&mut state, room, orch, Author::Bus, "real notice", false);
    let worker = state.agents.get_mut(&sender).unwrap();
    worker.dialog_notice = Some("blocked".into());
    worker.dialog_notice_to = Some(DialogNoticeTo::Orchestrator(notice));
    state.prioritize_orchestrator_dialogs(orch);
    assert_eq!(state.queued_requests(orch), &[notice, h, n]);
    assert!(state.request(notice).unwrap().queue_only);
    assert!(state.coalesce_queue(notice).is_none());
    assert!(!state.request(h).unwrap().queue_only);
}
