//! Red-first regression suite for non-retirement interrupt delivery across providers.
use super::*;
use crate::messaging::{control::Response, native::TransportError};
use std::collections::VecDeque;

fn send_msg(fixture: &mut Fixture, params: Value) -> Response {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("interrupt-deliver-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            method: "message.send".into(),
            params,
        },
        None,
    )
}

fn recover_req(fixture: &mut Fixture, request: RequestId) -> Response {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("recover-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            method: "request.recover".into(),
            params: json!({"request": request.0.to_string(), "confirm": true}),
        },
        None,
    )
}

fn sent_request(response: &Response) -> RequestId {
    RequestId(
        response.result["request_ids"][0]
            .as_u64()
            .unwrap_or_else(|| panic!("no request in {response:?}")),
    )
}

#[derive(Clone, Copy, Debug)]
enum StopOutcomeCase {
    Stopped,
    AlreadyIdle,
    Failed,
    Unknown,
}

struct ScriptedStopTransport {
    native: Arc<Mutex<Vec<Method>>>,
    stop_results: VecDeque<Result<ResponseResult, TransportError>>,
    prompt_results: VecDeque<Result<ResponseResult, TransportError>>,
}

impl ScriptedStopTransport {
    fn new(
        native: Arc<Mutex<Vec<Method>>>,
        stop_results: impl IntoIterator<Item = Result<ResponseResult, TransportError>>,
        prompt_results: impl IntoIterator<Item = Result<ResponseResult, TransportError>>,
    ) -> Self {
        Self {
            native,
            stop_results: stop_results.into_iter().collect(),
            prompt_results: prompt_results.into_iter().collect(),
        }
    }
}

impl Transport for ScriptedStopTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.native.lock().unwrap().push(method.clone());
        match &method {
            Method::AgentSendKeys(_) => {
                if let Some(result) = self.stop_results.pop_front() {
                    return result;
                }
                Ok(ResponseResult::Ok {})
            }
            Method::AgentPromptIfIdle(params) | Method::AgentPromptIfIdleAtGeneration(params) => {
                if let Some(result) = self.prompt_results.pop_front() {
                    return result;
                }
                Ok(ResponseResult::AgentPrompted {
                    agent: serde_json::from_value(json!({
                        "terminal_id": params.expected_terminal_id,
                        "pane_id": params.expected_pane_id,
                        "agent": params.expected_agent,
                        "agent_status": "idle",
                        "interactive_ready": true,
                        "workspace_id": "workspace", "tab_id": "tab",
                        "focused": false, "revision": 1,
                        "agent_session": {
                            "source": "bus", "agent": params.expected_agent,
                            "kind": "id", "value": params.expected_session_id,
                        },
                    }))
                    .unwrap(),
                    timestamps: Default::default(),
                    discarded_aborted_payload: false,
                })
            }
            _ => Ok(ResponseResult::Ok {}),
        }
    }
}

fn setup_busy_agent(provider: Provider, case: StopOutcomeCase) -> (Fixture, RequestId) {
    let mut fixture = Fixture::new(provider);
    let lead = match case {
        StopOutcomeCase::AlreadyIdle => {
            // Submitted (unbound) request while agent is already Idle -> needs_interrupt_stop is false -> already_idle.
            let id = fixture.send("already idle turn", false);
            fixture.deliver();
            fixture.status(RuntimeStatus::Idle);
            id
        }
        _ => running(&mut fixture, "long running turn", "turn-1"),
    };
    let stop_result = match case {
        StopOutcomeCase::Stopped => Ok(ResponseResult::AgentInterrupted {
            terminal_id: "terminal".into(),
            session_id: "session".into(),
            turn_id: Some("turn-1".into()),
        }),
        StopOutcomeCase::AlreadyIdle => Ok(ResponseResult::Ok {}),
        StopOutcomeCase::Failed => Err(TransportError {
            code: Some("agent_send_keys_failed".into()),
            message: "simulated stop write failure".into(),
            definitely_rejected: true,
            timestamps: Default::default(),
        }),
        StopOutcomeCase::Unknown => Err(TransportError {
            code: Some("timeout".into()),
            message: "simulated uncertain stop timeout".into(),
            definitely_rejected: false,
            timestamps: Default::default(),
        }),
    };
    let native = fixture.native.clone();
    fixture.worker.transport = Box::new(ScriptedStopTransport::new(
        native,
        std::iter::once(stop_result),
        std::iter::empty(),
    ));
    (fixture, lead)
}

