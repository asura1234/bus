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

struct RetirementClose(Arc<Mutex<Vec<Method>>>, bool);

impl Transport for RetirementClose {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(&method, Method::PaneLocateProcess(_)) {
            return Ok(ResponseResult::CallerLocation {
                location: crate::protocol::api::schema::CallerLocation {
                    pane: None,
                    agent_owned: false,
                },
            });
        }
        if matches!(&method, Method::PaneCloseIfIdentity(_)) {
            self.0.lock().unwrap().push(method);
            return if self.1 {
                Ok(ResponseResult::Ok {})
            } else {
                Err(TransportError {
                    code: Some("terminal_identity_changed".into()),
                    message: "The old terminal still has unverifiable ownership".into(),
                    definitely_rejected: true,
                    timestamps: Default::default(),
                })
            };
        }
        Native(self.0.clone()).request(method)
    }
}

#[test]
fn revoked_or_missing_receipt_requires_verified_terminal_retirement() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let first = f.send("first follower", true);
    let second = f.send("second follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    f.status(RuntimeStatus::Idle);
    f.worker.transport = Box::new(RetirementClose(f.native.clone(), false));
    assert!(confirm_retirement_in_ui(&mut f, lead).is_err());
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
    // Reopen the actual Worker to check the old terminal fence survives.
    let old_worker = std::mem::replace(
        &mut f.worker,
        Worker::open(
            crate::utils::test_temp::unique_temp_path("retirement-restart-swap"),
            Box::new(Native(f.native.clone())),
        )
        .unwrap(),
    );
    drop(old_worker);
    f.worker = Worker::open(
        f.dir.clone(),
        Box::new(RetirementClose(f.native.clone(), true)),
    )
    .unwrap();
    assert!(f.request(lead).interrupt_retirement_pending);
    confirm_retirement_in_ui(&mut f, lead).unwrap();
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("interrupt_outcome_unknown")
    );
    assert_eq!(f.worker.state.queued_requests(f.agent), &[first, second]);
    assert_eq!(f.worker.state.agent(f.agent).unwrap().current_request, None);
    confirm_retirement_in_ui(&mut f, lead).unwrap();
    let calls = f.native.lock().unwrap();
    let closes: Vec<_> = calls
        .iter()
        .filter_map(|call| match call {
            Method::PaneCloseIfIdentity(p) => Some(p),
            _ => None,
        })
        .collect();
    assert_eq!(closes.len(), 2);
    assert!(closes.iter().all(|p| p.expected_terminal_id == "terminal"
        && p.expected_session_id.as_deref() == Some("session")));
    drop(calls);
    // Once a new session is ready, only the two queued requests may run.
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
    assert_eq!(
        f.typed().len(),
        3,
        "no replay or duplicate follower dispatch"
    );
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

fn retire_with_caller(
    f: &mut Fixture,
    lead: RequestId,
    nonce: &str,
    caller: Option<Value>,
) -> crate::messaging::control::Response {
    let mut params =
        json!({"request": lead.0.to_string(), "confirm": true, "retire_terminal": true});
    if let Some(caller) = caller {
        params["caller"] = caller;
    }
    f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: nonce.into(),
            method: "request.recover".into(),
            params,
        },
        None,
    )
}

struct RetirementOperator(Arc<Mutex<Vec<Method>>>, Option<bool>);

impl Transport for RetirementOperator {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match &method {
            Method::PaneLocateProcess(_) => {
                self.0.lock().unwrap().push(method);
                let agent_owned = self.1.ok_or_else(|| TransportError {
                    code: Some("caller_unknown".into()),
                    message: "unresolved process".into(),
                    definitely_rejected: true,
                    timestamps: Default::default(),
                })?;
                let pane = agent_owned.then(|| {
                    serde_json::from_value(json!({
                        "terminal_id":"terminal", "pane_id":"pane", "agent":"claude",
                        "workspace_id":"w", "tab_id":"t", "focused":false,
                        "agent_status":"working", "revision":1
                    }))
                    .unwrap()
                });
                Ok(ResponseResult::CallerLocation {
                    location: crate::protocol::api::schema::CallerLocation { pane, agent_owned },
                })
            }
            _ => RetirementClose(self.0.clone(), true).request(method),
        }
    }
}

