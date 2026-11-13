//! Per-request outbound correlation without a bridge transport.
use super::*;

fn fixture() -> (BusState, RoomId, AgentId, AgentId) {
    let mut state = BusState::new();
    let master = state.ensure_master_room();
    let work = state.create_room("work").unwrap();
    let orch = state
        .create_agent(master, "orch", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    state.bind_orchestrator(orch, work).unwrap();
    let worker = state
        .create_agent(work, "worker", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    (state, work, orch, worker)
}

fn prepare(state: &mut BusState, room: RoomId, sender: AgentId) -> String {
    let before = state.clone();
    state
        .post_to_developer(
            room,
            sender,
            format!(
                "[waiting-on-you] Choose rollout {}",
                before.feishu.outbox.len()
            ),
            vec![],
            10,
        )
        .unwrap();
    state.prepare_feishu_alerts(&before, "/tmp/private-bus-session", "ou_developer", 11);
    state
        .feishu
        .outbox
        .values()
        .find(|record| !before.feishu.outbox.contains_key(&record.code))
        .unwrap()
        .code
        .clone()
}

#[test]
fn feishu_outbox_keeps_each_request_and_worker_home_binding_before_send() {
    let (mut state, work, orch, worker) = fixture();
    let first = prepare(&mut state, work, worker);
    let second = prepare(&mut state, work, worker);
    assert_ne!(
        first, second,
        "distinct asks need separate DMs and reply routes"
    );
    assert_eq!(state.feishu.outbox.len(), 2);
    let record = &state.feishu.outbox[&first];
    assert_eq!(record.session, "/tmp/private-bus-session");
    assert_eq!(record.room, work);
    assert_eq!(record.raising_agent, Some(worker));
    assert_eq!(record.orchestrator, Some(orch));
    assert_eq!(
        record.message_id, None,
        "outbox exists before any transport success"
    );
    let restored: BusState = serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(restored.feishu, state.feishu);
}

#[test]
fn feishu_mapping_changes_require_a_durable_save() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    let before_send = state.clone();
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    assert!(state.durable_poll_change_from(&before_send));
}

#[test]
fn feishu_master_conversation_routes_to_the_raising_orchestrator() {
    let (mut state, work, orch, _) = fixture();
    let master = state.master_room().unwrap().id;
    let other = state
        .create_agent(master, "other", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    let other_room = state.create_room("other-work").unwrap();
    state.bind_orchestrator(other, other_room).unwrap();
    let code = prepare(&mut state, master, orch);
    assert_eq!(state.feishu.outbox[&code].orchestrator, Some(orch));
    assert_ne!(state.feishu.outbox[&code].orchestrator, Some(other));
    assert_eq!(state.orchestrator_of(work).unwrap().id, orch);
}

#[test]
fn feishu_completed_worker_reply_keeps_its_request_source_and_room_binding() {
    let (mut state, work, orch, worker) = fixture();
    let original = state
        .submit_message_from(
            work,
            Draft {
                text: "Inspect access".into(),
                files: vec![],
                recipient_ids: [worker].into(),
            },
            Author::Developer,
            1,
        )
        .unwrap()[0];
    let before = state.clone();
    state.finish_for_test(original, "[waiting-on-you] Approve access", 10);
    state.prepare_feishu_alerts(&before, "/tmp/private-bus-session", "ou_developer", 11);
    assert_eq!(state.feishu.outbox.len(), 1);
    let record = state.feishu.outbox.values().next().unwrap();
    assert_eq!(record.room, work);
    assert_eq!(record.raising_agent, Some(worker));
    assert_eq!(record.orchestrator, Some(orch));
    assert_eq!(record.source.request_id, Some(original));
    assert_eq!(
        record.source.prompt_id,
        state.request(original).unwrap().prompt.id
    );
}

#[test]
fn feishu_bus_attention_notice_keeps_the_originating_orchestrator() {
    let (mut state, _, orch, _) = fixture();
    let before = state.clone();
    state
        .attention_episode(
            "episode".into(),
            Some(true),
            orch,
            "[waiting-on-you] Unlock terminal",
            10,
        )
        .unwrap();
    state.prepare_feishu_alerts(&before, "/tmp/private-bus-session", "ou_developer", 11);
    let record = state.feishu.outbox.values().next().unwrap();
    assert_eq!(record.raising_agent, Some(orch));
    assert_eq!(record.orchestrator, Some(orch));
}

#[test]
fn feishu_outbox_keeps_an_unbound_worker_without_selecting_another_orchestrator() {
    let (mut state, _, _, _) = fixture();
    let unbound = state.create_room("unbound").unwrap();
    let worker = state
        .create_agent(
            unbound,
            "unbound-worker",
            Provider::Codex,
            "/tmp".into(),
            None,
        )
        .unwrap();
    let code = prepare(&mut state, unbound, worker);
    assert_eq!(state.feishu.outbox[&code].orchestrator, None);
}
