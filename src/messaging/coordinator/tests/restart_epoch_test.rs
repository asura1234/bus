use super::*;

#[path = "restart_continuation_test.rs"]
mod restart_continuation_tests;

#[path = "detached_continuation_test.rs"]
mod detached_continuation_tests;

#[derive(Clone, Copy, Debug)]
enum EpochMode {
    Server,
    Full,
    Manual,
}

#[derive(Clone, Copy)]
enum Receipt {
    Early,
    Late,
    Replay,
}

fn epoch_transport(fixture: &Fixture, rebind: bool, generation: u64) -> Box<dyn Transport> {
    let kind = crate::messaging::identity::provider_kind(fixture.provider).label();
    let info = serde_json::from_value(json!({
        "generation": generation,
        "terminal_id": if rebind { "restored-terminal" } else { "terminal" },
        "pane_id": if rebind { "restored-pane" } else { "pane" },
        "name": crate::messaging::identity::managed_name(fixture.room, fixture.agent),
        "agent": kind, "agent_status": "idle", "interactive_ready": true, "native_ready": true,
        "workspace_id": "workspace", "tab_id": "tab", "focused": false, "revision": 1,
        "agent_session": {"source": crate::messaging::identity::managed_source(fixture.provider),
            "agent": kind, "kind": "id", "value": "session"}
    }))
    .unwrap();
    Box::new(RestartLifecycleNative {
        methods: fixture.native.clone(),
        info,
    })
}

