//! Interrupt settlement must wait for retirement of a restored Claude input.
//! Native composer byte-level cases live in claude_rich_draft_test.rs.
use super::*;

fn interrupt(f: &mut Fixture) -> crate::messaging::control::Response {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!(
                "retirement-interrupt-{}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
            method: "agent.interrupt".into(),
            params: json!({"agent": "builder"}),
        },
        None,
    )
}

/// The native stop may have happened, but its composer retirement has no
/// receipt. Keep ordinary prompt methods real to the coordinator fixture.
struct UnconfirmedRetirement(Arc<Mutex<Vec<Method>>>);

impl Transport for UnconfirmedRetirement {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(&method, Method::AgentSendKeys(_)) {
            self.0.lock().unwrap().push(method);
            return Err(TransportError {
                code: Some("agent_interrupt_unconfirmed".into()),
                message: "Stop may have run; restored Bus input was not retired".into(),
                definitely_rejected: false,
                timestamps: Default::default(),
            });
        }
        Native(self.0.clone()).request(method)
    }
}

#[test]
fn claude_retirement_receipt_fences_the_exact_lead_and_joined_inputs() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let joined = f.send("joined correction", false);
        f.deliver();
        assert_eq!(f.request(joined).group, Some(lead));
        let submitted: Vec<String> = f.typed().into_iter().map(|(text, _)| text).collect();

        let response = interrupt(&mut f);
        assert!(response.ok, "{provider:?}: {response:?}");
        let calls = f.native.lock().unwrap();
        let params = calls
            .iter()
            .find_map(|method| match method {
                Method::AgentSendKeys(params) => Some(params),
                _ => None,
            })
            .expect("the interruption sent a native stop");
        assert_eq!(params.target, "pane");
        assert_eq!(params.keys, ["esc"]);
        let identity = params
            .native_interrupt
            .as_ref()
            .expect("Claude stop requires guarded retirement");
        assert_eq!(identity.expected_terminal_id, "terminal");
        assert_eq!(identity.expected_session_id, "session");
        assert!(
            identity.claude_turn.is_none(),
            "joined receipts use exact clear only"
        );
        assert_eq!(
            identity.expected_payloads, submitted,
            "{provider:?}: fence the actual typed inputs, including joined members"
        );
    }
}

#[test]
fn claude_confirmed_retirement_closes_joined_work_and_keeps_two_followers_fifo() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let joined = f.send("joined correction", false);
        f.deliver();
        assert_eq!(f.request(joined).group, Some(lead));
        let first = f.send("first queued follower", true);
        let second = f.send("second queued follower", true);
        let response = interrupt(&mut f);
        assert!(response.ok, "{provider:?}: {response:?}");
        assert!(!f.request(lead).interrupt_retirement_pending);
        f.status(RuntimeStatus::Idle);
        for id in [lead, joined] {
            assert_eq!(
                f.request(id).phase,
                RequestPhase::Abandoned,
                "{provider:?}: {id:?}"
            );
            assert_eq!(f.request(id).failure_reason.as_deref(), Some("interrupted"));
        }
        assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
        f.deliver();
        assert_eq!(
            f.typed_content().last(),
            Some(&("first queued follower".into(), false))
        );
        assert_eq!(f.request(second).phase, RequestPhase::Queued);
        f.started("turn-2", "first queued follower");
        f.consume();
        f.stopped("turn-2", "first done");
        f.consume();
        f.status(RuntimeStatus::Idle);
        f.deliver();
        assert_eq!(
            f.typed_content().last(),
            Some(&("second queued follower".into(), false))
        );
        f.started("turn-3", "second queued follower");
        f.consume();
        f.stopped("turn-3", "second done");
        f.consume();
        assert_eq!(f.reply(first).as_deref(), Some("first done"));
        assert_eq!(f.reply(second).as_deref(), Some("second done"));
        assert!(f.worker.state.queued_requests(f.agent).is_empty());
        assert_eq!(
            f.typed_content()
                .iter()
                .filter(|(text, _)| text == "first queued follower")
                .count(),
            1
        );
        assert_eq!(
            f.typed_content()
                .iter()
                .filter(|(text, _)| text == "second queued follower")
                .count(),
            1
        );
    }
}