struct PendingClaudeSteer {
    inner: ScriptedStopTransport,
    code: &'static str,
}

impl Transport for PendingClaudeSteer {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.inner.request(method)
    }

    fn submit(&mut self, method: Method) -> crate::messaging::native::NativeCall {
        use crate::messaging::native::NativeCall;
        if !matches!(&method, Method::AgentPromptIfIdleAtGeneration(params) if params.steer) {
            return NativeCall::Complete(Box::new(self.request(method)));
        }
        self.inner.native.lock().unwrap().push(method);
        let (sender, receiver) = std::sync::mpsc::channel();
        sender
            .send(Err(TransportError {
                code: Some(self.code.into()),
                message: "Claude still held the prompt".into(),
                definitely_rejected: false,
                timestamps: Default::default(),
            }))
            .unwrap();
        NativeCall::Pending(receiver)
    }
}

#[test]
fn interrupt_send_settles_the_pending_claude_receipt_before_stopping_the_lead() {
    use crate::protocol::api::{AGENT_CLAUDE_TURN_BUSY, QUEUED_IN_TARGET_INPUT};
    for code in [AGENT_CLAUDE_TURN_BUSY, QUEUED_IN_TARGET_INPUT] {
        let mut fixture = Fixture::new(Provider::ClaudeDs);
        let lead = running(&mut fixture, "old work", "turn-1");
        let next = fixture.send("hold for the next turn", false);
        fixture.worker.transport = Box::new(PendingClaudeSteer {
            inner: ScriptedStopTransport::new(
                fixture.native.clone(),
                [Ok(ResponseResult::Ok {})],
                std::iter::empty(),
            ),
            code,
        });
        fixture.deliver();
        assert_eq!(fixture.request(next).phase, RequestPhase::Submitting);
        let params = json!({
            "room": fixture.room.0.to_string(),
            "to": ["builder"],
            "text": "new work",
        });
        let response = send_msg(&mut fixture, params);
        assert!(response.ok, "{response:?}");
        assert_eq!(
            fixture.request(lead).failure_reason.as_deref(),
            Some("interrupted")
        );
        assert_eq!(fixture.request(next).group, None);
        if code == AGENT_CLAUDE_TURN_BUSY {
            assert_eq!(fixture.request(next).phase, RequestPhase::Queued);
            assert!(fixture.request(next).failure_reason.is_none());
        } else {
            assert_eq!(fixture.request(next).phase, RequestPhase::Abandoned);
            assert_eq!(
                fixture.request(next).failure_reason.as_deref(),
                Some(QUEUED_IN_TARGET_INPUT)
            );
        }
    }
}

#[test]
fn busy_claude_codex_and_agy_interrupt_deliver_without_waiting_for_turn_end() {
    for provider in [
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Codex,
        Provider::Agy,
    ] {
        for case in [
            StopOutcomeCase::Stopped,
            StopOutcomeCase::AlreadyIdle,
            StopOutcomeCase::Failed,
            StopOutcomeCase::Unknown,
        ] {
            let (mut fixture, lead) = setup_busy_agent(provider, case);
            let before_typed = fixture.typed().len();
            let params = json!({
                "room": fixture.room.0.to_string(),
                "to": ["builder"],
                "text": "deliver now after interrupt",
            });
            let response = send_msg(&mut fixture, params);
            assert!(response.ok, "{provider:?}/{case:?}: {response:?}");
            let message = sent_request(&response);

            // Deliver WITHOUT waiting for the old turn to end (no Idle status poll).
            fixture.deliver();

            let typed = fixture.typed_content();
            assert!(
                typed.len() > before_typed
                    && typed
                        .last()
                        .is_some_and(|(text, _)| text.ends_with("deliver now after interrupt")),
                "{provider:?}/{case:?}: interrupting message was not delivered without waiting for turn end; typed={typed:?}, lead={:?}, msg={:?}",
                fixture.request(lead),
                fixture.request(message),
            );
            assert_eq!(
                fixture.request(message).phase,
                RequestPhase::Active,
                "{provider:?}/{case:?}: delivered message must be Active"
            );
            assert_eq!(
                fixture.request(message).group,
                None,
                "{provider:?}/{case:?}: interrupting message must run as its own turn"
            );
            assert!(
                fixture.request(lead).settled(),
                "{provider:?}/{case:?}: interrupted lead must be settled, not held"
            );
        }
    }
}

