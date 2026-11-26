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
        assert_eq!(fixture.request(lead).group, Some(sent_request(&response)));
        assert!(fixture.request(lead).failure_reason.is_none());
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

            if provider.uses_claude_harness() && matches!(case, StopOutcomeCase::Failed) {
                assert_eq!(fixture.typed().len(), before_typed);
                assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
                assert_eq!(fixture.request(message).phase, RequestPhase::Queued);
                fixture.stopped("turn-1", "completed despite refused stop");
                fixture.consume();
                fixture.status(RuntimeStatus::Idle);
                fixture.deliver();
                assert_eq!(fixture.request(message).phase, RequestPhase::Active);
                assert_eq!(
                    fixture.reply(lead).as_deref(),
                    Some("completed despite refused stop")
                );
                continue;
            }
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
                fixture.request(lead).group,
                provider.uses_claude_harness().then_some(message),
                "{provider:?}/{case:?}: Claude shares the replacement reply with the old request"
            );
            assert_eq!(
                fixture.request(lead).settled(),
                !provider.uses_claude_harness(),
                "{provider:?}/{case:?}"
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

        // The interrupt runs first and the earlier ordinary request still runs.
        for (turn, request, reply) in [
            ("10", interrupt_second, "interrupt done"),
            ("20", queued_first, "queued work done"),
        ] {
            fixture.deliver();
            assert_eq!(
                fixture.request(request).phase,
                RequestPhase::Active,
                "{provider:?}"
            );
            let payload = fixture.typed().last().unwrap().0.clone();
            fixture.started(turn, &payload);
            fixture.consume();
            fixture.stopped(turn, reply);
            fixture.consume();
            fixture.status(RuntimeStatus::Idle);
            assert_eq!(fixture.reply(request).as_deref(), Some(reply));
        }
        assert!(
            fixture
                .worker
                .state
                .queued_requests(fixture.agent)
                .is_empty(),
            "{provider:?}: queue must drain completely"
        );
        if provider.uses_claude_harness() {
            assert_eq!(fixture.reply(lead).as_deref(), Some("interrupt done"));
        } else {
            assert!(fixture.request(lead).settled());
        }
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
fn interrupt_delivery_clears_the_returned_copy_and_replays_the_lead() {
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
        sent_prompt
            .discard_aborted_payload
            .as_ref()
            .map(|action| action.payload()),
        Some(lead_payload.as_str())
    );
    assert_eq!(fixture.request(incoming).phase, RequestPhase::Active);

    // Clearing the terminal copy must leave the original request open.
    let status = msg_status(&mut fixture, lead_msg);
    assert!(status.ok, "{status:?}");
    let row = &status.result["requests"][0];
    assert_eq!(row["stage"], "joined");
    assert!(row["reason"].is_null());
    assert!(!sent_prompt.text.contains("M2_IDLE_PAYLOAD report alpha"));
    assert_eq!(
        sent_prompt
            .discard_aborted_payload
            .as_ref()
            .and_then(|action| action.replay()),
        Some(lead_payload.as_str())
    );

    // The mock returned positive discard proof, so the callback includes the
    // original body before the replacement, and fixes the submitted variant.
    let incoming_payload = format!("{lead_payload}\n\n{}", sent_prompt.text);
    fixture.started("turn-2", &incoming_payload);
    fixture.consume();
    fixture.stopped("turn-2", "M3_ACK");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);

    assert_eq!(fixture.reply(lead).as_deref(), Some("M3_ACK"));
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
fn a_cleared_returned_prompt_survives_a_definite_replacement_refusal() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "keep this returned directive", "old-turn");
    f.worker.transport = Box::new(ScriptedStopTransport::new(
        f.native.clone(),
        [Ok(ResponseResult::Ok {})],
        [Err(TransportError {
            code: Some(crate::protocol::api::AGENT_RETURNED_DRAFT_CLEARED.into()),
            message: "returned draft cleared, replacement not submitted".into(),
            definitely_rejected: true,
            timestamps: Default::default(),
        })],
    ));
    let room = f.room.0.to_string();
    let sent = send_msg(
        &mut f,
        json!({"room": room, "to": ["builder"], "text": "new work"}),
    );
    let replacement = sent_request(&sent);
    f.deliver();
    assert_eq!(f.request(replacement).phase, RequestPhase::Queued);
    let saved = f.worker.store.load().unwrap().unwrap();
    assert!(
        saved
            .agent(f.agent)
            .unwrap()
            .autonomous_stop
            .aborted_payload
            .as_ref()
            .unwrap()
            .cleared
    );

    f.status(RuntimeStatus::Idle);
    f.worker.submission_rechecks.clear();
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    assert_eq!(
        payload.matches("keep this returned directive").count(),
        1,
        "{payload:?}"
    );
    assert!(payload.ends_with("new work"), "{payload:?}");
    f.started("new-turn", &payload);
    f.consume();
    f.stopped("new-turn", "done");
    f.consume();
    f.status(RuntimeStatus::Idle);
    for request in [lead, replacement] {
        assert_eq!(f.reply(request).as_deref(), Some("done"));
    }
}