fn epoch_reopen(fixture: &mut Fixture, rebind: bool, generation: u64) {
    let placeholder = Worker::open(
        fixture.dir.join("epoch-placeholder"),
        Box::new(Native(fixture.native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut fixture.worker, placeholder));
    fixture.worker = Worker::open(
        fixture.dir.clone(),
        epoch_transport(fixture, rebind, generation),
    )
    .unwrap();
}

fn observe_epoch(fixture: &mut Fixture, generation: u64) {
    fixture.worker.transport = epoch_transport(fixture, false, generation);
    fixture.worker.tick().unwrap();
}

fn delivery_generation(fixture: &Fixture, id: RequestId) -> u64 {
    fixture
        .request(id)
        .delivery_epoch
        .as_ref()
        .unwrap()
        .generation
}

fn copies_of(fixture: &Fixture, text: &str) -> usize {
    fixture
        .worker
        .state
        .requests()
        .filter(|request| {
            request.prompt.text.contains(text)
                || request
                    .reply()
                    .is_some_and(|reply| reply.text.contains(text))
        })
        .count()
}

fn hold_transient_error(fixture: &mut Fixture, request: RequestId) {
    let mut document = serde_json::to_value(&fixture.worker.state).unwrap();
    document["agents"][fixture.agent.0.to_string()]["provider_retry"] = json!({
        "lead": request,
        "attempts": 1,
        "retry_at_ms": u64::MAX,
        "quota": false,
        "queued": false,
    });
    fixture
        .worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
}

fn epoch_ordering(mode: EpochMode, current: bool, receipt: Receipt, queue_after: bool) {
    for provider in [
        Provider::ClaudeCode,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
        Provider::Kimi,
    ] {
        for bound in [false, true] {
            for transient_hold in [false, true] {
                if transient_hold && provider == Provider::Cursor {
                    continue;
                }
                let mut f = Fixture::new(provider);
                let turn = if provider == Provider::Kimi {
                    "1"
                } else {
                    "turn-1"
                };
                if provider == Provider::Kimi {
                    super::kimi_steering_tests::home(&f);
                }
                f.worker.transport = epoch_transport(&f, false, 1);
                f.worker.tick().unwrap();
                assert_eq!(f.worker.serving_generation, 1);
                let old = if current {
                    if bound || transient_hold {
                        Some(running(&mut f, "old generation request", turn))
                    } else {
                        let id = f.send("old awaiting-start request", false);
                        f.deliver();
                        Some(id)
                    }
                } else if transient_hold {
                    Some(f.send("queued retry lead", true))
                } else {
                    None
                };
                if transient_hold {
                    let old = old.unwrap();
                    hold_transient_error(&mut f, old);
                    if !current {
                        let mut document = serde_json::to_value(&f.worker.state).unwrap();
                        document["agents"][f.agent.0.to_string()]["provider_retry"]["queued"] =
                            json!(true);
                        document["requests"][old.0.to_string()]["provider_retry_attempts"] =
                            json!(1);
                        f.worker
                            .save(serde_json::from_value(document).unwrap())
                            .unwrap();
                    }
                }
                let queued = (!queue_after).then(|| f.send("queued before resume", true));
                if current && !transient_hold {
                    let request = f.request(old.unwrap());
                    assert_eq!(f.worker.serving_generation, 1, "{provider:?}/{mode:?}");
                    assert_eq!(request.phase, RequestPhase::Active, "{provider:?}/{mode:?}");
                    assert_eq!(
                        request.delivery_epoch.as_ref().unwrap().generation,
                        1,
                        "{provider:?}/{mode:?}"
                    );
                }
                match mode {
                    EpochMode::Server => {
                        f.worker.transport = epoch_transport(&f, false, 2);
                        assert_eq!(
                            f.worker.serving_generation, 1,
                            "{mode:?} keeps the open worker"
                        );
                    }
                    EpochMode::Full => {
                        epoch_reopen(&mut f, true, 2);
                        assert_eq!(
                            f.worker.serving_generation, 0,
                            "{mode:?} opens a new worker"
                        );
                    }
                    // Manual reopens the process in the same terminal. Full also rebinds.
                    EpochMode::Manual => {
                        epoch_reopen(&mut f, false, 2);
                        assert_eq!(
                            f.worker.serving_generation, 0,
                            "{mode:?} opens a new worker in the same terminal"
                        );
                    }
                }
                if matches!(receipt, Receipt::Early) {
                    restart_lifecycle_receipt(&f, "restarted");
                }
                let queued = queue_after
                    .then(|| f.send("queued after resume", true))
                    .or(queued);
                let held = f
                    .worker
                    .state
                    .agent(f.agent)
                    .unwrap()
                    .provider_retry
                    .clone();
                let held_queue = f.worker.state.queued_requests(f.agent).to_vec();
                f.worker.tick().unwrap();
                assert_eq!(f.worker.serving_generation, 2, "{provider:?}/{mode:?}");
                assert_eq!(
                    f.worker
                        .state
                        .agent(f.agent)
                        .unwrap()
                        .runtime_identity
                        .terminal_id
                        .as_deref(),
                    match mode {
                        EpochMode::Full => Some("restored-terminal"),
                        EpochMode::Server | EpochMode::Manual => Some("terminal"),
                    },
                    "{provider:?}/{mode:?}"
                );
                if matches!(receipt, Receipt::Late | Receipt::Replay) {
                    restart_lifecycle_receipt(&f, "restarted");
                    f.worker.tick().unwrap();
                }
                if matches!(receipt, Receipt::Replay) {
                    let phases: Vec<_> = [old, queued]
                        .into_iter()
                        .flatten()
                        .map(|id| (id, f.request(id).phase))
                        .collect();
                    let typed = f.typed().len();
                    restart_lifecycle_receipt(&f, "restarted");
                    f.worker.tick().unwrap();
                    for (id, phase) in phases {
                        assert_eq!(f.request(id).phase, phase, "receipt replay changed {id:?}");
                    }
                    assert_eq!(f.typed().len(), typed, "receipt replay duplicated delivery");
                }
                if transient_hold {
                    let request = f.request(old.unwrap());
                    assert_ne!(
                        request.phase,
                        RequestPhase::Abandoned,
                        "{provider:?}/{mode:?}"
                    );
                    assert_eq!(request.phase == RequestPhase::Queued, !current);
                    if current {
                        assert_eq!(
                            request.delivery_epoch.as_ref().unwrap().generation,
                            1,
                            "{provider:?}/{mode:?} hold must stay on its delivery generation"
                        );
                    }
                    assert_eq!(
                        f.worker.state.agent(f.agent).unwrap().provider_retry,
                        held,
                        "task, attempts and original deadline must survive"
                    );
                    assert_eq!(
                        f.worker.state.queued_requests(f.agent),
                        held_queue,
                        "retry hold must retain FIFO without new input"
                    );
                } else if current {
                    let request = f.request(old.unwrap());
                    if bound {
                        assert!(!request.settled(), "{provider:?}/{mode:?}: {request:?}");
                        assert_eq!(delivery_generation(&f, request.id), 2);
                        assert_eq!(
                            f.worker.state.agent(f.agent).unwrap().current_request,
                            Some(request.id)
                        );
                        let notice = f.typed().last().unwrap().0.clone();
                        assert!(notice.contains("Bus restarted"), "{notice}");
                        assert!(!notice.contains("old generation request"), "{notice}");
                    } else {
                        assert_eq!(request.phase, RequestPhase::Abandoned);
                        assert_eq!(delivery_generation(&f, request.id), 1);
                        assert_eq!(
                            request.failure_reason.as_deref(),
                            Some("reply_not_captured")
                        );
                    }
                }
                if !transient_hold {
                    if let Some(queued) = queued {
                        if current && bound {
                            assert_eq!(f.request(queued).phase, RequestPhase::Queued);
                            assert!(f.request(queued).delivery_epoch.is_none());
                            continue;
                        }
                        assert_ne!(
                            f.request(queued).phase,
                            RequestPhase::Queued,
                            "{provider:?}/{mode:?}/bound={bound}/hold={transient_hold}: {:?}",
                            f.request(queued)
                        );
                        assert_eq!(
                            f.request(queued)
                                .delivery_epoch
                                .as_ref()
                                .unwrap()
                                .generation,
                            2
                        );
                        let text = if queue_after {
                            "queued after resume"
                        } else {
                            "queued before resume"
                        };
                        assert_eq!(
                            f.typed()
                                .iter()
                                .filter(|(payload, _)| payload.contains(text))
                                .count(),
                            1,
                            "every queued successor must be delivered exactly once"
                        );
                    }
                }
            }
        }
    }
}

macro_rules! epoch_case {
    ($name:ident, $mode:ident, $current:expr, $receipt:ident, $queue:ident) => {
        #[test]
        fn $name() {
            epoch_ordering(EpochMode::$mode, $current, Receipt::$receipt, $queue);
        }
    };
}

epoch_case!(restart_epoch_c01, Server, true, Early, false);
epoch_case!(restart_epoch_c02, Server, true, Early, true);
epoch_case!(restart_epoch_c03, Server, true, Late, false);
epoch_case!(restart_epoch_c04, Server, true, Late, true);
epoch_case!(restart_epoch_c05, Server, true, Replay, false);
epoch_case!(restart_epoch_c06, Server, true, Replay, true);
epoch_case!(restart_epoch_c07, Server, false, Early, false);
epoch_case!(restart_epoch_c08, Server, false, Early, true);
epoch_case!(restart_epoch_c09, Server, false, Late, false);
epoch_case!(restart_epoch_c10, Server, false, Late, true);
epoch_case!(restart_epoch_c11, Server, false, Replay, false);
epoch_case!(restart_epoch_c12, Server, false, Replay, true);
epoch_case!(restart_epoch_c13, Full, true, Early, false);
epoch_case!(restart_epoch_c14, Full, true, Early, true);
epoch_case!(restart_epoch_c15, Full, true, Late, false);
epoch_case!(restart_epoch_c16, Full, true, Late, true);
epoch_case!(restart_epoch_c17, Full, true, Replay, false);
epoch_case!(restart_epoch_c18, Full, true, Replay, true);
epoch_case!(restart_epoch_c19, Full, false, Early, false);
epoch_case!(restart_epoch_c20, Full, false, Early, true);
epoch_case!(restart_epoch_c21, Full, false, Late, false);
epoch_case!(restart_epoch_c22, Full, false, Late, true);
epoch_case!(restart_epoch_c23, Full, false, Replay, false);
epoch_case!(restart_epoch_c24, Full, false, Replay, true);
epoch_case!(restart_epoch_c25, Manual, true, Early, false);
epoch_case!(restart_epoch_c26, Manual, true, Early, true);
epoch_case!(restart_epoch_c27, Manual, true, Late, false);
epoch_case!(restart_epoch_c28, Manual, true, Late, true);
epoch_case!(restart_epoch_c29, Manual, true, Replay, false);
epoch_case!(restart_epoch_c30, Manual, true, Replay, true);
epoch_case!(restart_epoch_c31, Manual, false, Early, false);
epoch_case!(restart_epoch_c32, Manual, false, Early, true);
epoch_case!(restart_epoch_c33, Manual, false, Late, false);
epoch_case!(restart_epoch_c34, Manual, false, Late, true);
epoch_case!(restart_epoch_c35, Manual, false, Replay, false);
epoch_case!(restart_epoch_c36, Manual, false, Replay, true);

#[test]
fn restart_epoch_c37_pre_stop_final_stays_identical() {
    let mut f = Fixture::new(Provider::Codex);
    let old = running(&mut f, "finish before restart", "turn-1");
    f.stopped("turn-1", "exact pre-stop answer");
    f.consume();
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "exact pre-stop answer");
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "exact pre-stop answer");
}