#[test]
fn interrupt_delivery_refused_while_turn_running_retries_as_queued_turn() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Agy] {
        let mut fixture = Fixture::new(provider);
        let _lead = running(&mut fixture, "long running turn", "turn-1");
        let native = fixture.native.clone();
        fixture.worker.transport = Box::new(ScriptedStopTransport::new(
            native,
            [Ok(ResponseResult::Ok {})],
            [Err(TransportError {
                code: Some("agent_not_idle".into()),
                message: "Agent is not idle; prompt was not sent".into(),
                definitely_rejected: true,
                timestamps: Default::default(),
            })],
        ));
        let params = json!({
            "room": fixture.room.0.to_string(),
            "to": ["builder"],
            "text": "retry when pane accepts",
        });
        let response = send_msg(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let message = sent_request(&response);

        // First attempt hits refusal because turn is still running -> stays queued.
        fixture.deliver();
        assert_eq!(
            fixture.request(message).phase,
            RequestPhase::Queued,
            "{provider:?}: refused delivery must remain queued for retry"
        );

        // Next retry window delivers as queued turn.
        fixture.worker.submission_rechecks.clear();
        fixture.status(RuntimeStatus::Idle);
        fixture.deliver();
        assert_eq!(
            fixture.request(message).phase,
            RequestPhase::Active,
            "{provider:?}: retry window must deliver the queued message"
        );
        let typed = fixture.typed_content();
        assert!(
            typed
                .last()
                .is_some_and(|(text, _)| text.ends_with("retry when pane accepts")),
            "{provider:?}: {typed:?}"
        );
    }
}

#[test]
fn interrupted_agent_queue_always_drains_without_retirement_hold() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Agy] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "old turn", "1");
        let queued_first = fixture.send("queued first", true);
        let params = json!({
            "room": fixture.room.0.to_string(),
            "to": ["builder"],
            "text": "interrupt second",
        });
        let response = send_msg(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let interrupt_second = sent_request(&response);

        // Lead is closed; first queued item delivers immediately without retirement hold.
        fixture.deliver();
        assert!(fixture.request(lead).settled(), "{provider:?}");
        assert_eq!(
            fixture.request(queued_first).phase,
            RequestPhase::Active,
            "{provider:?}"
        );
        let first_payload = fixture.typed().last().unwrap().0.clone();
        fixture.started("10", &first_payload);
        fixture.consume();
        fixture.stopped("10", "done first");
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);

        // Second queued item drains next.
        fixture.deliver();
        assert_eq!(
            fixture.request(interrupt_second).phase,
            RequestPhase::Active,
            "{provider:?}"
        );
        let second_payload = fixture.typed().last().unwrap().0.clone();
        fixture.started("20", &second_payload);
        fixture.consume();
        fixture.stopped("20", "done second");
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        assert!(
            fixture
                .worker
                .state
                .queued_requests(fixture.agent)
                .is_empty(),
            "{provider:?}: queue must drain completely"
        );
        assert_eq!(fixture.reply(queued_first).as_deref(), Some("done first"));
        assert_eq!(
            fixture.reply(interrupt_second).as_deref(),
            Some("done second")
        );
    }
}