#[test]
fn claude_unconfirmed_retirement_survives_idle_late_stop_and_settlement_without_advancing() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let first = f.send("first queued follower", true);
        let second = f.send("second queued follower", true);
        f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
        // An earlier typed admission must not let a later failed retirement
        // inherit its visual-Idle permission to abandon the current input.
        let mut state = f.worker.state.clone();
        state.begin_interrupt(f.agent).unwrap();
        f.worker.save(state).unwrap();
        let typed_before = f.typed().len();
        let response = interrupt(&mut f);
        assert!(response.ok, "{provider:?}: {response:?}");
        assert_eq!(response.result["agents"][0]["outcome"], "failed");
        assert!(f.request(lead).interrupt_retirement_pending);
        assert!(!f.worker.state.agent(f.agent).unwrap().interrupt_pending);
        for _ in 0..3 {
            f.status(RuntimeStatus::Idle);
            f.worker.settle_requests().unwrap();
            f.deliver();
        }
        f.started("turn-1", "original input");
        f.consume();
        f.stopped("turn-1", "late stopped-turn answer");
        f.consume();
        f.status(RuntimeStatus::Idle);
        f.worker.settle_requests().unwrap();
        f.deliver();
        assert!(
            !f.request(lead).settled(),
            "{provider:?}: an unresolved native copy must remain tracked"
        );
        assert!(
            !f.worker.state.turn_ended(f.request(lead)),
            "{provider:?}: late Stop cannot certify composer retirement"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
        assert_eq!(
            f.typed().len(),
            typed_before,
            "{provider:?}: no follower can overtake unresolved input"
        );
        let stage = f.worker.control_message(f.request(lead).prompt.id).unwrap();
        assert_eq!(stage["complete"], false, "{provider:?}: {stage}");
        assert!(
            stage["requests"][0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("retirement")),
            "{provider:?}: unresolved retirement must be actionable: {stage}"
        );
        // Ordinary turn/submission stall deadlines cannot turn the uncertain
        // native copy into an abandoned request after enough Idle polls.
        f.now += 10 * 60 * 1_000;
        for _ in 0..3 {
            f.status(RuntimeStatus::Idle);
            f.worker.settle_requests().unwrap();
        }
        assert!(
            !f.request(lead).settled(),
            "{provider:?}: stalled retirement closed without proof"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        let persisted = JsonStore::new(f.dir.join("state.json"))
            .load()
            .unwrap()
            .unwrap();
        assert!(
            persisted
                .request(lead)
                .unwrap()
                .interrupt_retirement_pending
        );
        assert_eq!(
            persisted.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
    }
}

#[test]
fn claude_late_transient_failure_cannot_requeue_an_unretired_native_copy() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let first = f.send("first queued follower", true);
        let second = f.send("second queued follower", true);
        f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
        assert!(interrupt(&mut f).ok);
        let typed_before = f.typed().len();
        f.record(json!({
            "hook_event_name": "StopFailure", "session_id": "session",
            "prompt_id": "turn-1", "error": "server_error",
            "last_assistant_message": "API Error: 529 Overloaded.",
        }));
        f.consume();
        f.status(RuntimeStatus::Idle);
        // Spend every provider retry deadline without allowing an ordinary
        // transient retry to detach ownership and repaste the same old input.
        let mut state = f.worker.state.clone();
        state.retry_provider_requests(f.now + 10 * 60 * 1_000);
        f.worker.save(state).unwrap();
        f.worker.settle_requests().unwrap();
        f.deliver();
        assert!(f.request(lead).interrupt_retirement_pending);
        assert!(!f.request(lead).settled());
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        assert!(
            f.worker
                .state
                .agent(f.agent)
                .unwrap()
                .provider_retry
                .is_none(),
            "{provider:?}: unretired input entered automatic retry"
        );
        assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
        assert_eq!(
            f.typed().len(),
            typed_before,
            "{provider:?}: old input or follower was repasted before retirement"
        );
        let status = f.worker.control_message(f.request(lead).prompt.id).unwrap();
        assert_eq!(status["complete"], false);
        assert!(
            status["requests"][0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("retirement")),
            "{provider:?}: {status}"
        );
    }
}