#[test]
fn restart_epoch_c38_consumed_codex_rollout_recovers_without_fresh_hook() {
    let mut f = Fixture::new(Provider::Codex);
    let rollout = f.dir.join("rollout-before-stop.jsonl");
    let old = f.send("finish before restart without Stop", false);
    f.deliver();
    f.record(json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"finish before restart without Stop","transcript_path":rollout}));
    f.consume();
    f.status(RuntimeStatus::Working);
    let typed = f.typed().last().unwrap().0.clone();
    append(
        &rollout,
        &codex_turn(
            &rollout_at(&f, 1),
            "turn-1",
            &typed,
            Some("committed final"),
        ),
    );
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "committed final");
}

#[cfg(unix)]
#[test]
fn restart_epoch_c39_owned_kimi_wire_wins_before_interruption_fallback() {
    let mut f = Fixture::new(Provider::Kimi);
    let wire = super::kimi_steering_tests::home(&f);
    let old = running(&mut f, "finish on the Kimi wire", "1");
    let prompt = f.worker.state.render_request_payload(old).unwrap();
    let rows = [
        json!({"type":"turn.prompt","agentId":"main","turnId":1,"promptId":"p1","input":[{"type":"text","text":prompt}]}),
        json!({"type":"agent.message.appended","message":{"meta":{"source":"llm"},"message":{"role":"assistant","content":[{"type":"text","text":"Kimi final"}]}}}),
        json!({"type":"agent.turn.ended","turnId":1,"outcome":"done"}),
        json!({"type":"turn.ended","agentId":"main","turnId":1,"reason":"completed"}),
        json!({"type":"prompt.completed","agentId":"main","promptId":"p1","reason":"completed"}),
    ];
    std::fs::write(
        wire,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "Kimi final");
}

