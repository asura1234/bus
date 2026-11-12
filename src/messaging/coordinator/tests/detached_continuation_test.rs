use super::*;

// A native witness advances the generation even when the target's managed
// provider disappeared before save. The reused pane is never an owned agent.
struct DetachedNative {
    methods: Arc<Mutex<Vec<Method>>>,
    agent: AgentId,
    room: RoomId,
    provider: Provider,
    generation: u64,
    exited: bool,
    resumed: bool,
}

impl DetachedNative {
    fn info(&self, target: bool) -> schema::AgentInfo {
        let kind = crate::messaging::identity::provider_kind(self.provider).label();
        serde_json::from_value(json!({
            "generation": self.generation,
            "terminal_id": if target { "resumed-terminal" } else { "witness-terminal" },
            "pane_id": if target { "resumed-pane" } else { "witness-pane" },
            "name": if target { crate::messaging::identity::managed_name(self.room, self.agent) } else { "witness".into() },
            "agent": kind, "agent_status": "idle", "interactive_ready": true,
            "native_ready": true, "workspace_id":"w", "tab_id":"t", "focused":false, "revision":1,
            "agent_session":{"source":crate::messaging::identity::managed_source(self.provider),
                "agent":kind,"kind":"id","value":if target { "session" } else { "witness-session" }}
        })).unwrap()
    }
}

impl Transport for DetachedNative {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match &method {
            Method::AgentList(_) | Method::AgentListWithDetectionContext(_) => Ok(ResponseResult::AgentList {
                agents: vec![self.info(self.resumed)],
            }),
            Method::PaneGet(p) => Ok(ResponseResult::PaneInfo { pane: serde_json::from_value(json!({
                "pane_id":p.pane_id, "terminal_id":if self.exited { "terminal" } else { "replacement-shell" },
                "workspace_id":"w","tab_id":"t","focused":false,"agent_status":"unknown",
                "revision":1,"agent_process_exited":self.exited
            })).unwrap() }),
            Method::PaneRead(p) => Ok(ResponseResult::PaneRead { read: serde_json::from_value(json!({
                "pane_id":p.pane_id,"workspace_id":"w","tab_id":"t","source":p.source,
                "format":"text","text":"provider exited","revision":1,"truncated":false
            })).unwrap() }),
            Method::TabCreate(_) => Ok(ResponseResult::TabCreated {
                tab: serde_json::from_value(json!({"tab_id":"t","workspace_id":"w","number":1,"label":"resume","focused":false,"pane_count":1,"agent_status":"unknown"})).unwrap(),
                root_pane: serde_json::from_value(json!({"pane_id":"resumed-pane","terminal_id":"resumed-terminal","workspace_id":"w","tab_id":"t","focused":false,"agent_status":"unknown","revision":1})).unwrap(),
            }),
            Method::AgentStart(_) => {
                self.resumed = true;
                self.methods.lock().unwrap().push(method);
                Ok(ResponseResult::AgentStarted { agent:self.info(true), argv:vec!["provider".into()] })
            }
            Method::PaneCloseIfIdentity(_) => Ok(ResponseResult::Ok {}),
            _ => Native(self.methods.clone()).request(method),
        }
    }
}

fn detached_transport(f: &Fixture, generation: u64, exited: bool) -> Box<dyn Transport> {
    Box::new(DetachedNative {
        methods: f.native.clone(),
        agent: f.agent,
        room: f.room,
        provider: f.provider,
        generation,
        exited,
        resumed: false,
    })
}

fn reopen_detached(f: &mut Fixture, generation: u64) {
    reload(f);
    f.worker.transport = detached_transport(f, generation, false);
}

fn joined_task(f: &mut Fixture) -> (RequestId, RequestId, RequestId) {
    let lead = running(f, "ORIGINAL_TASK_NEVER_REPLAY", "turn-1");
    let member = f.send("ORIGINAL_STEERING_NEVER_REPLAY", false);
    f.deliver();
    f.started("turn-1", "ORIGINAL_STEERING_NEVER_REPLAY");
    f.consume();
    let queued = f.send("independent FIFO task", true);
    (lead, member, queued)
}