#[test]
fn genuinely_stuck_request_remains_recoverable_without_retirement() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Agy] {
        let mut fixture = Fixture::new(provider);
        let stuck = running(&mut fixture, "stuck turn", "turn-1");
        let follower = fixture.send("after recovery", true);
        // Provider went idle without emitting Stop -> genuinely stuck Active request.
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(fixture.request(stuck).phase, RequestPhase::Active);

        let recovered = recover_req(&mut fixture, stuck);
        assert!(recovered.ok, "{provider:?}: {recovered:?}");
        assert_eq!(recovered.result["stage"], "abandoned", "{provider:?}");
        assert_eq!(fixture.request(stuck).phase, RequestPhase::Abandoned);

        fixture.deliver();
        assert_eq!(
            fixture.request(follower).phase,
            RequestPhase::Active,
            "{provider:?}: queue must drain after manual recovery"
        );
    }
}

fn msg_status(fixture: &mut Fixture, message: PromptId) -> Response {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("status-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            method: "message.status".into(),
            params: json!({"message": message.0.to_string()}),
        },
        None,
    )
}

#[test]
fn interrupt_delivery_passes_aborted_payload_and_records_discarded_close_detail() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = fixture.send("M2_IDLE_PAYLOAD report alpha", false);
    fixture.deliver();
    let lead_payload = fixture.typed().last().unwrap().0.clone();
    let lead_msg = fixture.request(lead).prompt.id;
    fixture.started("turn-idle-1", &lead_payload);
    fixture.consume();
    fixture.status(RuntimeStatus::Working);

    let native = fixture.native.clone();
    let agent_info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
        "terminal_id": "terminal",
        "pane_id": "pane",
        "agent": "claude",
        "agent_status": "idle",
        "interactive_ready": true,
        "workspace_id": "workspace", "tab_id": "tab",
        "focused": false, "revision": 1,
        "agent_session": {
            "source": "bus", "agent": "claude",
            "kind": "id", "value": "session",
        },
    }))
    .unwrap();
    fixture.worker.transport = Box::new(ScriptedStopTransport::new(
        native.clone(),
        std::iter::once(Ok(ResponseResult::Ok {})),
        std::iter::once(Ok(ResponseResult::AgentPrompted {
            agent: agent_info,
            timestamps: Default::default(),
            discarded_aborted_payload: true,
        })),
    ));

    let room = fixture.room.0.to_string();
    let resp = send_msg(
        &mut fixture,
        json!({
            "room": room,
            "to": ["builder"],
            "text": "M3_INTERRUPT_ACK",
        }),
    );
    assert!(resp.ok, "{resp:?}");
    let incoming = sent_request(&resp);
    fixture.deliver();

    // The interrupt delivery carried the exact aborted payload of `lead`.
    let sent_prompt = native
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|method| match method {
            Method::AgentPromptIfIdleAtGeneration(params) => Some(params.clone()),
            _ => None,
        })
        .expect("expected interrupt delivery prompt");
    assert_eq!(
        sent_prompt.discard_aborted_payload.as_deref(),
        Some(lead_payload.as_str())
    );
    assert_eq!(fixture.request(incoming).phase, RequestPhase::Active);

    // Closed lead records discarded_aborted_payload as its close detail.
    let status = msg_status(&mut fixture, lead_msg);
    assert!(status.ok, "{status:?}");
    let row = &status.result["requests"][0];
    assert_eq!(row["stage"], "abandoned");
    assert_eq!(row["reason"], "interrupted");
    assert_eq!(row["detail"], "discarded_aborted_payload");
    assert!(row["reply"].is_null());

    // After confirmation, aborted_payload is cleared so a subsequent delivery carries None.
    let incoming_payload = sent_prompt.text;
    fixture.started("turn-2", &incoming_payload);
    fixture.consume();
    fixture.stopped("turn-2", "M3_ACK");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);

    let follow = fixture.send("afterward", false);
    fixture.deliver();
    assert_eq!(fixture.request(follow).phase, RequestPhase::Active);
    let follow_prompt = native
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|method| match method {
            Method::AgentPromptIfIdleAtGeneration(params) => Some(params.clone()),
            _ => None,
        })
        .expect("expected follow-up prompt");
    assert_eq!(follow_prompt.discard_aborted_payload, None);
}