#[test]
fn restart_epoch_c40_mixed_group_filters_each_delivery_generation() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = running(&mut f, "old lead", "turn-1");
    let newer = f.send("current-generation member", false);
    f.deliver();
    let identity = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .runtime_identity
        .clone();
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["requests"][old.0.to_string()]["delivery_epoch"] = json!({
        "generation": 1,
        "terminal_id": identity.terminal_id,
        "pane_id": identity.pane_id,
    });
    document["requests"][newer.0.to_string()]["delivery_epoch"] = json!({
        "generation": 2,
        "terminal_id": identity.terminal_id,
        "pane_id": identity.pane_id,
    });
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    assert_eq!(f.worker.serving_generation, 1);
    assert_ne!(f.request(old).phase, RequestPhase::Abandoned);
    assert_ne!(f.request(newer).phase, RequestPhase::Abandoned);
    assert_eq!(delivery_generation(&f, old), 1);
    assert_eq!(delivery_generation(&f, newer), 2);
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Queued);
    assert_eq!(delivery_generation(&f, old), 2);
    assert!(f.request(old).failure_reason.is_none());
    assert_eq!(
        f.request(newer).phase,
        RequestPhase::Active,
        "{:?}",
        f.request(newer)
    );
    assert_eq!(delivery_generation(&f, newer), 2);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(newer),
        "a current-generation delivery must retain an execution owner"
    );
    f.started("turn-2", "current-generation member");
    f.consume();
    f.stopped("turn-2", "new-generation answer");
    f.consume();
    assert_completed_without_reply(&f, newer);
    assert_reply_text_absent(&f, "new-generation answer");
    f.status(RuntimeStatus::Idle);
    f.deliver();
    let notice = f.typed().last().unwrap().0.clone();
    assert!(notice.contains("Bus restarted"));
    f.started("turn-3", &notice);
    f.consume();
    f.stopped("turn-3", "resumed older task completed");
    f.consume();
    assert_completed_without_reply(&f, old);
}

