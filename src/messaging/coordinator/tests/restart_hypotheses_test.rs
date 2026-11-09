use super::*;

fn save_hypothesis(f: &Fixture, label: &str) {
    if let Some(root) = std::env::var_os("BUS_319_PROBE_EVIDENCE_DIR") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join(format!("{label}.json")),
            serde_json::to_vec_pretty(&json!({
                "state": f.worker.state,
                "serving_generation": f.worker.serving_generation,
                "native_calls": format!("{:?}", f.native.lock().unwrap()),
                "rollout": std::fs::read_to_string(f.dir.join("rollout-same-turn-pending.jsonl")).ok(),
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

fn same_turn_pending(restart: bool, stop_spooled: bool) {
    let mut f = Fixture::new(Provider::Codex);
    f.worker.transport = restart_generation_proof_native(&f, 1);
    f.worker.tick().unwrap();
    let rollout = f.dir.join("rollout-same-turn-pending.jsonl");
    let lead = f.send("same turn lead task", false);
    f.deliver();
    let lead_payload = f.typed().last().unwrap().0.clone();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"same turn lead task","transcript_path":rollout}),
    );
    f.consume();
    f.status(RuntimeStatus::Working);
    let member = f.send("same turn steered followup", false);
    f.deliver();
    let member_payload = f.typed().last().unwrap().0.clone();
    assert!(f.request(member).steered && !f.request(member).trusted_start_bound);
    let turn = codex_turn(
        &rollout_at(&f, 1),
        "turn-1",
        &lead_payload,
        Some("shared attributable final"),
    );
    let (head, complete) = turn.trim_end().rsplit_once('\n').unwrap();
    let claimed = json!({"timestamp":rollout_at(&f, 1),"type":"event_msg",
        "payload":{"type":"item_completed","turn_id":"turn-1",
        "item":{"type":"UserMessage","content":[{"type":"text","text":member_payload}]}}});
    append(&rollout, &format!("{head}\n{claimed}\n{complete}\n"));
    if stop_spooled {
        f.stopped("turn-1", "shared attributable final");
        f.consume();
        assert_eq!(f.request(lead).phase, RequestPhase::Completed);
        assert!(f.request(lead).pending_final.is_some());
        assert!(!f.request(member).trusted_start_bound);
    }
    let previous_reply = f.request(lead).completed_reply.clone();
    let label = format!("same-turn-restart-{restart}-stop-{stop_spooled}");
    save_hypothesis(&f, &format!("{label}-before"));
    if restart {
        restart_lifecycle_reopen(&mut f, true);
    } else {
        f.worker.transport = restart_generation_proof_native(&f, 1);
    }
    f.worker.tick().unwrap();
    save_hypothesis(&f, &format!("{label}-after"));
    for id in [lead, member] {
        assert_eq!(f.request(id).phase, RequestPhase::Completed, "{label}");
        assert!(f.reply(id).is_none(), "{label}");
        assert!(f.request(id).failure_reason.is_none(), "{label}");
        assert!(!f.request(id).uncertain_outcome, "{label}");
    }
    assert_reply_text_absent(&f, "shared attributable final");
    if previous_reply.is_some() {
        assert_eq!(f.request(lead).completed_reply, previous_reply);
    }
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    assert_eq!(f.request(lead).reply_key(), f.request(member).reply_key());
    assert_eq!(f.worker.state.room(f.room).unwrap().unread_count, 0);
    assert!(f
        .worker
        .state
        .room(f.room)
        .unwrap()
        .latest_replies
        .is_empty());
    let queued = f.send("fresh queue after shared final", true);
    f.worker.tick().unwrap();
    assert_eq!(f.request(queued).phase, RequestPhase::Active);
    assert_eq!(
        f.typed()
            .iter()
            .filter(|(text, _)| text.contains("fresh queue after shared final"))
            .count(),
        1
    );
}

#[test]
fn restart_hypothesis_same_turn_completed_pending_anchor() {
    same_turn_pending(true, true);
}

#[test]
fn restart_hypothesis_same_turn_same_generation_control() {
    same_turn_pending(false, true);
}

#[test]
fn restart_hypothesis_same_turn_no_stop_control() {
    same_turn_pending(true, false);
}

/// Replay the pre-generation coordinator's actual begin/save/native/result
/// sequence. No JSON field is removed and no request phase is manufactured.
fn legacy_awaiting_start() -> (Fixture, RequestId) {
    let mut f = Fixture::new(Provider::Codex);
    let request = f.send("legacy awaiting-start input", false);
    let mut state = f.worker.state.clone();
    state.begin_submission(request, "launch", 0).unwrap();
    let text = state.render_request_payload(request).unwrap();
    f.worker.save(state).unwrap();
    let result =
        f.worker
            .transport
            .request(Method::AgentPromptIfIdle(schema::AgentPromptIfIdleParams {
                target: "pane".into(),
                text,
                expected_terminal_id: "terminal".into(),
                expected_pane_id: "pane".into(),
                expected_agent: "codex".into(),
                expected_session_id: "session".into(),
                steer: false,
                expected_generation: None,
            }));
    assert!(matches!(result, Ok(ResponseResult::AgentPrompted { .. })));
    let mut state = f.worker.state.clone();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(f.request(request).phase, RequestPhase::Active);
    assert!(!f.request(request).trusted_start_bound);
    assert!(f.request(request).delivery_epoch.is_none());
    assert!(serde_json::to_value(f.request(request))
        .unwrap()
        .get("delivery_epoch")
        .is_none());
    assert_eq!(f.typed().len(), 1);
    (f, request)
}

#[test]
fn restart_hypothesis_legacy_same_generation_reload_control() {
    let (mut f, old) = legacy_awaiting_start();
    let queued = f.send("queued after live legacy input", true);
    save_hypothesis(&f, "legacy-same-generation-before");
    reload(&mut f);
    f.worker.transport = restart_generation_proof_native(&f, 1);
    f.worker.tick().unwrap();
    save_hypothesis(&f, "legacy-same-generation-after-poll");
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(old)
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    f.started("turn-1", "legacy awaiting-start input");
    f.consume();
    f.stopped("turn-1", "live legacy final");
    f.consume();
    f.worker.tick().unwrap();
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "live legacy final");
    assert_eq!(f.request(queued).phase, RequestPhase::Active);
}