fn assert_waiting(
    f: &Fixture,
    lead: RequestId,
    member: RequestId,
    queued: RequestId,
    before: usize,
) {
    for id in [lead, member] {
        assert!(
            !f.request(id).settled(),
            "must retain original {id:?}: {:?}",
            f.request(id)
        );
        assert!(
            f.request(id).turn_ended_at_ms.is_none(),
            "runtime loss is not a native end"
        );
    }
    assert_eq!(f.request(member).group, Some(lead));
    let owner = f.worker.state.agent(f.agent).unwrap();
    assert_eq!(owner.status, RuntimeStatus::Unavailable);
    assert_eq!(owner.current_request, Some(lead));
    assert!(owner.runtime_identity.pane_id.is_none());
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert_eq!(
        f.typed().len(),
        before,
        "never type into the replacement shell"
    );
}

#[test]
fn detached_restart_waits_durably_for_verified_resume() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::new(provider);
        observe_epoch(&mut f, 4);
        let (lead, member, queued) = joined_task(&mut f);
        let before = f.typed().len();
        for generation in [5, 5, 6] {
            reopen_detached(&mut f, generation);
            f.worker.tick().unwrap();
            assert_waiting(&f, lead, member, queued, before);
            assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-1"));
            assert_eq!(
                delivery_generation(&f, lead),
                4,
                "keep old reconciliation evidence"
            );
        }
        f.expire_after_reconciliation(io::now_ms() + 10 * 60 * 1000);
        assert_waiting(&f, lead, member, queued, before);
    }
}

#[test]
fn detached_same_generation_process_exit_resumes_original_ids_once() {
    let _providers = crate::messaging::coordinator::tests::support::provider_binaries();
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let (lead, member, queued) = joined_task(&mut f);
    let before = f.typed().len();
    f.worker.transport = detached_transport(&f, 4, true);
    f.worker.tick().unwrap();
    assert_waiting(&f, lead, member, queued, before);
    reopen_detached(&mut f, 4);
    f.worker.tick().unwrap();
    assert_waiting(&f, lead, member, queued, before);
    install_resume_hooks(&f);
    let result = f.worker.resume_room(f.room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "resuming", "{result}");
    f.worker.tick().unwrap();
    assert_eq!(
        f.worker.serving_generation, 4,
        "do not fabricate a generation"
    );
    assert_eq!(
        f.typed().len(),
        before + 1,
        "one continuation after explicit resume"
    );
    let notice = f.typed().last().unwrap().0.clone();
    assert!(notice.contains(&format!("message {}", f.request(lead).prompt.id.0)));
    assert!(!notice.contains("ORIGINAL_"));
    assert_eq!(f.request(member).group, Some(lead));
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    f.worker.tick().unwrap();
    assert_eq!(f.typed().len(), before + 1);
    f.started("turn-1", "ORIGINAL_TASK_NEVER_REPLAY");
    f.stopped("turn-1", "late interrupted final");
    f.consume();
    assert!(!f.request(lead).settled());
    f.started("turn-2", &notice);
    f.consume();
    assert!(f.request(lead).trusted_start_bound);
    f.stopped("turn-2", "resumed native completion");
    f.consume();
    assert_completed_without_reply(&f, lead);
    assert_completed_without_reply(&f, member);
    assert_eq!(f.worker.state.requests().count(), 3);
}

#[test]
fn detached_restart_native_rebind_notifies_once_and_explicit_reply_settles_original() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let (lead, member, queued) = joined_task(&mut f);
    let before = f.typed().len();
    reopen_detached(&mut f, 5);
    f.worker.tick().unwrap();
    assert_waiting(&f, lead, member, queued, before);
    epoch_reopen(&mut f, true, 5);
    f.worker.tick().unwrap();
    let notice = f.typed().last().unwrap().0.clone();
    assert_eq!(f.typed().len(), before + 1);
    epoch_reopen(&mut f, true, 5);
    f.worker.tick().unwrap();
    assert_eq!(f.typed().len(), before + 1);
    f.started("turn-2", &notice);
    f.consume();
    let response = f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "detached-original-reply".into(),
            method: "message.send".into(),
            params: json!({"reply_to":f.request(lead).prompt.id.0.to_string(),
                "as":f.agent.0.to_string(),"text":"answer to original detached task"}),
        },
        None,
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result["stage"], "replied");
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("answer to original detached task")
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    f.stopped("turn-2", "native completion");
    f.consume();
    assert_completed_without_reply(&f, member);
}