#[test]
fn restart_epoch_c47_steering_uses_the_generation_fenced_method() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    running(&mut f, "old lead", "turn-1");
    let member = f.send("generation-fenced followup", false);
    f.deliver();
    assert_eq!(
        f.request(member)
            .delivery_epoch
            .as_ref()
            .unwrap()
            .generation,
        1
    );
    assert!(
        f.native.lock().unwrap().iter().any(|method| matches!(
            method,
            Method::AgentPromptIfIdleAtGeneration(params)
                if params.steer && params.expected_generation == Some(1)
        )),
        "steering must not bypass the serving-generation fence"
    );
}

#[test]
fn restart_epoch_c45_retry_hold_keeps_all_submitted_group_members() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let lead = running(&mut f, "held lead", "turn-1");
    let member = f.send("held followup", false);
    f.deliver();
    assert_eq!(f.request(member).group, Some(lead));
    hold_transient_error(&mut f, lead);
    assert_eq!(delivery_generation(&f, lead), 1);
    assert_eq!(delivery_generation(&f, member), 1);
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert_ne!(f.request(lead).phase, RequestPhase::Abandoned);
    assert_ne!(f.request(member).phase, RequestPhase::Abandoned);
    assert_eq!(delivery_generation(&f, lead), 1);
    assert_eq!(delivery_generation(&f, member), 1);
    assert_eq!(f.request(member).group, Some(lead));
}

#[test]
fn restart_epoch_c52_distinct_member_final_recovers_without_any_stop_hook() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let rollout = f.dir.join("rollout-distinct-members.jsonl");
    let lead = f.send("first committed task", false);
    f.deliver();
    let lead_payload = f.typed().last().unwrap().0.clone();
    f.record(
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
        "turn_id":"turn-1","prompt":"first committed task","transcript_path":rollout}),
    );
    f.consume();
    f.status(RuntimeStatus::Working);
    let member = f.send("second independently committed task", false);
    f.deliver();
    let member_payload = f.typed().last().unwrap().0.clone();
    append(
        &rollout,
        &codex_turn(
            &rollout_at(&f, 1),
            "turn-1",
            &lead_payload,
            Some("first final"),
        ),
    );
    append(
        &rollout,
        &codex_turn(
            &rollout_at(&f, 2),
            "turn-2",
            &member_payload,
            Some("second final"),
        ),
    );
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_completed_without_reply(&f, lead);
    assert_completed_without_reply(&f, member);
    assert_reply_text_absent(&f, "first final");
    assert_reply_text_absent(&f, "second final");
}

#[test]
fn restart_epoch_c51_other_agents_final_is_read_before_interruption_fallback() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let first = running(&mut f, "first interrupted agent", "turn-1");
    let mut state = f.worker.state.clone();
    let other = state
        .create_agent(f.room, "other", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            other,
            AgentRuntimeIdentity {
                launch_id: Some("other-launch".into()),
                terminal_id: Some("other-terminal".into()),
                pane_id: Some("other-pane".into()),
                session_id: Some("other-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(other).unwrap();
    state.observe_status(other, RuntimeStatus::Idle, 1).unwrap();
    f.worker.save(state).unwrap();
    callbacks::initialize(
        &f.dir.join("callbacks/other-launch"),
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(other.0),
            provider: Provider::Codex,
            launch_id: "other-launch".into(),
        },
    )
    .unwrap();
    f.agent = other;
    let completed = f.send("other agent completed before restart", false);
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    let rollout = f.dir.join("rollout-other-agent.jsonl");
    append(
        &rollout,
        &codex_turn(
            &rollout_at(&f, 1),
            "other-turn",
            &payload,
            Some("other final"),
        ),
    );
    f.worker.codex_rollouts.insert(other, rollout);
    f.agent = f.request(first).agent_id;
    assert_eq!(f.worker.serving_generation, 1);
    assert_eq!(f.request(first).phase, RequestPhase::Active);
    assert_eq!(delivery_generation(&f, first), 1);
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert!(!f.request(first).settled());
    assert_eq!(delivery_generation(&f, first), 2);
    assert!(f.request(first).failure_reason.is_none());
    assert_completed_without_reply(&f, completed);
    assert_reply_text_absent(&f, "other final");
}

#[test]
fn restart_epoch_c41_same_server_reload_keeps_live_turn() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = running(&mut f, "same generation live turn", "turn-1");
    epoch_reopen(&mut f, false, 1);
    f.worker.tick().unwrap();
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    f.stopped("turn-1", "completed after reload");
    f.consume();
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "completed after reload");
}