#[test]
fn interrupt_bypasses_refused_steer_and_recovers_the_returned_lead() {
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
            // 2. The interrupt clears the returned draft and sends its replay.
            Ok(ResponseResult::AgentPrompted {
                agent: agent_info.clone(),
                timestamps: Default::default(),
                discarded_aborted_payload: true,
            }),
            // 3. The earlier refused steer runs independently next.
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

    let room = fixture.room.0.to_string();
    let resp = send_msg(
        &mut fixture,
        json!({"room": room, "to": ["builder"], "text": "M3_WORKING_INT_B"}),
    );
    assert!(resp.ok, "{resp:?}");
    let interrupt = sent_request(&resp);

    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert_eq!(fixture.request(interrupt).phase, RequestPhase::Active);
    assert_eq!(fixture.request(m2_steer).phase, RequestPhase::Queued);
    let status = msg_status(&mut fixture, lead_msg);
    assert_eq!(status.result["requests"][0]["stage"], "joined");

    let last_prompt = || {
        native
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find_map(|method| match method {
                Method::AgentPromptIfIdleAtGeneration(params) if !params.steer => {
                    Some(params.clone())
                }
                _ => None,
            })
            .unwrap()
    };
    let incoming = last_prompt();
    assert_eq!(
        incoming
            .discard_aborted_payload
            .as_ref()
            .and_then(|action| action.replay()),
        Some(lead_payload.as_str())
    );
    let payload = format!("{lead_payload}\n\n{}", incoming.text);
    fixture.started("turn-2", &payload);
    fixture.consume();
    fixture.stopped("turn-2", "interrupt done");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("interrupt done"));
    assert_eq!(fixture.reply(interrupt).as_deref(), Some("interrupt done"));

    fixture.deliver();
    assert_eq!(fixture.request(m2_steer).phase, RequestPhase::Active);
    let next = last_prompt();
    assert_eq!(next.discard_aborted_payload, None);
    assert!(next.text.ends_with("M2_WORKING_STEER_B report beta"));
}

#[test]
fn claude_back_to_back_interrupts_keep_started_requests_without_retyping_them() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let mut requests = vec![running(&mut f, "first committed instruction", "turn-1")];
    let room = f.room.0.to_string();
    let mut typed_instructions = vec![String::from("first committed instruction")];
    for (turn, instruction) in [
        ("turn-2", "second committed instruction"),
        ("turn-3", "third committed instruction"),
    ] {
        let sent = send_msg(
            &mut f,
            json!({"room": room, "to": ["builder"], "text": instruction}),
        );
        assert!(sent.ok, "{sent:?}");
        requests.push(sent_request(&sent));
        f.deliver();
        let payload = f.typed().last().unwrap().0.clone();
        assert!(payload.ends_with(instruction), "{payload:?}");
        for previous in &typed_instructions {
            assert!(
                !payload.contains(previous),
                "retyped {previous:?}: {payload:?}"
            );
        }
        typed_instructions.push(instruction.to_owned());
        f.started(turn, &payload);
        f.consume();
        f.status(RuntimeStatus::Working);
        let (replacement, interrupted) = requests.split_last().unwrap();
        assert!(f.request(*replacement).trusted_start_bound);
        for previous in interrupted {
            assert_eq!(f.request(*previous).group, Some(*replacement));
        }
    }

    f.stopped("turn-3", "done after the final instruction");
    f.consume();
    f.status(RuntimeStatus::Idle);
    for request in requests {
        assert_eq!(
            f.reply(request).as_deref(),
            Some("done after the final instruction")
        );
    }
}