#[test]
fn detached_durable_old_end_wins_before_resume() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        let mut f = Fixture::new(provider);
        observe_epoch(&mut f, 4);
        let (lead, member, queued) = joined_task(&mut f);
        let before = f.typed().len();
        reopen_detached(&mut f, 5);
        // This is durable evidence from the interrupted execution, consumed
        // after detach and before it is fenced by a verified successor.
        f.stopped("turn-1", "durable completed task");
        f.worker.tick().unwrap();
        assert_completed_without_reply(&f, lead);
        assert_completed_without_reply(&f, member);
        assert_eq!(f.request(queued).phase, RequestPhase::Queued);
        assert_eq!(f.typed().len(), before);
    }
}

#[test]
fn detached_saved_idle_metadata_is_not_verified_resume() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let (lead, member, queued) = joined_task(&mut f);
    let before = f.typed().len();
    reopen_detached(&mut f, 5);
    f.worker.tick().unwrap();
    assert_waiting(&f, lead, member, queued, before);
    let mut info = DetachedNative {
        methods: f.native.clone(),
        agent: f.agent,
        room: f.room,
        provider: f.provider,
        generation: 5,
        exited: false,
        resumed: true,
    }
    .info(true);
    info.native_ready = false;
    f.worker.transport = Box::new(RestartLifecycleNative {
        methods: f.native.clone(),
        info,
    });
    f.worker.tick().unwrap();
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    assert!(!f.request(lead).settled());
    assert_eq!(f.request(member).group, Some(lead));
    assert_eq!(
        f.typed().len(),
        before,
        "saved metadata cannot authorize a notice"
    );
}

#[test]
fn detached_same_generation_terminal_failures_close_unanswered_group() {
    for cause in ["invalidated", "startup_failed", "deleted"] {
        let mut f = Fixture::new(Provider::Codex);
        observe_epoch(&mut f, 4);
        let (lead, member, queued) = joined_task(&mut f);
        f.worker.transport = detached_transport(&f, 4, true);
        f.worker.tick().unwrap();
        let before = f.typed().len();
        assert_waiting(&f, lead, member, queued, before);
        let mut state = f.worker.state.clone();
        match cause {
            "invalidated" => state.invalidate_agent_session(f.agent).unwrap(),
            "deleted" => state.delete_agent(f.agent, io::now_ms()).unwrap(),
            _ => {
                state
                    .set_agent_startup(
                        f.agent,
                        Some(crate::messaging::model::AgentStartup {
                            command: "codex resume session".into(),
                            started_at_ms: io::now_ms(),
                            reported: true,
                            process_observed: false,
                            native_terminal_id: Some("resumed-terminal".into()),
                            recovery: None,
                            failure: Some(crate::messaging::model::StartupFailure {
                                reason: "saved conversation could not resume".into(),
                                recovery: "replace the agent".into(),
                            }),
                        }),
                    )
                    .unwrap();
                state.abandon_failed_startup_queue(f.agent, io::now_ms());
            }
        }
        f.worker.save(state).unwrap();
        f.worker.tick().unwrap();
        for id in [lead, member] {
            assert_eq!(
                f.request(id).phase,
                RequestPhase::Abandoned,
                "{cause}: {:?}",
                f.request(id)
            );
            assert_eq!(
                f.request(id).failure_reason.as_deref(),
                Some(if cause == "deleted" {
                    "agent_deleted"
                } else {
                    "reply_not_captured"
                })
            );
        }
        assert_eq!(f.typed().len(), before, "no notice after {cause}");
    }
}