#[test]
fn working_steer_refused_then_interrupt_discards_aborted_lead_on_first_queued_delivery() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "long glossary turn", "turn-1");
    let lead_payload = fixture.typed().last().unwrap().0.clone();
    let lead_msg = fixture.request(lead).prompt.id;

    let native = fixture.native.clone();
    let agent_info: crate::protocol::api::schema::AgentInfo = serde_json::from_value(json!({
        "terminal_id": "terminal",
        "pane_id": "pane",
        "agent": "claude",
        "agent_status": "idle",
        "interactive_ready": true,
        "workspace_id": "workspace", "tab_id": "tab",
        "focused": false, "revision": 1,
        "agent_session": {
            "source": "bus", "agent": "claude",
            "kind": "id", "value": "session",
        },
    }))
    .unwrap();
    fixture.worker.transport = Box::new(ScriptedStopTransport::new(
        native.clone(),
        std::iter::once(Ok(ResponseResult::Ok {})),
        vec![
            // 1. Working steer refused by ClaudeTurnBusy (ComposerBusy)
            Err(TransportError {
                code: Some(crate::protocol::api::AGENT_CLAUDE_TURN_BUSY.into()),
                message: "Claude turn busy".into(),
                definitely_rejected: true,
                timestamps: Default::default(),
            }),
            // 2. First delivery after interrupt (queued steer) discards aborted lead payload
            Ok(ResponseResult::AgentPrompted {
                agent: agent_info.clone(),
                timestamps: Default::default(),
                discarded_aborted_payload: true,
            }),
            // 3. Interrupt message delivers afterwards with no aborted payload left
            Ok(ResponseResult::AgentPrompted {
                agent: agent_info,
                timestamps: Default::default(),
                discarded_aborted_payload: false,
            }),
        ],
    ));

    // Send M2 in steer mode (non-queue) while lead is Working -> stays queued after refusal.
    let m2_steer = fixture.send("M2_WORKING_STEER_B report beta", false);
    fixture.deliver();
    assert_eq!(fixture.request(m2_steer).phase, RequestPhase::Queued);

    // Send M3 in interrupt mode -> closes lead, records lead_payload as aborted_payload.
    let room = fixture.room.0.to_string();
    let resp = send_msg(
        &mut fixture,
        json!({
            "room": room,
            "to": ["builder"],
            "text": "M3_WORKING_INT_B",
        }),
    );
    assert!(resp.ok, "{resp:?}");
    let m3_interrupt = sent_request(&resp);

    // Stop completes -> agent becomes Idle; first delivery after stop is queued M2,
    // which receives and discards lead_payload.
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert_eq!(fixture.request(m2_steer).phase, RequestPhase::Active);
    let status = msg_status(&mut fixture, lead_msg);
    assert_eq!(status.result["requests"][0]["stage"], "abandoned");
    assert_eq!(status.result["requests"][0]["reason"], "interrupted");
    assert_eq!(
        status.result["requests"][0]["detail"],
        "discarded_aborted_payload"
    );

    // Finish M2 and deliver M3 -> M3 receives discard_aborted_payload: None.
    let m2_prompt = native
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|method| match method {
            Method::AgentPromptIfIdleAtGeneration(params) if !params.steer => Some(params.clone()),
            _ => None,
        })
        .expect("expected M2 delivery");
    assert_eq!(
        m2_prompt.discard_aborted_payload.as_deref(),
        Some(lead_payload.as_str())
    );
    fixture.started("turn-2", &m2_prompt.text);
    fixture.consume();
    fixture.stopped("turn-2", "beta");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);

    fixture.deliver();
    assert_eq!(fixture.request(m3_interrupt).phase, RequestPhase::Active);
    let m3_prompt = native
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find_map(|method| match method {
            Method::AgentPromptIfIdleAtGeneration(params) if !params.steer => Some(params.clone()),
            _ => None,
        })
        .expect("expected M3 delivery");
    assert_eq!(m3_prompt.discard_aborted_payload, None);
}