#[test]
fn restart_epoch_c42_refusal_does_not_advance_or_close_live_work() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = f.send("live awaiting start", false);
    f.deliver();
    f.worker.transport = epoch_transport(&f, false, 1);
    restart_lifecycle_receipt(&f, "refused");
    f.worker.tick().unwrap();
    assert_eq!(f.request(old).phase, RequestPhase::Active);
}

#[test]
fn restart_epoch_c43_rollback_receipt_leaves_generation_one_active() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = f.send("rollback interrupted request", false);
    f.deliver();
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    assert_eq!(delivery_generation(&f, old), 1);
    f.worker.transport = epoch_transport(&f, false, 1);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 1);
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    assert_eq!(delivery_generation(&f, old), 1);
    // rolled_back is a diagnostic receipt. It must not settle this request.
    let path = crate::utils::restart_record::outcome_path(&f.dir);
    restart_lifecycle_receipt(&f, "rolled_back");
    assert!(path.exists());
    f.worker.tick().unwrap();
    assert!(
        !path.exists(),
        "rolled_back receipt must be consumed without a notice"
    );
    assert_eq!(f.worker.serving_generation, 1);
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    assert_eq!(delivery_generation(&f, old), 1);
    assert!(f.request(old).failure_reason.is_none());
    assert!(f
        .worker
        .state
        .requests()
        .all(|request| !request.prompt.text.starts_with("Restarted ")));
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    assert_eq!(delivery_generation(&f, old), 1);
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

#[test]
fn restart_epoch_c44_later_server_interruption_has_its_own_generation() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = f.send("request before independent server restart", false);
    f.deliver();
    epoch_reopen(&mut f, true, 1);
    f.worker.tick().unwrap();
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    assert_eq!(f.worker.serving_generation, 1);
    assert_eq!(delivery_generation(&f, old), 1);
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(delivery_generation(&f, old), 1);
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

#[test]
fn restart_epoch_c53_retry_hold_keeps_identity_and_uses_new_generation() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = running(&mut f, "retry after transient failure", "turn-1");
    hold_transient_error(&mut f, old);
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .provider_retry
        .is_some());
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(
        f.request(old).delivery_epoch.as_ref().unwrap().generation,
        1
    );
    assert!(f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .provider_retry
        .is_some());
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["agents"][f.agent.0.to_string()]["provider_retry"]["retry_at_ms"] = json!(0);
    document["agents"][f.agent.0.to_string()]["provider_retry"]["queued"] = json!(false);
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    f.worker.tick().unwrap();
    assert_eq!(
        f.request(old).delivery_epoch.as_ref().unwrap().generation,
        2
    );
}

#[test]
fn restart_epoch_c46_explicit_final_is_not_replayed_by_receipts() {
    let mut f = Fixture::new(Provider::Codex);
    let old = running(&mut f, "finish explicitly", "turn-1");
    f.stopped("turn-1", "one answer only");
    f.consume();
    let phase = f.request(old).phase;
    let epoch = f.request(old).delivery_epoch.clone();
    let typed = f.typed().len();
    assert_eq!(phase, RequestPhase::Completed);
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "one answer only");
    assert_eq!(copies_of(&f, "one answer only"), 0);
    let path = crate::utils::restart_record::outcome_path(&f.dir);
    restart_lifecycle_receipt(&f, "restarted");
    assert!(path.exists());
    f.worker.tick().unwrap();
    assert!(!path.exists(), "restarted receipt must be consumed");
    assert_eq!(f.request(old).phase, phase);
    assert_eq!(f.request(old).delivery_epoch, epoch);
    assert_eq!(f.typed().len(), typed);
    assert_eq!(copies_of(&f, "one answer only"), 0);
    assert_completed_without_reply(&f, old);
    assert_reply_text_absent(&f, "one answer only");
}

