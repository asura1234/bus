use super::{
    AgentId, AgentRuntimeIdentity, BusState, LaunchKind, LaunchOutcome, Provider, RequestId,
    RequestPhase, RoomId, ARCHIVE_AFTER_MS,
};
use crate::messaging::storage::state_store::JsonStore;

fn send(state: &mut BusState, room: RoomId, agent: AgentId, text: &str, at_ms: u64) -> RequestId {
    state.set_draft_text(room, text).expect("draft text");
    state
        .set_draft_recipients(room, [agent])
        .expect("recipients");
    state.submit_draft(room, at_ms).expect("submit")[0]
}

fn release(state: &mut BusState, agent: AgentId, id: RequestId) {
    if let Some(queue) = state.queues.get_mut(&agent) {
        queue.retain(|queued| *queued != id);
    }
    if let Some(agent_state) = state.agents.get_mut(&agent) {
        if agent_state.current_request == Some(id) {
            agent_state.current_request = None;
        }
    }
}

#[test]
fn clear_dashboard_data_drops_settled_requests_replies_and_launches_and_keeps_live_requests() {
    let mut state = BusState::new();
    let room = state.create_room("work").expect("room");
    let agent = state
        .create_agent(room, "worker", Provider::Codex, "/tmp".into(), None)
        .expect("agent");
    let archived = send(&mut state, room, agent, "old message", 1);
    state.finish_for_test(archived, "old reply", 1);
    let replied = send(&mut state, room, agent, "please answer", 2_000);
    state.finish_for_test(replied, "the reply body", 2_000);
    let failed = send(&mut state, room, agent, "please fail", 2_000);
    {
        let request = state.requests.get_mut(&failed).expect("failed request");
        request.phase = RequestPhase::Abandoned;
        request.failure_reason = Some("bus_timeout".into());
        request.completed_at_ms = Some(2_000);
    }
    release(&mut state, agent, failed);
    assert!(
        state.archive(ARCHIVE_AFTER_MS + 50),
        "the old settled request must reach history.jsonl"
    );
    assert!(state.history().request(archived).is_some());
    assert!(state.request(replied).unwrap().reply().is_some());

    let active = send(&mut state, room, agent, "still working", 5_000);
    {
        let request = state.requests.get_mut(&active).expect("active request");
        request.phase = RequestPhase::Active;
    }
    release(&mut state, agent, active);
    state.agents.get_mut(&agent).expect("agent").current_request = Some(active);
    state.set_draft_text(room, "waiting").expect("queued text");
    state
        .set_draft_recipients(room, [agent])
        .expect("queued recipients");
    let queued = state.submit_draft_queued(room, 5_000).expect("queue")[0];

    let pending =
        state.begin_launch_attempt("worker", Provider::Codex, room, LaunchKind::Create, 4_000);
    state
        .attach_launch_attempt(pending, agent)
        .expect("attach launch");
    let failed_launch =
        state.begin_launch_attempt("worker", Provider::Codex, room, LaunchKind::Resume, 4_100);
    assert!(state.settle_launch_attempt(
        failed_launch,
        &AgentRuntimeIdentity::default(),
        LaunchOutcome::Failed,
        Some("missing_executable"),
        4_200,
    ));
    assert_eq!(state.launch_attempts().count(), 2);

    let root = crate::utils::test_temp::unique_temp_path("clear-dashboard-data");
    std::fs::create_dir_all(&root).unwrap();
    let store = JsonStore::new(root.join("state.json"));
    store.save(&state).unwrap();
    let mut loaded = store.load().unwrap().unwrap();
    let queued_before = loaded.request(queued).unwrap().clone();
    let active_before = loaded.request(active).unwrap().clone();
    assert_eq!(queued_before.phase, RequestPhase::Queued);
    assert_eq!(active_before.phase, RequestPhase::Active);
    assert!(loaded.history().request(archived).is_some());
    assert_eq!(
        loaded.request(replied).unwrap().reply().unwrap().text,
        "the reply body"
    );
    assert_eq!(
        loaded.request(failed).unwrap().failure_reason.as_deref(),
        Some("bus_timeout")
    );
    assert!(loaded.launch_attempt(pending).is_some());
    assert!(loaded.launch_attempt(failed_launch).is_some());
    assert_eq!(
        loaded.agent(agent).unwrap().launch_attempt_id,
        Some(pending)
    );

    assert!(loaded.clear_dashboard_data());
    store.rewrite_history(&loaded).unwrap();
    store.save(&loaded).unwrap();
    let again = store.load().unwrap().unwrap();
    let ids: Vec<_> = again.requests().map(|request| request.id).collect();
    assert!(ids.contains(&queued), "queued request stays");
    assert!(ids.contains(&active), "active request stays");
    assert!(!ids.contains(&replied), "settled reply is gone");
    assert!(!ids.contains(&failed), "settled failure is gone");
    assert!(!ids.contains(&archived), "archived request is gone");
    assert_eq!(again.request(queued).unwrap(), &queued_before);
    assert_eq!(again.request(active).unwrap(), &active_before);
    assert!(again.requests().all(|request| request.reply().is_none()));
    assert!(again.launch_attempts().next().is_none());
    assert!(again.agent(agent).unwrap().launch_attempt_id.is_none());
    let history = std::fs::read_to_string(root.join("history.jsonl")).unwrap_or_default();
    assert!(!history.contains("old reply"), "{history}");
    assert!(!history.contains("the reply body"), "{history}");
    assert!(!history.contains("bus_timeout"), "{history}");
    std::fs::remove_dir_all(root).unwrap();
}
