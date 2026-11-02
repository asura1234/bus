use super::*;

const DECISION: &str = "retained worker decision";

fn worker_woken_turn() -> (BusState, RoomId, AgentId, AgentId, RequestId) {
    let (mut state, room, worker, _) = state_with_room_and_agents();
    let master = state.ensure_master_room();
    let orch = state
        .create_agent(
            master,
            "decision-maker",
            Provider::ClaudeCode,
            "/repo".into(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    state
        .set_agent_runtime_identity(
            orch,
            AgentRuntimeIdentity {
                launch_id: Some("launch-orch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("provider-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(orch).unwrap();
    state.observe_status(orch, RuntimeStatus::Idle, 1).unwrap();
    let request = state
        .submit_message_from(
            room,
            Draft {
                text: "worker result".into(),
                files: vec![],
                recipient_ids: [orch].into(),
            },
            Author::Agent(worker),
            2,
        )
        .unwrap()[0];
    start_request(&mut state, request, "launch-orch", 10);
    (state, master, worker, orch, request)
}

fn finish_decision(state: &mut BusState, orch: AgentId, request: RequestId) {
    let prompt = state.request(request).unwrap().prompt.rendered_payload();
    assert_eq!(
        state.accept_callback(ProviderCallback::final_event(
            "decision-final",
            13,
            orch,
            "launch-orch",
            "provider-session",
            "turn-1",
            &prompt,
            DECISION
        )),
        CallbackDisposition::AcceptedCompleted
    );
}

#[test]
fn deleted_worker_keeps_the_orchestrators_master_decision() {
    let (mut state, master, worker, orch, request) = worker_woken_turn();
    state.delete_agent(worker, 12).unwrap();
    assert!(state.agent(worker).is_none());
    finish_decision(&mut state, orch, request);
    assert_eq!(
        state.request(request).unwrap().reply().unwrap().text,
        DECISION
    );
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        1
    );
}

#[test]
fn an_individual_answer_keeps_its_reply_and_the_later_decision_reaches_master() {
    let (mut state, master, worker, orch, request) = worker_woken_turn();
    let message = state.request(request).unwrap().prompt.id;
    state
        .answer_message(message, orch, "Use the fallback".into(), 12)
        .unwrap();
    finish_decision(&mut state, orch, request);
    assert_eq!(
        state.request(request).unwrap().reply().unwrap().text,
        "Use the fallback"
    );
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        1
    );
    state
        .route_worker_replies(100_000, &std::collections::BTreeSet::new())
        .unwrap();
    let instructions: Vec<_> = state
        .requests()
        .filter(|r| r.agent_id == worker && r.prompt.author == Author::Agent(orch))
        .collect();
    assert_eq!(instructions.len(), 1);
    assert!(instructions[0].prompt.text.contains("Use the fallback"));
    assert!(!instructions[0].prompt.text.contains(DECISION));
}

#[test]
fn an_explicit_master_post_is_not_repeated_by_the_captured_final() {
    let (mut state, master, _, orch, request) = worker_woken_turn();
    state
        .post_to_developer(master, orch, DECISION.into(), vec![], 12)
        .unwrap();
    finish_decision(&mut state, orch, request);
    assert_eq!(
        state.request(request).unwrap().reply().unwrap().text,
        DECISION
    );
    assert_eq!(
        state.notices(master).filter(|p| p.text == DECISION).count(),
        1
    );
}