#[test]
fn restart_epoch_c54_save_failure_blocks_new_input_until_reconciled() {
    use crate::messaging::storage::state_store::SaveStage;
    for stage in [
        SaveStage::TempCreate,
        SaveStage::TempWrite,
        SaveStage::TempSync,
        SaveStage::Replace,
        SaveStage::ParentSync,
    ] {
        for reopen in [false, true] {
            let mut f = Fixture::new(Provider::Codex);
            observe_epoch(&mut f, 1);
            let old = f.send("request before durable restart", false);
            f.deliver();
            let queued = f.send("queued behind durable restart", true);
            let before = f.typed().len();
            assert_eq!(f.worker.serving_generation, 1);
            assert_eq!(f.request(old).phase, RequestPhase::Active);
            assert_eq!(delivery_generation(&f, old), 1);
            f.worker.transport = epoch_transport(&f, false, 2);
            f.worker.store.fail_once_at(stage);
            assert!(f.worker.tick().is_err(), "{stage:?}");
            assert_eq!(f.typed().len(), before, "{stage:?}");
            assert_eq!(f.request(old).phase, RequestPhase::Active, "{stage:?}");
            assert_eq!(delivery_generation(&f, old), 1, "{stage:?}");
            if reopen {
                epoch_reopen(&mut f, false, 2);
            }
            f.worker.tick().unwrap();
            assert_eq!(f.worker.serving_generation, 2, "{stage:?}");
            assert_eq!(f.request(old).phase, RequestPhase::Abandoned, "{stage:?}");
            assert_eq!(delivery_generation(&f, old), 1, "{stage:?}");
            assert_eq!(
                f.request(old).failure_reason.as_deref(),
                Some("reply_not_captured"),
                "{stage:?}"
            );
            assert_eq!(f.typed().len(), before + 1, "{stage:?}/reopen={reopen}");
            assert_eq!(
                f.request(queued)
                    .delivery_epoch
                    .as_ref()
                    .unwrap()
                    .generation,
                2
            );
        }
    }
}

#[test]
fn restart_epoch_c48_legacy_unknown_metadata_reconciles_at_native_change() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = f.send("legacy request without delivery metadata", false);
    f.deliver();
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["requests"][old.0.to_string()]
        .as_object_mut()
        .unwrap()
        .remove("delivery_epoch");
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    assert!(f.request(old).delivery_epoch.is_none());
    assert_eq!(f.request(old).phase, RequestPhase::Active);
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    // A new process has no previous serving generation. The rebound terminal
    // does not match, so migration stamps generation 0 and serving 2 closes it.
    assert_eq!(delivery_generation(&f, old), 0);
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
}

#[test]
fn restart_epoch_c49_owned_transient_error_creates_hold_not_abandonment() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 1);
    let old = running(&mut f, "hold transient failure", "turn-1");
    let rollout = f.dir.join("rollout-transient-error.jsonl");
    std::fs::write(
        &rollout,
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1",
            "last_agent_message":null,"error":{"message":"Selected model is at capacity.",
            "codex_error_info":"server_overloaded"}}})
        .to_string()
            + "\n",
    )
    .unwrap();
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["agents"][f.agent.0.to_string()]["codex_rollout"] = json!(rollout.clone());
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    f.worker.codex_rollouts.insert(f.agent, rollout);
    f.worker.transport = epoch_transport(&f, false, 2);
    f.worker.tick().unwrap();
    assert_eq!(
        f.worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .as_ref()
            .map(|hold| hold.lead),
        Some(old)
    );
    assert_ne!(f.request(old).phase, RequestPhase::Abandoned);
}