#[test]
fn claude_late_started_cannot_revive_an_abandoned_retired_request_or_bind_its_follower() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let follower = f.send("fresh follower", true);
        assert!(interrupt(&mut f).ok);
        f.status(RuntimeStatus::Idle);
        assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
        f.deliver();
        f.started("turn-1", "original input");
        f.consume();
        assert_eq!(
            f.request(lead).phase,
            RequestPhase::Abandoned,
            "{provider:?}: old request revived"
        );
        assert_eq!(
            f.request(lead).failure_reason.as_deref(),
            Some("interrupted")
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(follower)
        );
        assert!(
            !f.request(follower).trusted_start_bound,
            "{provider:?}: old prompt claimed a newer request"
        );
        f.started("turn-2", "fresh follower");
        f.consume();
        assert!(f.request(follower).trusted_start_bound);
        assert_eq!(
            f.request(follower).provider_turn_id.as_deref(),
            Some("turn-2")
        );
        assert_eq!(
            f.typed_content()
                .iter()
                .filter(|(text, _)| text == "original input")
                .count(),
            1
        );
    }
}

#[test]
fn manual_recovery_cannot_release_an_unretired_claude_native_copy() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let follower = f.send("queued follower", true);
        f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
        assert!(interrupt(&mut f).ok);
        f.status(RuntimeStatus::Idle);
        let before = f.typed().len();
        let recovered = f.worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: format!("recover-unretired-{}", f.agent.0),
                method: "request.recover".into(),
                params: json!({"request": lead.0.to_string(), "confirm": true}),
            },
            None,
        );
        assert!(
            !recovered.ok,
            "manual closure needs an explicit safe native resolution"
        );
        assert!(!f.request(lead).settled());
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(lead)
        );
        f.deliver();
        assert_eq!(f.typed().len(), before);
        assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    }
}

#[test]
fn a_late_joined_started_cannot_orphan_retirement_after_a_late_stop() {
    for provider in [Provider::ClaudeCode, Provider::ClaudeDs] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original input", "turn-1");
        let joined = f.send("joined correction", false);
        f.deliver();
        assert_eq!(f.request(joined).group, Some(lead));
        assert!(!f.request(joined).trusted_start_bound);
        let follower = f.send("queued follower", true);
        f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
        assert!(interrupt(&mut f).ok);
        let before = f.typed().len();
        f.stopped("turn-1", "late original answer");
        f.consume();
        assert!(f.request(lead).pending_final.is_some());
        assert!(f.request(lead).interrupt_retirement_pending);
        f.started("turn-2", "joined correction");
        f.consume();
        assert!(
            f.worker
                .state
                .agent(f.agent)
                .unwrap()
                .current_request
                .and_then(|id| f.worker.state.request(id))
                .is_some_and(|request| request.interrupt_retirement_pending),
            "moving turn ownership must preserve the unresolved native obligation"
        );
        assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-1"));
        assert_eq!(
            f.request(lead).pending_final.as_ref().unwrap().text,
            "late original answer"
        );
        assert!(!f.request(joined).trusted_start_bound);
        f.stopped("turn-2", "joined answer");
        f.consume();
        f.status(RuntimeStatus::Idle);
        f.deliver();
        assert_eq!(f.typed().len(), before);
        assert_eq!(f.request(follower).phase, RequestPhase::Queued);
        assert!(!f.request(lead).settled());
    }
}

#[test]
fn pending_retirement_keeps_its_resolution_guidance() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    let status = f.worker.control_message(f.request(lead).prompt.id).unwrap();
    let detail = status["requests"][0]["detail"].as_str().unwrap();
    assert!(detail.contains("do not resend"), "{detail}");
}

#[test]
fn a_completed_anchor_cannot_release_unretired_joined_input() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let joined = f.send("joined correction", false);
    f.deliver();
    f.stopped("turn-1", "original answer");
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert_eq!(f.request(joined).group, Some(lead));
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    assert!(f.request(lead).interrupt_retirement_pending);
    f.status(RuntimeStatus::Idle);
    f.worker.settle_requests().unwrap();
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(lead)
    );
    f.deliver();
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
}

#[test]
fn claude_interrupt_mode_composer_journal_persists_each_follower_once() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    let (events, _receiver) = std::sync::mpsc::channel();
    for text in ["first follower", "second follower"] {
        f.worker
            .command(
                crate::messaging::coordinator::BusCommand::SubmitQueued(
                    f.room,
                    Draft {
                        text: text.into(),
                        files: Vec::new(),
                        recipient_ids: [f.agent].into(),
                    },
                ),
                &events,
            )
            .unwrap();
    }
    let saved = JsonStore::new(f.dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.interrupt_retirement_owner(f.agent), Some(lead));
    let messages: Vec<_> = saved
        .queued_requests(f.agent)
        .iter()
        .map(|id| saved.request(*id).unwrap().prompt.text.as_str())
        .collect();
    assert_eq!(messages, ["first follower", "second follower"]);
}