fn confirm_retirement_in_ui(f: &mut Fixture, request: RequestId) -> Result<(), String> {
    let original = f.request(request).native_delivery_identity.clone().unwrap();
    f.worker.command(
        BusCommand::ConfirmTerminalRetirement { request, original },
        &mpsc::channel().0,
    )
}

fn verified_retirement_caller() -> Value {
    json!({"process":{"pid":123,"birth":456}})
}

#[test]
fn round2_deferred_joined_answers_survive_guarded_retry() {
    deferred_joined_answers_survive_retirement(false);
}

#[test]
fn round2_deferred_joined_answers_survive_confirmed_teardown() {
    deferred_joined_answers_survive_retirement(true);
}

fn deferred_joined_answers_survive_retirement(teardown: bool) {
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
    if teardown {
        f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(false)));
        confirm_retirement_in_ui(&mut f, lead).unwrap();
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
        "teardown={teardown}"
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

#[test]
fn round2_terminal_retirement_requires_a_verified_developer_operator() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    for (nonce, native_owner, caller) in [
        ("omitted-caller", Some(true), None),
        (
            "agent-origin",
            Some(true),
            Some(verified_retirement_caller()),
        ),
        (
            "unresolved-origin",
            None,
            Some(verified_retirement_caller()),
        ),
    ] {
        f.worker.transport = Box::new(RetirementOperator(f.native.clone(), native_owner));
        let response = retire_with_caller(&mut f, lead, nonce, caller);
        assert!(!response.ok, "{nonce}: {response:?}");
        assert!(f.request(lead).interrupt_retirement_pending);
        assert!(!f
            .native
            .lock()
            .unwrap()
            .iter()
            .any(|m| matches!(m, Method::PaneCloseIfIdentity(_))));
    }
    // Even the in-process confirmation path checks the coordinator-owning UI.
    for unverifiable_ui in [Some(true), None] {
        f.worker.transport = Box::new(RetirementOperator(f.native.clone(), unverifiable_ui));
        assert!(confirm_retirement_in_ui(&mut f, lead).is_err());
        assert!(f.request(lead).interrupt_retirement_pending);
    }
    assert!(!f
        .native
        .lock()
        .unwrap()
        .iter()
        .any(|method| matches!(method, Method::PaneCloseIfIdentity(_))));
    f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(false)));
    confirm_retirement_in_ui(&mut f, lead).unwrap();
    assert_eq!(
        f.native
            .lock()
            .unwrap()
            .iter()
            .filter(|m| matches!(m, Method::PaneCloseIfIdentity(_)))
            .count(),
        1
    );
}

#[test]
fn round2_first_interrupt_never_retires_a_preexisting_replacement_terminal() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let follower = f.send("queued follower", true);
    let mut state = f.worker.state.clone();
    let mut replacement = state.agent(f.agent).unwrap().runtime_identity.clone();
    replacement.terminal_id = Some("terminal-B".into());
    replacement.pane_id = Some("pane-B".into());
    state
        .set_agent_runtime_identity(f.agent, replacement.clone())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(false)));
    confirm_retirement_in_ui(&mut f, lead).unwrap();
    let calls = f.native.lock().unwrap();
    let targets: Vec<_> = calls
        .iter()
        .filter_map(|method| match method {
            Method::PaneCloseIfIdentity(p) => {
                Some((p.expected_terminal_id.as_str(), p.pane_id.as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(targets, [("terminal", "pane")]);
    assert!(!calls
        .iter()
        .any(|method| matches!(method, Method::AgentSendKeys(p) if p.target == "pane-B")));
    drop(calls);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().runtime_identity,
        replacement
    );
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
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
fn round2_legacy_delivery_without_original_identity_refuses_destructive_recovery() {
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
    f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(false)));
    let replacement = f
        .worker
        .state
        .agent(f.agent)
        .unwrap()
        .runtime_identity
        .clone();
    assert!(f
        .worker
        .command(
            BusCommand::ConfirmTerminalRetirement {
                request: lead,
                original: replacement
            },
            &mpsc::channel().0,
        )
        .is_err());
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    assert!(!f.native.lock().unwrap().iter().any(|method| matches!(
        method,
        Method::AgentSendKeys(_) | Method::PaneCloseIfIdentity(_)
    )));
}

#[test]
fn out_of_pane_control_relay_cannot_authorize_destructive_retirement() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);

    // The kernel peer is resolvable, but the native ancestry walk places it
    // outside all managed panes. This also describes a local control relay.
    f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(false)));
    let response = retire_with_caller(
        &mut f,
        lead,
        "outside-pane-relay",
        Some(verified_retirement_caller()),
    );
    let calls = f.native.lock().unwrap();
    assert!(calls
        .iter()
        .any(|method| matches!(method, Method::PaneLocateProcess(_))));
    assert!(
        !calls
            .iter()
            .any(|method| matches!(method, Method::PaneCloseIfIdentity(_))),
        "an ordinary control peer issued a destructive close: {response:?}"
    );
    assert!(!response.ok);
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
}