#[test]
fn restart_epoch_c50_reordered_receipt_cannot_capture_new_epoch_work() {
    let mut f = Fixture::new(Provider::Codex);
    let old = f.send("old epoch request", false);
    f.deliver();
    let queued = f.send("new epoch successor", true);
    epoch_reopen(&mut f, true, 2);
    f.worker.tick().unwrap();
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    assert_eq!(delivery_generation(&f, old), 0);
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Active);
    assert_eq!(delivery_generation(&f, queued), 2);
    let typed = f.typed().len();
    let queued_epoch = f.request(queued).delivery_epoch.clone();
    let old_epoch = f.request(old).delivery_epoch.clone();
    let path = crate::utils::restart_record::outcome_path(&f.dir);
    restart_lifecycle_receipt(&f, "restarted");
    assert!(path.exists());
    f.worker.tick().unwrap();
    assert!(!path.exists(), "restarted receipt must be consumed");
    assert_eq!(f.worker.serving_generation, 2);
    assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
    assert_eq!(f.request(old).delivery_epoch, old_epoch);
    assert_eq!(f.request(queued).phase, RequestPhase::Active);
    assert_eq!(f.request(queued).delivery_epoch, queued_epoch);
    assert_eq!(f.typed().len(), typed);
}

#[test]
fn restart_published_sessionless_unknown_submit_settles_on_new_generation_even_without_rebind() {
    struct LostAcknowledgment(Arc<Mutex<Vec<Method>>>);
    impl Transport for LostAcknowledgment {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            assert!(matches!(
                method,
                Method::AgentPromptIfUnboundAtGeneration(_)
            ));
            self.0.lock().unwrap().push(method);
            Err(TransportError {
                code: None,
                message: "connection lost after input".into(),
                definitely_rejected: false,
                timestamps: Default::default(),
            })
        }
    }
    for reopen in [false, true] {
        let mut f = Fixture::new(Provider::Codex);
        observe_epoch(&mut f, 1);
        let mut state = f.worker.state.clone();
        let mut identity = state.agent(f.agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        state.set_agent_runtime_identity(f.agent, identity).unwrap();
        f.worker.save(state).unwrap();
        f.worker.transport = Box::new(LostAcknowledgment(f.native.clone()));
        let old = f.send(
            "input before SessionStart and lost submit acknowledgment",
            false,
        );
        f.deliver();
        assert_eq!(f.request(old).phase, RequestPhase::Submitting);
        assert!(f.request(old).provider_session_id.is_none());
        let pending = f.send("retain until native startup succeeds", true);
        let pending_before = f.request(pending).clone();
        let inputs = f
            .native
            .lock()
            .unwrap()
            .iter()
            .filter(|method| {
                matches!(
                    method,
                    Method::AgentPromptIfUnboundAtGeneration(_)
                        | Method::AgentPromptIfIdleAtGeneration(_)
                )
            })
            .count();
        assert_eq!(inputs, 1);
        if reopen {
            reload(&mut f);
        }
        for generation in [1, 2] {
            let info = serde_json::from_value(json!({
                "generation": generation,
                "terminal_id": "terminal", "pane_id": "failed-placeholder",
                "agent": null, "agent_status": "unknown", "interactive_ready": false,
                "workspace_id": "workspace", "tab_id": "tab", "focused": false,
                "revision": 1
            }))
            .unwrap();
            f.worker.transport = Box::new(RestartLifecycleNative {
                methods: f.native.clone(),
                info,
            });
            f.worker.tick().unwrap();
            assert_eq!(f.request(pending), &pending_before);
            assert_eq!(
                f.native
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|method| matches!(
                        method,
                        Method::AgentPromptIfUnboundAtGeneration(_)
                            | Method::AgentPromptIfIdleAtGeneration(_)
                    ))
                    .count(),
                inputs,
                "uncertain input must never replay"
            );
            if generation == 1 {
                assert_eq!(f.request(old).phase, RequestPhase::Submitting);
            } else {
                assert_eq!(f.request(old).phase, RequestPhase::Abandoned);
                assert_eq!(
                    f.request(old).failure_reason.as_deref(),
                    Some("reply_not_captured")
                );
            }
        }
    }
}