#[test]
fn idle_unresolved_request_can_retry_guarded_retirement() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let mut state = f.worker.state.clone();
    state.set_interrupt_retirement(f.agent, true).unwrap();
    f.worker.save(state).unwrap();
    f.status(RuntimeStatus::Idle);
    let outcome = interrupt(&mut f);
    assert_eq!(outcome.result["agents"][0]["outcome"], "interrupting");
    assert!(!f.request(lead).interrupt_retirement_pending);
}

#[test]
fn round2_deferred_joined_answers_survive_guarded_retry() {
    deferred_joined_answers_survive_retirement(false);
}

#[test]
fn round2_deferred_joined_answers_survive_verified_original_exit() {
    deferred_joined_answers_survive_retirement(true);
}

fn deferred_joined_answers_survive_retirement(exited: bool) {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let member = f.send("joined correction", false);
    f.deliver();
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    f.stopped("turn-1", "lead answer");
    f.consume();
    f.started("turn-2", "joined correction");
    f.stopped("turn-2", "member answer");
    f.consume();
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(lead)
    );
    if exited {
        f.worker.transport = Box::new(ObservedOriginalExit {
            calls: f.native.clone(),
            terminal: "terminal",
            exited: true,
        });
        let response = recover_held_request(&mut f, lead);
        assert!(response.ok, "{response:?}");
        let mut state = f.worker.state.clone();
        let mut identity = state.agent(f.agent).unwrap().runtime_identity.clone();
        identity.terminal_id = Some("replacement".into());
        identity.pane_id = Some("pane".into());
        state.set_agent_runtime_identity(f.agent, identity).unwrap();
        f.worker.save(state).unwrap();
    } else {
        f.worker.transport = Box::new(Native(f.native.clone()));
        assert_eq!(
            interrupt(&mut f).result["agents"][0]["outcome"],
            "interrupting"
        );
    }
    // The production tick polls Idle before consuming the next spool batch.
    retirement_idle_tick(&mut f);
    assert_eq!(f.reply(lead).as_deref(), Some("lead answer"));
    assert_eq!(
        f.reply(member).as_deref(),
        Some("member answer"),
        "original exited={exited}"
    );
    assert_eq!(
        f.request(member).provider_turn_id.as_deref(),
        Some("turn-2")
    );
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(follower)
    );
    assert_eq!(
        f.typed_content().last(),
        Some(&("queued follower".into(), false))
    );
}

struct IdleRetirementPoll {
    calls: Arc<Mutex<Vec<Method>>>,
    info: schema::AgentInfo,
}

impl Transport for IdleRetirementPoll {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::AgentListWithDetectionContext(_)) {
            self.calls.lock().unwrap().push(method);
            return Ok(ResponseResult::AgentList {
                agents: vec![self.info.clone()],
            });
        }
        Native(self.calls.clone()).request(method)
    }
}

fn retirement_idle_tick(f: &mut Fixture) {
    let agent = f.worker.state.agent(f.agent).unwrap();
    let identity = &agent.runtime_identity;
    let info = serde_json::from_value(json!({
        "terminal_id":identity.terminal_id, "pane_id":identity.pane_id, "agent":"claude",
        "name": crate::messaging::identity::managed_name(agent.room_id, agent.id),
        "native_ready":true, "interactive_ready":true, "agent_status":"idle",
        "agent_session":{"source":"herdr:claude","agent":"claude","kind":"id","value":"session"},
        "workspace_id":"w", "tab_id":"t", "focused":false, "revision":1
    }))
    .unwrap();
    f.worker.transport = Box::new(IdleRetirementPoll {
        calls: f.native.clone(),
        info,
    });
    f.worker.tick().unwrap();
    assert!(f
        .native
        .lock()
        .unwrap()
        .iter()
        .any(|method| matches!(method, Method::AgentListWithDetectionContext(_))));
}

#[test]
fn round2_joined_answer_published_after_native_retry_survives_the_idle_tick() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let member = f.send("joined correction", false);
    f.deliver();
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    f.stopped("turn-1", "lead answer");
    f.consume();
    f.worker.transport = Box::new(Native(f.native.clone()));
    assert_eq!(
        interrupt(&mut f).result["agents"][0]["outcome"],
        "interrupting"
    );
    f.started("turn-2", "joined correction");
    f.stopped("turn-2", "member answer");
    retirement_idle_tick(&mut f);
    assert_eq!(f.reply(lead).as_deref(), Some("lead answer"));
    assert_eq!(f.reply(member).as_deref(), Some("member answer"));
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(follower)
    );
}

