//! Bus routing tests deliberately use normalized inputs, independent of the bridge wire schema.
use super::*;
use crate::messaging::model::feishu::{IncomingReply, IngestOutcome};

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

fn incoming(id: &str, parent: Option<&str>, root: Option<&str>, text: &str) -> IncomingReply {
    IncomingReply {
        app_id: "cli_test".into(),
        message_id: id.into(),
        chat_id: "oc_dm".into(),
        sender_id: "ou_developer".into(),
        text: text.into(),
        reply_to: parent.map(str::to_owned),
        root_id: root.map(str::to_owned),
        create_time: 20,
    }
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
fn feishu_parent_outbox_loads_when_inbound_receipts_are_absent() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let mut saved = serde_json::to_value(&state).unwrap();
    saved["feishu"].as_object_mut().unwrap().remove("inbox");
    let mut restored: BusState = serde_json::from_value(saved).unwrap();
    assert!(restored.feishu.inbox.is_empty());
    assert_eq!(restored.feishu.outbox, state.feishu.outbox);
    assert!(matches!(
        restored
            .ingest_feishu_reply(
                incoming("om_in", Some("om_out"), None, "Proceed"),
                "ou_developer",
                21,
            )
            .unwrap(),
        IngestOutcome::Delivered { .. }
    ));
}

#[test]
fn feishu_mapping_and_parked_reply_changes_require_a_durable_save() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    let before_send = state.clone();
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    assert!(state.durable_poll_change_from(&before_send));
    let before_park = state.clone();
    state
        .ingest_feishu_reply(
            incoming("om_unknown", None, None, "Proceed"),
            "ou_developer",
            21,
        )
        .unwrap();
    assert_eq!(state.requests().count(), 0);
    assert!(state.durable_poll_change_from(&before_park));
}

#[test]
fn feishu_reply_arriving_before_send_completion_resolves_when_the_exact_mapping_commits() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    let input = incoming("om_early", Some("om_out"), None, "Proceed");
    assert!(matches!(
        state
            .ingest_feishu_reply(input.clone(), "ou_developer", 21)
            .unwrap(),
        IngestOutcome::Parked { .. }
    ));
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let result = state
        .ingest_feishu_reply(input, "ou_developer", 22)
        .unwrap();
    assert!(matches!(result, IngestOutcome::Delivered { .. }));
    assert_eq!(state.requests().count(), 1);
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
fn feishu_reply_is_developer_authored_correlated_and_gets_an_independent_turn() {
    let (mut state, work, orch, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    let source = state.feishu.outbox[&code].source.clone();
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let outcome = state
        .ingest_feishu_reply(
            incoming("om_in", Some("om_out"), None, "Proceed"),
            "ou_developer",
            21,
        )
        .unwrap();
    let IngestOutcome::Delivered { request_id, .. } = outcome else {
        panic!("{outcome:?}")
    };
    let request = state.request(request_id).unwrap();
    assert_eq!(request.agent_id, orch);
    assert_eq!(request.room_id, work);
    assert_eq!(request.prompt.author, Author::Developer);
    assert_eq!(request.prompt.text, "Proceed");
    assert!(
        request.queue_only,
        "Feishu replies must get a real turn, including while the orchestrator is busy"
    );
    let provenance = request.prompt.in_reply_to.as_ref().unwrap();
    assert_eq!(provenance.source, source);
    assert_eq!(provenance.feishu_message_id, "om_in");
    assert_eq!(provenance.raising_agent, Some(worker));
    let payload = state.render_request_payload(request_id).unwrap();
    assert!(
        payload.contains("in_reply_to"),
        "the provider must receive correlation: {payload}"
    );
    assert!(payload.contains("Proceed"));
}

#[test]
fn feishu_replay_after_store_reload_returns_the_same_request() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let event = incoming("om_in", Some("om_out"), None, "Proceed");
    let first = state
        .ingest_feishu_reply(event.clone(), "ou_developer", 21)
        .unwrap();
    let root = crate::utils::test_temp::unique_temp_path("feishu-durable-dedupe");
    let store = crate::messaging::storage::state_store::JsonStore::new(root.join("state.json"));
    store.save(&state).unwrap();
    let mut reloaded = store.load().unwrap().unwrap();
    let count = reloaded.requests().count();
    assert_eq!(
        reloaded
            .ingest_feishu_reply(event, "ou_developer", 99)
            .unwrap(),
        first
    );
    assert_eq!(reloaded.requests().count(), count);
    let mut changed = incoming("om_in", Some("om_out"), None, "Different instruction");
    changed.create_time = 100;
    assert_eq!(
        reloaded
            .ingest_feishu_reply(changed, "ou_developer", 101)
            .unwrap(),
        first
    );
    assert_eq!(reloaded.requests().last().unwrap().prompt.text, "Proceed");
    std::fs::remove_dir_all(root).unwrap();
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
    let code = record.code.clone();
    let source = record.source.clone();
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let outcome = state
        .ingest_feishu_reply(
            incoming("om_in", Some("om_out"), None, "Approved"),
            "ou_developer",
            21,
        )
        .unwrap();
    let IngestOutcome::Delivered { request_id, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(
        state
            .request(request_id)
            .unwrap()
            .prompt
            .in_reply_to
            .as_ref()
            .unwrap()
            .source,
        source
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
fn feishu_unbound_worker_reply_is_parked_without_selecting_another_orchestrator() {
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
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    assert!(matches!(
        state
            .ingest_feishu_reply(
                incoming("om_in", Some("om_out"), None, "Proceed"),
                "ou_developer",
                21
            )
            .unwrap(),
        IngestOutcome::Parked { .. }
    ));
    assert_eq!(state.requests().count(), 0);
}

#[test]
fn feishu_unauthenticated_sender_cannot_consume_the_message_id() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let event = incoming("om_in", Some("om_out"), None, "Proceed");
    let mut forged = event.clone();
    forged.sender_id = "ou_impostor".into();
    assert!(state
        .ingest_feishu_reply(forged, "ou_developer", 21)
        .is_err());
    assert!(!state.feishu.inbox.contains_key("cli_test:om_in"));
    assert!(matches!(
        state
            .ingest_feishu_reply(event, "ou_developer", 22)
            .unwrap(),
        IngestOutcome::Delivered { .. }
    ));
}

#[test]
fn feishu_unknown_and_conflicting_ancestry_stay_undelivered() {
    let (mut state, work, _, worker) = fixture();
    let first = prepare(&mut state, work, worker);
    let second = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&first, "om_first", "oc_dm")
        .unwrap();
    state
        .complete_feishu_send(&second, "om_second", "oc_dm")
        .unwrap();
    for event in [
        incoming("om_unknown", None, None, "Proceed"),
        incoming(
            "om_conflict",
            Some("om_first"),
            Some("om_second"),
            "Proceed",
        ),
    ] {
        assert!(matches!(
            state
                .ingest_feishu_reply(event, "ou_developer", 21)
                .unwrap(),
            IngestOutcome::Parked { .. }
        ));
    }
    assert_eq!(state.requests().count(), 0);
    assert_eq!(state.feishu.inbox.len(), 2);
}

#[test]
fn feishu_root_nested_reply_and_short_code_resolve_without_guessing() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    for event in [
        incoming("om_first", Some("unknown_parent"), Some("om_out"), "First"),
        incoming("om_nested", Some("om_first"), None, "Nested"),
        incoming("om_code", None, None, &format!("bus:{code} Continue")),
    ] {
        assert!(matches!(
            state
                .ingest_feishu_reply(event, "ou_developer", 21)
                .unwrap(),
            IngestOutcome::Delivered { .. }
        ));
    }
    assert_eq!(state.requests().count(), 3);
}