#[test]
fn detached_socket_outage_does_not_interrupt_a_same_generation_live_turn() {
    struct Outage;
    impl Transport for Outage {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            Err(TransportError {
                timestamps: Default::default(),
                code: Some("server_unavailable".into()),
                message: "scratch socket unavailable".into(),
                definitely_rejected: false,
            })
        }
    }
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let (lead, member, queued) = joined_task(&mut f);
    let before = f.typed().len();
    f.worker.transport = Box::new(Outage);
    for _ in 0..3 {
        f.worker.tick().unwrap();
    }
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    let mut info = DetachedNative {
        methods: f.native.clone(),
        agent: f.agent,
        room: f.room,
        provider: f.provider,
        generation: 4,
        exited: false,
        resumed: true,
    }
    .info(true);
    info.terminal_id = "terminal".into();
    info.pane_id = "pane".into();
    info.agent_status = schema::AgentStatus::Working;
    f.worker.transport = Box::new(RestartLifecycleNative {
        methods: f.native.clone(),
        info,
    });
    f.worker.tick().unwrap();
    assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-1"));
    assert!(!f.request(lead).settled());
    assert_eq!(f.request(member).group, Some(lead));
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert_eq!(
        f.typed().len(),
        before,
        "socket outage is not provider-exit evidence"
    );
}

fn install_resume_hooks(f: &Fixture) {
    let hooks = f.dir.join(".codex/hooks.json");
    let document = crate::agents::providers::hook_json::merge_hooks(
        json!({}),
        crate::agents::providers::hook_json::HookContract::for_provider(Provider::Codex),
        &std::env::current_exe().unwrap(),
    )
    .unwrap();
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    std::fs::write(hooks, serde_json::to_vec(&document).unwrap()).unwrap();
}

#[test]
fn detached_conclusive_resume_preparation_failure_closes_original_group() {
    let _providers = crate::messaging::coordinator::tests::support::provider_binaries();
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let (lead, member, queued) = joined_task(&mut f);
    f.worker.transport = detached_transport(&f, 4, true);
    f.worker.tick().unwrap();
    let before = f.typed().len();
    assert_waiting(&f, lead, member, queued, before);
    install_resume_hooks(&f);
    // Capture verification conclusively fails before any native resume input.
    std::fs::remove_file(f.dir.join("callbacks/launch/manifest.json")).unwrap();
    let result = f.worker.resume_room(f.room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "blocked", "{result}");
    assert!(
        result["agents"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("capture configuration"),
        "{result}"
    );
    // The conclusive operation result survives reload before settlement.
    reopen_detached(&mut f, 4);
    f.worker.tick().unwrap();
    for id in [lead, member] {
        assert_eq!(f.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            f.request(id).failure_reason.as_deref(),
            Some("reply_not_captured")
        );
    }
    assert_eq!(
        f.typed().len(),
        before,
        "failed resume never types a notice"
    );
}
#[test]
fn detached_wait_preserves_a_newer_delivery_owner_and_requires_native_readiness() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let lead = running(&mut f, "old interrupted task", "turn-1");
    let newer = f.send("current-generation input", false);
    f.deliver();
    let queued = f.send("independent FIFO successor", true);
    let before = f.typed().len();
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["requests"][newer.0.to_string()]["delivery_epoch"]["generation"] = json!(5);
    f.worker
        .save(serde_json::from_value(document).unwrap())
        .unwrap();
    reopen_detached(&mut f, 5);
    f.worker.tick().unwrap();
    assert!(!f.request(lead).settled());
    assert_eq!(f.request(newer).group, None);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(newer),
        "waiting for the old task must retain its current-generation successor owner"
    );
    let mut info = DetachedNative {
        methods: f.native.clone(),
        agent: f.agent,
        room: f.room,
        provider: f.provider,
        generation: 5,
        exited: false,
        resumed: true,
    }
    .info(true);
    info.native_ready = false;
    f.worker.transport = Box::new(RestartLifecycleNative {
        methods: f.native.clone(),
        info,
    });
    f.worker.tick().unwrap();
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().status,
        RuntimeStatus::Unavailable
    );
    assert_eq!(
        f.typed().len(),
        before,
        "an older waiting task still needs native readiness"
    );
    f.worker.transport = epoch_transport(&f, true, 5);
    f.worker.tick().unwrap();
    assert_eq!(f.request(lead).phase, RequestPhase::Queued);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(newer)
    );
    assert_eq!(
        f.typed().len(),
        before,
        "the notice must wait for the newer delivery"
    );
    f.started("turn-5", "current-generation input");
    f.consume();
    f.stopped("turn-5", "current task completed");
    f.consume();
    assert_completed_without_reply(&f, newer);
    f.status(RuntimeStatus::Idle);
    f.deliver();
    assert_eq!(f.typed().len(), before + 1);
    let notice = f.typed().last().unwrap().0.clone();
    assert!(notice.contains("Bus restarted"));
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    f.started("turn-6", &notice);
    f.consume();
    f.stopped("turn-6", "old task completed after verified recovery");
    f.consume();
    assert_completed_without_reply(&f, lead);
}