#[test]
fn legacy_delivery_without_original_identity_cannot_use_exit_evidence() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let follower = f.send("queued follower", true);
    let mut old_snapshot = serde_json::to_value(&f.worker.state).unwrap();
    let row = old_snapshot["requests"][lead.0.to_string()]
        .as_object_mut()
        .unwrap();
    row.remove("native_delivery_identity").unwrap();
    assert!(row
        .get("delivery_epoch")
        .is_some_and(|epoch| !epoch.is_null()));
    f.worker
        .save(serde_json::from_value(old_snapshot).unwrap())
        .unwrap();

    assert_eq!(interrupt(&mut f).result["agents"][0]["outcome"], "failed");
    f.worker.transport = Box::new(ObservedOriginalExit {
        calls: f.native.clone(),
        terminal: "terminal",
        exited: true,
    });
    assert!(!recover_held_request(&mut f, lead).ok);
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    assert!(!f.native.lock().unwrap().iter().any(|method| matches!(
        method,
        Method::AgentSendKeys(_) | Method::PaneCloseIfIdentity(_)
    )));
}

struct ObservedOriginalExit {
    calls: Arc<Mutex<Vec<Method>>>,
    terminal: &'static str,
    exited: bool,
}

impl Transport for ObservedOriginalExit {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::TerminalRequireAbsent(_)) {
            self.calls.lock().unwrap().push(method);
            return Err(TransportError {
                code: Some("terminal_present".into()),
                message: "original terminal remains attached or detached".into(),
                definitely_rejected: true,
                timestamps: Default::default(),
            });
        }
        if matches!(method, Method::AgentListWithDetectionContext(_)) {
            self.calls.lock().unwrap().push(method);
            return Ok(ResponseResult::AgentList { agents: Vec::new() });
        }
        if let Method::PaneGet(params) = &method {
            let pane = serde_json::from_value(json!({
                "terminal_id": self.terminal, "pane_id": params.pane_id,
                "workspace_id":"w", "tab_id":"t", "focused":false,
                "agent_status":"unknown", "revision":1,
                "agent_process_exited": self.exited,
                "agent_exited_session_id": self.exited.then_some("session"),
            }))
            .unwrap();
            self.calls.lock().unwrap().push(method);
            return Ok(ResponseResult::PaneInfo { pane });
        }
        Native(self.calls.clone()).request(method)
    }
}

fn recover_held_request(f: &mut Fixture, lead: RequestId) -> crate::messaging::control::Response {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("recover-original-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            method: "request.recover".into(),
            params: json!({"request": lead.0.to_string(), "confirm": true}),
        },
        None,
    )
}

#[test]
fn claude_ds_missing_receipt_resolves_unknown_after_verified_original_exit_without_touching_drafts()
{
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let first = f.send("first follower", true);
    let second = f.send("second follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert_eq!(interrupt(&mut f).result["agents"][0]["outcome"], "failed");
    f.status(RuntimeStatus::Idle);
    for (terminal, exited) in [("terminal", false), ("replacement", true)] {
        f.worker.transport = Box::new(ObservedOriginalExit {
            calls: f.native.clone(),
            terminal,
            exited,
        });
        let _ = recover_held_request(&mut f, lead);
        f.deliver();
        assert!(f.request(lead).interrupt_retirement_pending);
        assert_eq!(f.typed().len(), 1);
    }
    f.worker.transport = Box::new(ObservedOriginalExit {
        calls: f.native.clone(),
        terminal: "terminal",
        exited: true,
    });
    let response = recover_held_request(&mut f, lead);
    assert!(response.ok, "{response:?}");
    assert!(f.request(lead).uncertain_outcome);
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("interrupt_outcome_unknown")
    );
    assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
    assert!(recover_held_request(&mut f, lead).ok);
    assert!(!f.native.lock().unwrap().iter().any(|method| matches!(
        method,
        Method::PaneClose(_) | Method::PaneCloseIfIdentity(_)
    )));
    assert_eq!(
        f.native
            .lock()
            .unwrap()
            .iter()
            .filter(|method| matches!(method, Method::AgentSendKeys(_)))
            .count(),
        1
    );

    let mut state = f.worker.state.clone();
    let mut identity = state.agent(f.agent).unwrap().runtime_identity.clone();
    identity.pane_id = Some("pane".into());
    identity.terminal_id = Some("new-terminal".into());
    state.set_agent_runtime_identity(f.agent, identity).unwrap();
    f.worker.save(state).unwrap();
    f.status(RuntimeStatus::Idle);
    f.deliver();
    assert_eq!(
        f.typed_content().last(),
        Some(&("first follower".into(), false))
    );
    f.started("turn-2", "first follower");
    f.consume();
    f.stopped("turn-2", "done first");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.deliver();
    assert_eq!(
        f.typed_content().last(),
        Some(&("second follower".into(), false))
    );
    assert_eq!(f.typed().len(), 3);
}