struct HypothesisNative {
    calls: Arc<Mutex<Vec<Method>>>,
    infos: Vec<schema::AgentInfo>,
}

impl Transport for HypothesisNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(
            &method,
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_)
        ) {
            self.calls.lock().unwrap().push(method);
            return Ok(ResponseResult::AgentList {
                agents: self.infos.clone(),
            });
        }
        Native(self.calls.clone()).request(method)
    }
}

fn hypothesis_info(
    f: &Fixture,
    agent: AgentId,
    generation: u64,
    terminal: &str,
    pane: &str,
    session: &str,
) -> schema::AgentInfo {
    serde_json::from_value(json!({
        "generation":generation,"terminal_id":terminal,"pane_id":pane,
        "name":crate::messaging::identity::managed_name(f.room,agent),
        "agent":"codex","agent_status":"idle","interactive_ready":true,
        "workspace_id":"workspace","tab_id":"tab","focused":false,"revision":1,
        "agent_session":{"source":crate::messaging::identity::managed_source(Provider::Codex),"agent":"codex","kind":"id","value":session}
    }))
    .unwrap()
}

#[test]
fn restart_hypothesis_legacy_closes_on_witness_generation_advance() {
    let (mut f, old) = legacy_awaiting_start();
    let queued = f.send("queued after interrupted legacy input", true);
    let mut state = f.worker.state.clone();
    let witness = state
        .create_agent(
            f.room,
            "generation-witness",
            Provider::Codex,
            f.dir.clone(),
            None,
        )
        .unwrap();
    state
        .set_agent_runtime_identity(
            witness,
            AgentRuntimeIdentity {
                launch_id: Some("witness-launch".into()),
                terminal_id: Some("witness-terminal".into()),
                pane_id: Some("witness-pane".into()),
                session_id: Some("witness-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(witness).unwrap();
    f.worker.save(state).unwrap();
    callbacks::initialize(
        &f.dir.join("callbacks/witness-launch"),
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(witness.0),
            provider: Provider::Codex,
            launch_id: "witness-launch".into(),
        },
    )
    .unwrap();
    let witness_g1 = hypothesis_info(
        &f,
        witness,
        1,
        "witness-terminal",
        "witness-pane",
        "witness-session",
    );
    f.worker.transport = Box::new(HypothesisNative {
        calls: f.native.clone(),
        infos: vec![witness_g1],
    });
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 1);
    assert_eq!(
        f.worker.state.agent(witness).unwrap().status,
        RuntimeStatus::Idle
    );
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    assert!(f.request(old).delivery_epoch.is_none());
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(old)
    );
    assert_eq!(
        f.typed().len(),
        1,
        "no input admitted while legacy native info is absent"
    );
    save_hypothesis(&f, "legacy-owned-witness-g1-before-restart");
    let witness_g2 = hypothesis_info(
        &f,
        witness,
        2,
        "witness-terminal",
        "witness-pane",
        "witness-session",
    );
    // A witness-only advance stamps this epoch-less request with the previous
    // serving generation and closes it on that same tick. The successor stays
    // queued until this agent's own info returns.
    f.worker.transport = Box::new(HypothesisNative {
        calls: f.native.clone(),
        infos: vec![witness_g2.clone()],
    });
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert_eq!(
        f.request(old).delivery_epoch.as_ref().unwrap().generation,
        1,
        "the witness advance closes the previous serving generation"
    );
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        None,
        "closing the legacy request frees the agent before its own info returns"
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert!(f.request(queued).delivery_epoch.is_none());
    assert_eq!(
        f.typed().len(),
        1,
        "no input while the native agent is absent"
    );
    save_hypothesis(&f, "legacy-g2-known-before-native-reattribution");
    let legacy_g2 = hypothesis_info(&f, f.agent, 2, "terminal", "pane", "session");
    f.worker.transport = Box::new(HypothesisNative {
        calls: f.native.clone(),
        infos: vec![legacy_g2, witness_g2],
    });
    f.worker.tick().unwrap();
    save_hypothesis(&f, "legacy-owned-witness-g2-after-restart");
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert_eq!(
        f.request(old).delivery_epoch.as_ref().unwrap().generation,
        1
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Active);
    assert_eq!(
        f.request(queued)
            .delivery_epoch
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(queued)
    );
    assert_eq!(
        f.typed().len(),
        2,
        "one original input and one successor, no replay"
    );
}