#[test]
fn claude_interrupt_keeps_the_prior_attachment_and_accepts_its_explicit_reply() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let attachment = f.dir.join("reference image.png");
    std::fs::write(&attachment, b"attachment fixture").unwrap();
    let attachment = attachment.canonicalize().unwrap();
    let room = f.room.0.to_string();
    let sent = send_msg(
        &mut f,
        json!({"room": room, "to": ["builder"], "text": "interrupted work", "files": [attachment]}),
    );
    let lead = sent_request(&sent);
    let lead_prompt = f.request(lead).prompt.id;
    f.deliver();
    f.started("old-turn", &f.typed().last().unwrap().0);
    f.consume();
    f.status(RuntimeStatus::Working);
    assert!(f.request(lead).trusted_start_bound);

    let sent = send_msg(
        &mut f,
        json!({"room": room, "to": ["builder"], "text": "new instruction"}),
    );
    let replacement = sent_request(&sent);
    f.deliver();

    let payload = f.typed().last().unwrap().0.clone();
    assert!(!payload.contains("interrupted work"));
    assert!(!payload.contains(attachment.to_str().unwrap()));
    assert!(payload.ends_with("new instruction"));
    f.started("new-turn", &payload);
    f.consume();
    let agent = f.agent.0.to_string();
    let reply = send_msg(
        &mut f,
        json!({
            "as": agent, "reply_to": lead_prompt.0.to_string(), "text": "answered early"
        }),
    );
    assert!(reply.ok, "{reply:?}");
    f.stopped("new-turn", "both prompts handled");
    f.consume();
    f.status(RuntimeStatus::Idle);
    assert_eq!(f.reply(lead).as_deref(), Some("answered early"));
    assert_eq!(
        f.reply(replacement).as_deref(),
        Some("both prompts handled")
    );
}

#[test]
fn claude_interrupt_stops_a_new_autonomous_turn_before_retrying_delivery() {
    for busy_receipt_first in [false, true] {
        let mut f = Fixture::new(Provider::ClaudeDs);
        let lead = running(&mut f, "interrupted work", "old-turn");
        let room = f.room.0.to_string();
        let sent = send_msg(
            &mut f,
            json!({"room": room, "to": ["builder"], "text": "new instruction"}),
        );
        let replacement = sent_request(&sent);
        if busy_receipt_first {
            // Claude woke before the native attempt, and its callback
            // has not reached the coordinator yet.
            f.worker.transport = Box::new(ScriptedStopTransport::new(
                f.native.clone(),
                [Ok(ResponseResult::Ok {})],
                [Err(TransportError {
                    code: Some(crate::protocol::api::AGENT_CLAUDE_TURN_BUSY.into()),
                    message: "Claude turn busy".into(),
                    definitely_rejected: true,
                    timestamps: Default::default(),
                })],
            ));
            f.deliver();
            assert_eq!(f.request(replacement).phase, RequestPhase::Queued);
        }
        f.started(
            "shell-wake",
            "<task-notification><task-id>shell</task-id></task-notification>",
        );
        f.consume();
        f.status(RuntimeStatus::Working);
        f.deliver();
        f.deliver();
        let stop_count = f
            .native
            .lock()
            .unwrap()
            .iter()
            .filter(|method| matches!(method, Method::AgentSendKeys(_)))
            .count();
        assert_eq!(stop_count, 2, "one stop for each observed turn");

        f.status(RuntimeStatus::Idle);
        let mut state = f.worker.state.clone();
        state.observe_autonomous_idle(f.agent, f.now).unwrap();
        f.worker.save(state).unwrap();
        f.worker.submission_rechecks.clear();
        f.deliver();

        let payload = f.typed().last().unwrap().0.clone();
        assert!(!payload.contains("interrupted work"), "{payload:?}");
        assert!(payload.ends_with("new instruction"), "{payload:?}");
        f.started("new-turn", &payload);
        f.consume();
        f.stopped("new-turn", "both prompts handled");
        f.consume();
        f.status(RuntimeStatus::Idle);
        for request in [lead, replacement] {
            assert_eq!(f.reply(request).as_deref(), Some("both prompts handled"));
        }
    }
}