#[test]
fn request_recovery_rejects_the_removed_destructive_wire_parameter() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let result = f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "obsolete-terminal-recovery-flag".into(),
            method: "request.recover".into(),
            params: json!({"request": lead.0.to_string(), "confirm": true, "retire_terminal": true}),
        },
        None,
    );
    assert!(!result.ok);
    assert_eq!(result.error.unwrap().code, "invalid_params");
    assert!(!f.native.lock().unwrap().iter().any(|method| matches!(
        method,
        Method::PaneClose(_) | Method::PaneCloseIfIdentity(_)
    )));
}

#[test]
fn claude_ds_original_exit_resolves_the_retirement_hold_on_poll() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert_eq!(interrupt(&mut f).result["agents"][0]["outcome"], "failed");
    f.worker.transport = Box::new(ObservedOriginalExit {
        calls: f.native.clone(),
        terminal: "terminal",
        exited: true,
    });
    f.worker.tick().unwrap();
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("interrupt_outcome_unknown")
    );
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    assert!(!f.native.lock().unwrap().iter().any(|method| matches!(
        method,
        Method::PaneClose(_) | Method::PaneCloseIfIdentity(_)
    )));
}

struct PressAliases(Arc<Mutex<Vec<Method>>>);

impl Transport for PressAliases {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let target = match &method {
            Method::AgentObserve(params) => &params.target,
            Method::AgentPress(params) => &params.target,
            _ => return Native(self.0.clone()).request(method),
        };
        let (terminal, pane) = match target.as_str() {
            "terminal" | "moved-pane" => ("terminal", "moved-pane"),
            "replacement-pane" | "replacement-terminal" | "claude" | "claude-ds" => {
                ("replacement-terminal", "replacement-pane")
            }
            "unrelated" => ("other-terminal", "other-pane"),
            target if target.starts_with("bus-r") => ("replacement-terminal", "replacement-pane"),
            _ => panic!("unexpected native target {target}"),
        };
        let ResponseResult::TerminalObserved { mut observation } = open_native_question(pane)
        else {
            unreachable!()
        };
        observation.terminal_id = terminal.into();
        let result = match &method {
            Method::AgentPress(params) => {
                if target == "unrelated" {
                    assert_eq!(params.expected_terminal_id.as_deref(), Some(terminal));
                }
                ResponseResult::TerminalPressed {
                    press: schema::TerminalPressResult {
                        observation,
                        keys: params.keys.clone(),
                        text_written: false,
                        selection_pending: false,
                    },
                }
            }
            _ => ResponseResult::TerminalObserved { observation },
        };
        self.0.lock().unwrap().push(method);
        Ok(result)
    }
}