#[test]
fn feishu_two_request_codes_and_conflicting_code_and_ancestry_are_ambiguous() {
    let (mut state, work, _, worker) = fixture();
    let first = prepare(&mut state, work, worker);
    let second = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&first, "om_first", "oc_dm")
        .unwrap();
    state
        .complete_feishu_send(&second, "om_second", "oc_dm")
        .unwrap();
    for event in [
        incoming(
            "om_codes",
            None,
            None,
            &format!("bus:{first} bus:{second} Proceed"),
        ),
        incoming(
            "om_conflict_code",
            Some("om_first"),
            None,
            &format!("bus:{second} Proceed"),
        ),
    ] {
        assert!(matches!(
            state
                .ingest_feishu_reply(event, "ou_developer", 21)
                .unwrap(),
            IngestOutcome::Parked { .. }
        ));
    }
    assert_eq!(state.requests().count(), 0);
}

#[test]
fn feishu_changed_recipient_cannot_authorize_a_reply_to_an_old_recipient() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let mut event = incoming("om_in", Some("om_out"), None, "Proceed");
    event.sender_id = "ou_new_developer".into();
    let receipt = state
        .ingest_feishu_reply(event.clone(), "ou_new_developer", 21)
        .unwrap();
    assert!(matches!(receipt, IngestOutcome::Parked { .. }));
    let mut restored: BusState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(
        restored
            .ingest_feishu_reply(event, "ou_new_developer", 22)
            .unwrap(),
        receipt
    );
    assert_eq!(state.requests().count(), 0);
}

#[test]
fn feishu_authenticated_chat_conflict_is_durable_and_does_not_block_a_later_reply() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    let mut conflict = incoming("om_conflict", Some("om_out"), None, "Proceed");
    conflict.chat_id = "oc_other".into();
    let parked = state
        .ingest_feishu_reply(conflict.clone(), "ou_developer", 21)
        .unwrap();
    assert!(matches!(parked, IngestOutcome::Parked { .. }));
    assert_eq!(state.requests().count(), 0);
    let mut restored: BusState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    assert_eq!(
        restored
            .ingest_feishu_reply(conflict, "ou_developer", 22)
            .unwrap(),
        parked
    );
    assert!(matches!(
        restored
            .ingest_feishu_reply(
                incoming("om_valid", Some("om_out"), None, "Continue"),
                "ou_developer",
                23
            )
            .unwrap(),
        IngestOutcome::Delivered { .. }
    ));
    assert_eq!(restored.requests().count(), 1);
    assert_eq!(
        restored.feishu.inbox["cli_test:om_conflict"].outcome,
        parked
    );
}