#[test]
fn terminal_retirement_ui_confirmation_is_fenced_to_the_displayed_original() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);

    let mut stale = f.request(lead).native_delivery_identity.clone().unwrap();
    stale.session_id = Some("different-session".into());
    let response = f.worker.command(
        BusCommand::ConfirmTerminalRetirement {
            request: lead,
            original: stale,
        },
        &mpsc::channel().0,
    );
    assert!(response.is_err());
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    assert!(!f
        .native
        .lock()
        .unwrap()
        .iter()
        .any(|method| matches!(method, Method::PaneCloseIfIdentity(_))));

    f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(false)));
    confirm_retirement_in_ui(&mut f, lead).unwrap();
    confirm_retirement_in_ui(&mut f, lead).unwrap();
    assert_eq!(
        f.native
            .lock()
            .unwrap()
            .iter()
            .filter(|method| matches!(method, Method::PaneCloseIfIdentity(_)))
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn terminal_retirement_control_socket_ignores_forged_caller_and_refuses_an_agent() {
    use crate::messaging::control;
    use std::{thread, time::Duration};

    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "original input", "turn-1");
    let follower = f.send("queued follower", true);
    f.worker.transport = Box::new(UnconfirmedRetirement(f.native.clone()));
    assert!(interrupt(&mut f).ok);
    f.worker.transport = Box::new(RetirementOperator(f.native.clone(), Some(true)));
    let (tx, rx) = mpsc::sync_channel(1);
    let _server = control::start(&f.dir, tx).unwrap();
    let dir = f.dir.clone();
    let sender = thread::spawn(move || {
        control::request(&dir, &control::Request {
            id: "forged-retirement-caller".into(),
            method: "request.recover".into(),
            params: json!({"request": lead.0.to_string(), "confirm":true, "retire_terminal":true,
                "caller": {"process":{"pid":1,"birth":0},"interrupt_request_id":"forged"}}),
        }).unwrap()
    });
    let (_, BusCommand::Control(call)) = rx.recv_timeout(Duration::from_secs(5)).unwrap() else {
        panic!("the wire path must only enqueue a control request");
    };
    let peer = crate::platform::process_instance(std::process::id()).unwrap();
    assert_eq!(call.request.params["caller"], json!({"process":peer}));
    let response = f.worker.control_response_with_events(&call.request, None);
    call.reply.send(response).unwrap();
    let response = sender.join().unwrap();
    assert!(!response.ok);
    assert!(f.request(lead).interrupt_retirement_pending);
    assert_eq!(f.request(follower).phase, RequestPhase::Queued);
    let calls = f.native.lock().unwrap();
    assert!(calls.iter().any(|method| matches!(method, Method::PaneLocateProcess(process) if process.pid == peer.pid && process.birth == peer.birth)));
    assert!(!calls
        .iter()
        .any(|method| matches!(method, Method::PaneCloseIfIdentity(_))));
}