#[test]
fn agent_press_refuses_held_terminal_by_id_or_moved_pane() {
    for missing_origin in [false, true] {
        let mut f = Fixture::new(Provider::ClaudeDs);
        let lead = running(&mut f, "held work", "turn-1");
        let mut state = f.worker.state.clone();
        state.set_interrupt_retirement(f.agent, true).unwrap();
        let mut replacement = state.agent(f.agent).unwrap().runtime_identity.clone();
        replacement.terminal_id = Some("replacement-terminal".into());
        replacement.pane_id = Some("replacement-pane".into());
        state
            .set_agent_runtime_identity(f.agent, replacement)
            .unwrap();
        if missing_origin {
            let mut saved = serde_json::to_value(&state).unwrap();
            let row = &mut saved["requests"][lead.0.to_string()];
            row.as_object_mut()
                .unwrap()
                .remove("interrupt_retirement_identity");
            row.as_object_mut()
                .unwrap()
                .remove("native_delivery_identity");
            state = serde_json::from_value(saved).unwrap();
        }
        assert_eq!(
            state.interrupt_retirement_origin(lead).is_none(),
            missing_origin
        );
        f.worker.save(state).unwrap();
        f.worker.transport = Box::new(PressAliases(f.native.clone()));

        let observed = f.worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "observe-held".into(),
                method: "agent.observe".into(),
                params: json!({"agent":"moved-pane", "pane":true}),
            },
            None,
        );
        assert!(observed.ok);
        assert_eq!(observed.result["terminal_id"], "terminal");
        let managed = crate::messaging::identity::managed_name(f.room, f.agent);
        let mut press = |target: &str, pane: bool, terminal: Option<&str>| {
            let mut params = json!({"agent":target, "pane":pane, "keys":["enter"]});
            if let Some(terminal) = terminal {
                params["terminal_id"] = json!(terminal);
            }
            f.worker.control_response_with_events(
                &crate::messaging::control::Request {
                    id: format!("press-{target}"),
                    method: "agent.press".into(),
                    params,
                },
                None,
            )
        };
        for (target, pane, terminal) in [
            ("terminal", true, Some("terminal")),
            ("moved-pane", true, None),
            (managed.as_str(), true, None),
            ("claude", true, None),
            ("claude-ds", true, None),
            ("builder", false, None),
            ("replacement-terminal", true, None),
        ] {
            let response = press(target, pane, terminal);
            assert!(!response.ok, "wrote through {target}");
            assert!(
                response.error.unwrap().message.contains("retirement"),
                "{target}"
            );
        }
        assert!(press("unrelated", true, None).ok);
        assert_eq!(
            f.native
                .lock()
                .unwrap()
                .iter()
                .filter(|call| matches!(call, Method::AgentPress(_)))
                .count(),
            1
        );
    }
}

struct AbsentOriginal(Arc<Mutex<Vec<Method>>>, Option<schema::AgentInfo>);

impl Transport for AbsentOriginal {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::AgentListWithDetectionContext(_)) {
            self.0.lock().unwrap().push(method);
            return Ok(ResponseResult::AgentList {
                agents: self.1.clone().into_iter().collect(),
            });
        }
        if let Method::TerminalRequireAbsent(params) = &method {
            let terminal_id = params.terminal_id.clone();
            self.0.lock().unwrap().push(method);
            return Ok(ResponseResult::TerminalAbsent { terminal_id });
        }
        Native(self.0.clone()).request(method)
    }
}

#[test]
fn claude_ds_verified_absence_keeps_an_unbound_leads_spooled_answer() {
    for case in ["matched", "different session", "ambiguous start"] {
        let mut f = Fixture::new(Provider::ClaudeDs);
        let lead = f.send("original unbound input", true);
        let mut state = f.worker.state.clone();
        let mut identity = state.agent(f.agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        state.set_agent_runtime_identity(f.agent, identity).unwrap();
        state.begin_submission(lead, "launch", 0).unwrap();
        state
            .stamp_native_delivery_epoch(
                lead,
                crate::messaging::model::DeliveryEpoch {
                    generation: 1,
                    terminal_id: "terminal".into(),
                    pane_id: "pane".into(),
                },
                Some(0),
            )
            .unwrap();
        state
            .record_submission(
                lead,
                SubmissionOutcome::Confirmed {
                    provider_session_id: None,
                    provider_turn_id: None,
                },
            )
            .unwrap();
        state.set_interrupt_retirement(f.agent, true).unwrap();
        f.worker.save(state).unwrap();
        let first = f.send("first follower", true);
        let second = f.send("second follower", true);
        f.started("turn-1", "original unbound input");
        if case == "different session" {
            f.record(json!({
                "hook_event_name":"Stop", "session_id":"foreign-session", "prompt_id":"turn-1",
                "last_assistant_message":"foreign answer", "background_tasks":[], "session_crons":[]
            }));
        } else {
            if case == "ambiguous start" {
                f.started("turn-2", "original unbound input");
            }
            f.stopped("turn-1", "captured original answer");
        }
        f.worker.transport = Box::new(AbsentOriginal(f.native.clone(), None));
        assert!(recover_held_request(&mut f, lead).ok);
        let expected = (case == "matched").then_some("captured original answer");
        assert_eq!(f.reply(lead).as_deref(), expected, "{case}");
        if expected.is_none() {
            assert_eq!(
                f.request(lead).failure_reason.as_deref(),
                Some("interrupt_outcome_unknown")
            );
            assert!(!f.request(lead).trusted_start_bound);
        }
        assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
    }
}

#[test]
fn verified_original_absence_does_not_skip_the_replacement_poll() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    running(&mut f, "original work", "turn-1");
    let mut state = f.worker.state.clone();
    state.set_interrupt_retirement(f.agent, true).unwrap();
    let mut replacement = state.agent(f.agent).unwrap().runtime_identity.clone();
    replacement.terminal_id = Some("replacement-terminal".into());
    replacement.pane_id = Some("replacement-pane".into());
    state
        .set_agent_runtime_identity(f.agent, replacement)
        .unwrap();
    f.worker.save(state).unwrap();
    let info = serde_json::from_value(json!({
        "terminal_id":"replacement-terminal", "pane_id":"replacement-pane", "agent":"claude",
        "name":crate::messaging::identity::managed_name(f.room, f.agent),
        "native_ready":true, "interactive_ready":true, "agent_status":"idle",
        "agent_session":{"source":"herdr:claude","agent":"claude","kind":"id","value":"session"},
        "workspace_id":"w", "tab_id":"t", "focused":false, "revision":1
    }))
    .unwrap();
    f.worker.transport = Box::new(AbsentOriginal(f.native.clone(), Some(info)));
    f.worker.tick().unwrap();
    let agent = f.worker.state.agent(f.agent).unwrap();
    assert_eq!(
        agent.runtime_identity.terminal_id.as_deref(),
        Some("replacement-terminal")
    );
    assert_eq!(agent.status, RuntimeStatus::Idle);
}