#[test]
fn feishu_short_code_can_recover_an_uncertain_send_without_resending() {
    let (mut state, work, orch, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    let event = incoming("om_in", None, None, &format!("bus:{code} Proceed"));
    let result = state
        .ingest_feishu_reply(event, "ou_developer", 21)
        .unwrap();
    let IngestOutcome::Delivered { request_id, .. } = result else {
        panic!("{result:?}")
    };
    assert_eq!(state.request(request_id).unwrap().agent_id, orch);
}

#[test]
fn feishu_deleted_orchestrator_is_parked_even_when_a_replacement_has_its_name() {
    let (mut state, work, orch, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    state.delete_agent(orch, 20).unwrap();
    let master = state.master_room().unwrap().id;
    let replacement = state
        .create_agent(master, "orch", Provider::Codex, "/tmp".into(), None)
        .unwrap();
    state.bind_orchestrator(replacement, work).unwrap();
    assert!(matches!(
        state
            .ingest_feishu_reply(
                incoming("om_in", Some("om_out"), None, "Proceed"),
                "ou_developer",
                21
            )
            .unwrap(),
        IngestOutcome::Parked { .. }
    ));
    assert_eq!(state.requests().count(), 0);
}

#[test]
fn feishu_parked_child_resolves_after_its_backfilled_parent_is_accepted() {
    let (mut state, work, _, worker) = fixture();
    let code = prepare(&mut state, work, worker);
    state
        .complete_feishu_send(&code, "om_out", "oc_dm")
        .unwrap();
    // A code-addressed plain message is a valid root. Its live child can beat
    // history backfill of that root, without carrying the request code itself.
    let mut child = incoming(
        "om_child",
        Some("om_parent"),
        Some("om_parent"),
        "Use option B",
    );
    child.create_time = 30;
    assert!(matches!(
        state
            .ingest_feishu_reply(child.clone(), "ou_developer", 40)
            .unwrap(),
        IngestOutcome::Parked { .. }
    ));
    let parent = incoming("om_parent", None, None, &format!("bus:{code} Proceed"));
    assert!(matches!(
        state
            .ingest_feishu_reply(parent, "ou_developer", 50)
            .unwrap(),
        IngestOutcome::Delivered { .. }
    ));
    // The route is now exact, including after a Bus restart. Replaying must
    // return a delivered receipt for the child's one queued instruction.
    let mut restored: BusState =
        serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
    let result = restored
        .ingest_feishu_reply(child, "ou_developer", 60)
        .unwrap();
    assert!(
        matches!(result, IngestOutcome::Delivered { .. }),
        "a known parent must release its parked child: {result:?}"
    );
    assert_eq!(restored.requests().count(), 2);
}

#[test]
fn feishu_new_confirmed_repeat_root_owns_later_aliases() {
    let (mut state, work, _, worker) = fixture();
    let add = |state: &mut BusState, now| {
        let previous = state.clone();
        let source = state
            .post_to_developer(
                work,
                worker,
                "[waiting-on-you] Choose rollout".into(),
                vec![],
                now,
            )
            .unwrap();
        state.prepare_feishu_alerts(&previous, "/tmp/private-bus-session", "ou_developer", now);
        source
    };
    let first = add(&mut state, 1_000);
    let first_code = state
        .feishu
        .outbox
        .values()
        .find(|record| record.source.prompt_id == first)
        .unwrap()
        .code
        .clone();
    // The explicit repeat is admitted while the original send has no receipt.
    let repeated = add(&mut state, 11_000);
    let repeated_code = state
        .feishu
        .outbox
        .values()
        .find(|record| record.source.prompt_id == repeated)
        .unwrap()
        .code
        .clone();
    assert_ne!(first_code, repeated_code);
    // Serial transport can subsequently return receipts for both sends.
    state
        .complete_feishu_send(&first_code, "om_first", "oc_dm")
        .unwrap();
    state
        .complete_feishu_send(&repeated_code, "om_repeat", "oc_dm")
        .unwrap();
    let later = add(&mut state, 21_000);
    assert_eq!(state.feishu.outbox.len(), 2);
    let new_root = &state.feishu.outbox[&repeated_code];
    assert_eq!(
        new_root.aliases.len(),
        1,
        "the explicit repeat became the new root under decision 49695"
    );
    assert_eq!(new_root.aliases[0].source.prompt_id, later);
    assert!(
        state.feishu.outbox[&first_code].aliases.is_empty(),
        "a late old receipt must not take aliases back from the new root"
    );
}