#[test]
fn detached_invalidated_ownership_before_process_exit_is_terminal() {
    let mut f = Fixture::new(Provider::Codex);
    observe_epoch(&mut f, 4);
    let (lead, member, _) = joined_task(&mut f);
    let before = f.typed().len();
    let mut state = f.worker.state.clone();
    state.invalidate_agent_session(f.agent).unwrap();
    f.worker.save(state).unwrap();
    f.worker.transport = detached_transport(&f, 4, true);
    f.worker.tick().unwrap();
    for id in [lead, member] {
        assert_eq!(
            f.request(id).phase,
            RequestPhase::Abandoned,
            "positive process death cannot retain invalidated task ownership"
        );
    }
    assert_eq!(f.typed().len(), before);
}

#[test]
fn detached_historical_resume_failure_cannot_abandon_a_later_started_task() {
    use crate::messaging::model::{LaunchKind, LaunchOutcome};
    let _providers = crate::messaging::coordinator::tests::support::provider_binaries();
    let mut f = Fixture::new(Provider::Codex);
    let mut state = f.worker.state.clone();
    let identity = state.agent(f.agent).unwrap().runtime_identity.clone();
    let old = state.begin_launch_attempt("builder", f.provider, f.room, LaunchKind::Resume, 1);
    state.attach_launch_attempt(old, f.agent).unwrap();
    assert!(state.bind_launch_identity(old, &identity));
    assert!(state.settle_launch_attempt(
        old,
        &identity,
        LaunchOutcome::Failed,
        Some("startup_timeout"),
        2
    ));
    f.worker.save(state).unwrap();
    // A late successful readiness observation must keep that failed audit
    // record immutable; this later task then really starts and owns the turn.
    observe_epoch(&mut f, 4);
    let (lead, member, queued) = joined_task(&mut f);
    let before = f.typed().len();
    f.worker.transport = detached_transport(&f, 4, true);
    f.worker.tick().unwrap();
    assert_waiting(&f, lead, member, queued, before);
    assert_eq!(
        f.worker.state.launch_attempt(old).unwrap().outcome,
        LaunchOutcome::Failed
    );
    reopen_detached(&mut f, 4);
    f.worker.tick().unwrap();
    assert_waiting(&f, lead, member, queued, before);
    install_resume_hooks(&f);
    let result = f.worker.resume_room(f.room).unwrap();
    assert_eq!(result["agents"][0]["outcome"], "resuming", "{result}");
    f.worker.tick().unwrap();
    assert_eq!(f.typed().len(), before + 1);
    let notice = f.typed().last().unwrap().0.clone();
    assert!(notice.contains("Bus restarted"));
    f.started("turn-2", &notice);
    f.consume();
    f.stopped("turn-2", "finished the later task");
    f.consume();
    assert_completed_without_reply(&f, lead);
    assert_completed_without_reply(&f, member);
    assert_eq!(
        f.worker.state.launch_attempt(old).unwrap().outcome,
        LaunchOutcome::Failed
    );
}