#[cfg(unix)]
#[test]
fn claude_single_input_native_retirement_releases_exact_follower_on_verified_idle() {
    use crate::agents::providers::claude_code::retirement::TranscriptAnchor;

    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = start_claude_retirement_anchor_fixture(&mut f);
    let follower = f.send("queued follower", true);
    assert!(interrupt(&mut f).ok);
    let calls = f.native.lock().unwrap();
    let identity = calls
        .iter()
        .find_map(|method| match method {
            Method::AgentSendKeys(params) => params.native_interrupt.as_ref(),
            _ => None,
        })
        .unwrap();
    assert_eq!(identity.retirement_request_id, Some(lead.0));
    let turn = identity.claude_turn.as_ref().unwrap();
    assert_eq!(turn.prompt_id, "turn-1");
    assert_eq!(turn.launch_id, "launch");
    let load = |launch, terminal, request| {
        TranscriptAnchor::load(
            &f.dir,
            launch,
            "session",
            &turn.prompt_id,
            terminal,
            request,
        )
    };
    assert!(load(&turn.launch_id, "terminal", lead.0).is_some());
    assert!(load("old-launch", "terminal", lead.0).is_none());
    assert!(load(&turn.launch_id, "replacement", lead.0).is_none());
    assert!(load(&turn.launch_id, "terminal", follower.0).is_none());
    drop(calls);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    retirement_idle_tick(&mut f);
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
    f.deliver();
    f.deliver();
    assert_eq!(
        f.typed_content()
            .iter()
            .filter(|(text, _)| text == "queued follower")
            .count(),
        1
    );
}

#[cfg(unix)]
fn start_claude_retirement_anchor_fixture(f: &mut Fixture) -> RequestId {
    let lead = f.send("original input", false);
    f.deliver();
    let path = f.dir.join("session.jsonl");
    std::fs::write(&path, "").unwrap();
    f.record(json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "session",
        "prompt_id": "turn-1",
        "prompt": "original input",
        "transcript_path": path,
    }));
    f.consume();
    f.status(RuntimeStatus::Working);
    lead
}

#[cfg(unix)]
#[test]
fn claude_single_input_retirement_keeps_hold_until_proof_and_preserves_earlier_final() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = start_claude_retirement_anchor_fixture(&mut f);
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert_eq!(interrupt(&mut f).result["agents"][0]["outcome"], "failed");
    retirement_idle_tick(&mut f);
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    assert_eq!(f.typed_content().len(), 1);

    f.stopped("turn-1", "attributable answer");
    f.consume();
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    f.worker.transport = Box::new(Native(f.native.clone()));
    assert_eq!(
        interrupt(&mut f).result["agents"][0]["outcome"],
        "interrupting"
    );
    retirement_idle_tick(&mut f);
    assert_eq!(f.reply(lead).as_deref(), Some("attributable answer"));
    assert_eq!(
        f.typed_content()
            .iter()
            .filter(|(text, _)| text == "queued follower")
            .count(),
        1
    );
}
