//! `bus send --mode`: who may choose how a message reaches a running turn.
use super::*;
use crate::messaging::{
    control::Response,
    model::SendInterrupt,
    native::{NativeCall, NativeResult},
};

fn send(fixture: &mut Fixture, params: Value) -> Response {
    // Each call is a new request: a repeated ID would replay the first receipt.
    static NEXT: AtomicU64 = AtomicU64::new(1);
    fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: format!("delivery-mode-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            method: "message.send".into(),
            params,
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

fn send_params(fixture: &Fixture, text: &str) -> Value {
    json!({"room":fixture.room.0.to_string(),"to":["builder"],"text":text})
}

/// The stop keys Bus pressed, per target pane.
fn stop_keys(fixture: &Fixture) -> Vec<Vec<String>> {
    fixture
        .native
        .lock()
        .unwrap()
        .iter()
        .filter_map(|method| match method {
            Method::AgentSendKeys(params) => Some(params.keys.clone()),
            _ => None,
        })
        .collect()
}

fn last_typed(fixture: &Fixture) -> (String, bool) {
    fixture
        .typed_content()
        .last()
        .cloned()
        .unwrap_or_else(|| panic!("nothing was typed"))
}

/// Agent senders carry their bare `name:` sender header; compare the text tail.
fn assert_typed(fixture: &Fixture, expected: &str, steer: bool) {
    let (text, steered) = last_typed(fixture);
    assert_eq!(steered, steer, "{text:?}");
    assert!(
        text.ends_with(expected),
        "{text:?} must end with {expected:?}"
    );
}

/// An orchestrator bound to the fixture room, as MASTER agents are.
fn orchestrator(fixture: &mut Fixture) -> AgentId {
    let mut state = fixture.worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orch = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, fixture.room).unwrap();
    fixture.worker.save(state).unwrap();
    orch
}

/// A room member that is not an orchestrator, for agent-authored sends.
fn peer(fixture: &mut Fixture) -> AgentId {
    let mut state = fixture.worker.state.clone();
    let peer = state
        .create_agent(
            fixture.room,
            "peer",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    fixture.worker.save(state).unwrap();
    peer
}

/// Providers whose stop key this fixture can model. Kimi needs its saved home.
fn providers() -> Vec<Provider> {
    let mut providers = vec![
        Provider::ClaudeCode,
        Provider::ClaudeDs,
        Provider::Codex,
        Provider::Agy,
    ];
    #[cfg(unix)]
    providers.push(Provider::Kimi);
    providers
}

fn fixture_for(provider: Provider) -> Fixture {
    let fixture = Fixture::new(provider);
    #[cfg(unix)]
    if provider == Provider::Kimi {
        super::kimi_steering_tests::home(&fixture);
    }
    fixture
}

#[test]
fn a_developer_send_interrupts_the_running_turn_and_runs_as_the_next_turn() {
    for provider in providers() {
        let mut fixture = fixture_for(provider);
        let lead = running(&mut fixture, "run the old plan", "1");
        let params = send_params(&fixture, "change course now");
        let response = send(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let message = sent_request(&response);
        assert_eq!(
            stop_keys(&fixture),
            [vec!["esc".to_owned()]],
            "{provider:?}: the developer's message must press the stop key once"
        );
        assert!(
            fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .interrupt_pending,
            "{provider:?}"
        );
        let queued = fixture.request(message);
        assert_eq!(queued.phase, RequestPhase::Queued, "{provider:?}");
        assert!(queued.queue_only, "{provider:?}");
        assert!(!queued.steered, "{provider:?}");
        assert_eq!(queued.group, None, "{provider:?}");

        // The stop key is asynchronous: nothing is typed into the dying turn.
        let typed = fixture.typed().len();
        fixture.deliver();
        fixture.deliver();
        assert_eq!(fixture.typed().len(), typed, "{provider:?}");

        // No Stop hook follows an interrupt; the idle screen ends the turn.
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(
            fixture.request(lead).failure_reason.as_deref(),
            Some("interrupted"),
            "{provider:?}"
        );
        assert!(
            !fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .interrupt_pending
        );
        fixture.deliver();
        assert_typed(&fixture, "change course now", false);
        assert_eq!(fixture.request(message).group, None, "{provider:?}");
    }
}

/// An interrupting message starts its own turn. The stopped execution's Stop
/// cannot become that message's answer; its own final does.
#[test]
fn an_interrupting_message_gets_its_own_answer_not_the_stopped_turns_final() {
    let mut fixture = fixture_for(Provider::ClaudeCode);
    let lead = running(&mut fixture, "run the old plan", "1");
    let params = send_params(&fixture, "change course now");
    let response = send(&mut fixture, params);
    let message = sent_request(&response);
    // The stopped execution still emits its Stop after the interrupt.
    fixture.stopped("1", "old plan stopped");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let notice = fixture.typed().last().unwrap().0.clone();
    assert!(notice.contains("change course now"), "{notice}");
    fixture.started("2", &notice);
    fixture.consume();
    fixture.stopped("2", "new plan done");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(message).as_deref(), Some("new plan done"));
    assert_ne!(
        fixture.reply(lead).as_deref(),
        Some("new plan done"),
        "the interrupting message's answer stays on the new request"
    );
}

#[test]
fn a_second_developer_send_while_the_stop_is_in_flight_does_not_press_again() {
    // A second Esc opens Claude's rewind and a second Ctrl+C quits Cursor.
    for provider in providers() {
        let mut fixture = fixture_for(provider);
        running(&mut fixture, "run the old plan", "1");
        let first = send_params(&fixture, "first correction");
        assert!(send(&mut fixture, first).ok);
        let params = send_params(&fixture, "second correction");
        let second = send(&mut fixture, params);
        assert!(second.ok, "{provider:?}: {second:?}");
        assert_eq!(
            stop_keys(&fixture),
            [vec!["esc".to_owned()]],
            "{provider:?}: one stop per turn"
        );
        assert!(
            fixture.request(sent_request(&second)).queue_only,
            "{provider:?}"
        );
    }
}

#[test]
fn messages_queued_before_the_interrupt_survive_and_keep_their_order() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "run the old plan", "1");
    let follower = fixture.send("earlier queued work", true);
    let params = send_params(&fixture, "change course now");
    let response = send(&mut fixture, params);
    let message = sent_request(&response);
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[follower, message]
    );
    assert!(fixture.request(follower).failure_reason.is_none());
    assert!(fixture.request(message).failure_reason.is_none());
    // FIFO: work that was already waiting is not discarded or reordered.
    fixture.deliver();
    assert_typed(&fixture, "earlier queued work", false);
    fixture.started("2", "earlier queued work");
    fixture.consume();
    fixture.stopped("2", "queued work done");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert_typed(&fixture, "change course now", false);
}

#[test]
fn orchestrator_steer_joins_the_running_turn_like_today() {
    for provider in providers() {
        let mut fixture = fixture_for(provider);
        let orch = orchestrator(&mut fixture);
        let lead = running(&mut fixture, "write the parser", "1");
        let params = json!({
            "room":fixture.room.0.to_string(),"as":orch.0.to_string(),
            "to":["builder"],"text":"also add tests","mode":"steer",
        });
        let response = send(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let message = sent_request(&response);
        assert!(stop_keys(&fixture).is_empty(), "{provider:?}");
        assert!(
            !fixture
                .worker
                .state
                .agent(fixture.agent)
                .unwrap()
                .interrupt_pending
        );
        fixture.deliver();
        assert_typed(&fixture, "also add tests", true);
        assert_eq!(fixture.request(message).group, Some(lead), "{provider:?}");
        assert!(fixture.request(message).steered, "{provider:?}");
    }
}

#[test]
fn orchestrator_queue_waits_for_the_turn_to_end() {
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut fixture = fixture_for(provider);
        let orch = orchestrator(&mut fixture);
        running(&mut fixture, "write the parser", "1");
        let params = json!({
            "room":fixture.room.0.to_string(),"as":orch.0.to_string(),
            "to":["builder"],"text":"then write the docs","mode":"queue",
        });
        let response = send(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let message = sent_request(&response);
        assert!(stop_keys(&fixture).is_empty(), "{provider:?}");
        assert!(fixture.request(message).queue_only, "{provider:?}");
        fixture.deliver();
        assert_eq!(
            fixture.typed().len(),
            1,
            "{provider:?}: a queued message is never typed into the turn"
        );
        fixture.stopped("1", "parser written");
        fixture.consume();
        fixture.status(RuntimeStatus::Idle);
        fixture.deliver();
        assert_typed(&fixture, "then write the docs", false);
    }
}

#[test]
fn orchestrator_interrupt_ends_the_turn_and_runs_next() {
    for provider in providers() {
        let mut fixture = fixture_for(provider);
        let orch = orchestrator(&mut fixture);
        let lead = running(&mut fixture, "run the old plan", "1");
        let params = json!({
            "room":fixture.room.0.to_string(),"as":orch.0.to_string(),
            "to":["builder"],"text":"stop and do this","mode":"interrupt",
        });
        let response = send(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let message = sent_request(&response);
        assert_eq!(
            stop_keys(&fixture),
            [vec!["esc".to_owned()]],
            "{provider:?}"
        );
        assert!(fixture.request(message).queue_only, "{provider:?}");
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(
            fixture.request(lead).failure_reason.as_deref(),
            Some("interrupted"),
            "{provider:?}"
        );
        fixture.deliver();
        assert_typed(&fixture, "stop and do this", false);
    }
}

#[test]
fn the_orchestrator_default_delivery_is_unchanged() {
    for provider in providers() {
        let mut fixture = fixture_for(provider);
        let orch = orchestrator(&mut fixture);
        let lead = running(&mut fixture, "write the parser", "1");
        let params = json!({
            "room":fixture.room.0.to_string(),"as":orch.0.to_string(),
            "to":["builder"],"text":"also add tests",
        });
        let response = send(&mut fixture, params);
        assert!(response.ok, "{provider:?}: {response:?}");
        let message = sent_request(&response);
        assert!(stop_keys(&fixture).is_empty(), "{provider:?}");
        fixture.deliver();
        assert_typed(&fixture, "also add tests", true);
        assert_eq!(fixture.request(message).group, Some(lead), "{provider:?}");
    }
}

#[test]
fn agent_authored_messages_never_interrupt() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let peer = peer(&mut fixture);
    let lead = running(&mut fixture, "write the parser", "1");
    let params = json!({
        "room":fixture.room.0.to_string(),"as":peer.0.to_string(),
        "to":["builder"],"text":"stop now","mode":"interrupt",
    });
    let refused = send(&mut fixture, params);
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("interrupt")),
        "{refused:?}"
    );
    assert!(stop_keys(&fixture).is_empty());
    assert_eq!(fixture.worker.state.queued_requests(fixture.agent), &[]);
    assert!(
        !fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending
    );

    // An ordinary agent keeps today's steer-or-queue delivery.
    let params = json!({
        "room":fixture.room.0.to_string(),"as":peer.0.to_string(),
        "to":["builder"],"text":"and tests",
    });
    let steered = send(&mut fixture, params);
    assert!(steered.ok, "{steered:?}");
    fixture.deliver();
    assert_eq!(
        fixture.request(sent_request(&steered)).group,
        Some(lead),
        "the default agent send must still steer"
    );
}

#[test]
fn developer_sends_cannot_be_weakened_to_steer_or_queue() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "write the parser", "1");
    for params in [
        json!({"room":fixture.room.0.to_string(),"to":["builder"],"text":"x","mode":"steer"}),
        json!({"room":fixture.room.0.to_string(),"to":["builder"],"text":"x","mode":"queue"}),
        json!({"room":fixture.room.0.to_string(),"to":["builder"],"text":"x","queue":true}),
    ] {
        let refused = send(&mut fixture, params.clone());
        assert!(!refused.ok, "{params}: {refused:?}");
        assert!(
            refused
                .error
                .as_ref()
                .is_some_and(|error| error.message.contains("interrupt")),
            "{params}: {refused:?}"
        );
    }
    assert!(stop_keys(&fixture).is_empty());
    assert_eq!(fixture.worker.state.requests().count(), 1, "only the lead");
    // The explicit stronger mode is allowed and does the same thing.
    let params =
        json!({"room":fixture.room.0.to_string(),"to":["builder"],"text":"go","mode":"interrupt"});
    let response = send(&mut fixture, params);
    assert!(response.ok, "{response:?}");
    assert_eq!(stop_keys(&fixture), [vec!["esc".to_owned()]]);
}

#[test]
fn an_unknown_delivery_mode_is_refused_without_sending() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "write the parser", "1");
    let params =
        json!({"room":fixture.room.0.to_string(),"to":["builder"],"text":"x","mode":"later"});
    let refused = send(&mut fixture, params);
    assert!(!refused.ok, "{refused:?}");
    assert!(
        refused
            .error
            .as_ref()
            .is_some_and(|error| error.message.contains("interrupt, steer or queue")),
        "{refused:?}"
    );
    assert_eq!(fixture.worker.state.requests().count(), 1);
}

#[test]
fn an_interrupt_send_stays_in_the_next_turn_after_a_restart() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "run the old plan", "1");
    let params = send_params(&fixture, "change course now");
    let response = send(&mut fixture, params);
    let message = sent_request(&response);
    // A restart reloads state.json: the mode must survive as queue_only so the
    // new coordinator never steers it into the turn the interrupt ended.
    let saved = JsonStore::new(fixture.dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.request(message).unwrap().phase, RequestPhase::Queued);
    assert!(
        saved.request(message).unwrap().queue_only,
        "the restarted coordinator must not steer it into the old turn"
    );
    assert_eq!(saved.next_steering(fixture.agent), None);
    // The live coordinator ends that turn; the queued message starts the next.
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.request(message).failure_reason,
        None,
        "the queued message is not part of the interrupted turn"
    );
    fixture.deliver();
    assert_typed(&fixture, "change course now", false);
}

#[test]
fn room_composer_developer_send_interrupts_the_running_turn() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "run the old plan", "1");
    let draft = Draft {
        text: "change course now".into(),
        files: Vec::new(),
        recipient_ids: [fixture.agent].into(),
    };
    let (events, _receiver) = std::sync::mpsc::channel();
    fixture
        .worker
        .command(
            crate::messaging::coordinator::BusCommand::Submit(fixture.room, draft),
            &events,
        )
        .unwrap();
    assert_eq!(
        stop_keys(&fixture),
        [vec!["esc".to_owned()]],
        "a developer message from the room composer must press the stop key"
    );
    assert!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending
    );
    let message = *fixture
        .worker
        .state
        .queued_requests(fixture.agent)
        .last()
        .unwrap();
    assert!(fixture.request(message).queue_only);
    assert_eq!(fixture.request(message).group, None);
    let typed = fixture.typed().len();
    fixture.deliver();
    assert_eq!(
        fixture.typed().len(),
        typed,
        "the composer message must not be typed into the dying turn"
    );
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
    fixture.deliver();
    assert_typed(&fixture, "change course now", false);
}

#[test]
fn the_room_composer_interrupts_the_running_turn_for_either_key() {
    // Enter and Option/Alt+Enter both send the developer's message; neither
    // key can weaken it back to steer or queue.
    for queued in [false, true] {
        for provider in providers() {
            let mut fixture = fixture_for(provider);
            let lead = running(&mut fixture, "run the old plan", "1");
            let draft = Draft {
                text: "change course now".into(),
                files: Vec::new(),
                recipient_ids: [fixture.agent].into(),
            };
            let (events, _receiver) = std::sync::mpsc::channel();
            let command = if queued {
                BusCommand::SubmitQueued(fixture.room, draft)
            } else {
                BusCommand::Submit(fixture.room, draft)
            };
            fixture.worker.command(command, &events).unwrap();
            assert_eq!(
                stop_keys(&fixture),
                [vec!["esc".to_owned()]],
                "{provider:?}"
            );
            assert!(
                fixture
                    .worker
                    .state
                    .agent(fixture.agent)
                    .unwrap()
                    .interrupt_pending,
                "{provider:?}"
            );
            let message = *fixture
                .worker
                .state
                .queued_requests(fixture.agent)
                .last()
                .unwrap();
            assert!(fixture.request(message).queue_only, "{provider:?}");
            assert_eq!(fixture.request(message).group, None, "{provider:?}");
            let typed = fixture.typed().len();
            fixture.deliver();
            assert_eq!(fixture.typed().len(), typed, "{provider:?}");
            fixture.status(RuntimeStatus::Idle);
            assert_eq!(
                fixture.request(lead).failure_reason.as_deref(),
                Some("interrupted"),
                "{provider:?}"
            );
            fixture.deliver();
            assert_typed(&fixture, "change course now", false);
        }
    }
}

#[test]
fn developer_send_keeps_interrupt_intent_and_reports_native_retirement_result() {
    let mut fixture = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut fixture, "old work", "1");
    let params = send_params(&fixture, "developer correction");
    let response = send(&mut fixture, params);
    assert!(response.ok, "{response:?}");
    let follower = sent_request(&response);
    assert!(fixture.request(follower).queue_only);
    assert_eq!(fixture.request(follower).group, None);
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[follower]
    );
    assert_eq!(stop_keys(&fixture), [vec!["esc".to_owned()]]);
    assert_eq!(
        response.result["interrupts"][0]["agent_id"],
        fixture.agent.0
    );
    assert_eq!(response.result["interrupts"][0]["outcome"], "interrupting");
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
}

fn second_running(f: &mut Fixture) -> AgentId {
    let mut state = f.worker.state.clone();
    let agent = state
        .create_agent(f.room, "second", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("second-launch".into()),
                terminal_id: Some("second-terminal".into()),
                pane_id: Some("second-pane".into()),
                session_id: Some("second-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    let request = state
        .submit_message_with(
            f.room,
            Draft {
                text: "second agent's work".into(),
                recipient_ids: [agent].into(),
                ..Draft::default()
            },
            Author::Developer,
            f.now,
            true,
        )
        .unwrap()[0];
    state.begin_submission(request, "second-launch", 0).unwrap();
    state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("second-session".into()),
                provider_turn_id: Some("turn-2".into()),
            },
        )
        .unwrap();
    state
        .observe_status(agent, RuntimeStatus::Working, f.now)
        .unwrap();
    f.worker.save(state).unwrap();
    agent
}

struct FailAtFirstStop(Arc<Mutex<Vec<Method>>>);

impl Transport for FailAtFirstStop {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::AgentSendKeys(_)) {
            self.0.lock().unwrap().push(method);
            panic!("simulated coordinator loss after first stop dispatch");
        }
        Native(self.0.clone()).request(method)
    }
}

fn reopen_after_interrupt(f: &mut Fixture) {
    let scratch = Worker::open(f.dir.join("scratch"), Box::new(Native(f.native.clone()))).unwrap();
    drop(std::mem::replace(&mut f.worker, scratch));
    f.worker = Worker::open(f.dir.clone(), Box::new(Native(f.native.clone()))).unwrap();
}

#[test]
fn multi_recipient_interrupt_replays_only_recipients_not_yet_dispatched() {
    for composer in [false, true] {
        let mut f = Fixture::new(Provider::Codex);
        running(&mut f, "old work", "turn-1");
        let second = second_running(&mut f);
        f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if composer {
                let draft = Draft {
                    text: "new work".into(),
                    recipient_ids: [f.agent, second].into(),
                    ..Draft::default()
                };
                f.worker
                    .command(BusCommand::Submit(f.room, draft), &mpsc::channel().0)
                    .unwrap();
            } else {
                let params = json!({"room":f.room.0.to_string(),"to":["builder","second"],"text":"new work"});
                assert!(send(&mut f, params).ok);
            }
        }));
        assert!(crashed.is_err());
        reopen_after_interrupt(&mut f);
        // The normal tick refreshes runtime status before replay.
        let mut polled = f.worker.state.clone();
        for recipient in [f.agent, second] {
            polled
                .observe_status(recipient, RuntimeStatus::Working, f.now)
                .unwrap();
        }
        f.worker.save(polled).unwrap();
        f.worker.replay_queued_send_interrupts(f.now).unwrap();
        // A fresh send only queues when the original turn has an ambiguous stop claim.
        let params = json!({
            "room": f.room.0.to_string(), "to": ["builder"], "text": "another send"
        });
        let response = send(&mut f, params);
        assert!(response.ok, "{response:?}");
        assert_eq!(response.result["interrupts"][0]["outcome"], "unknown");
        let calls = f.native.lock().unwrap();
        let panes: Vec<_> = calls
            .iter()
            .filter_map(|method| match method {
                Method::AgentSendKeys(params) => Some(params.target.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(panes, ["pane", "second-pane"], "composer={composer}");
    }
}

#[test]
fn legacy_composer_journal_replays_the_send_without_issuing_a_second_stop() {
    let mut f = Fixture::new(Provider::Codex);
    running(&mut f, "old work", "turn-1");
    let legacy = serde_json::from_value(json!({
        "op":"submit_draft", "room":f.room, "draft":{
            "text":"already interrupted", "recipient_ids":[f.agent], "files":[]
        }, "queued":true, "now_ms":f.now
    }))
    .unwrap();
    f.worker.store.journal(legacy).unwrap();
    let durable = f.worker.store.load().unwrap().unwrap();
    let follower = durable.queued_requests(f.agent)[0];
    assert!(
        durable.request(follower).unwrap().send_interrupt.is_none(),
        "the old operation must not recreate a stop intent"
    );
    reopen_after_interrupt(&mut f);
    f.worker.replay_queued_send_interrupts(f.now).unwrap();
    assert_eq!(f.worker.state.queued_requests(f.agent).len(), 1);
    assert!(
        stop_keys(&f).is_empty(),
        "a legacy record may already have stopped the turn"
    );
}

#[test]
fn room_composer_keeps_per_recipient_stop_outcomes_without_resubmitting() {
    let mut f = Fixture::new(Provider::Codex);
    running(&mut f, "old work", "turn-1");
    let second = second_running(&mut f);
    let mut state = f.worker.state.clone();
    state
        .observe_status(second, RuntimeStatus::Unavailable, f.now)
        .unwrap();
    f.worker.save(state).unwrap();
    let draft = Draft {
        text: "new work".into(),
        recipient_ids: [f.agent, second].into(),
        ..Draft::default()
    };
    f.worker
        .command(BusCommand::Submit(f.room, draft), &mpsc::channel().0)
        .unwrap();
    let first = f.worker.state.queued_requests(f.agent)[0];
    let prompt = f.request(first).prompt.id;
    let status = f.worker.control_message(prompt).unwrap();
    assert_eq!(
        status["requests"][0]["interrupt"]["outcome"],
        "interrupting"
    );
    assert_eq!(status["requests"][1]["interrupt"]["outcome"], "failed");
    assert_eq!(f.worker.state.queued_requests(f.agent).len(), 1);
    assert_eq!(f.worker.state.queued_requests(second).len(), 1);
}

#[test]
fn composer_acknowledges_a_durable_send_when_its_stop_claim_write_fails() {
    let mut f = Fixture::new(Provider::Codex);
    running(&mut f, "old work", "turn-1");
    f.worker.store.fail_once_at_after(
        crate::messaging::storage::state_store::SaveStage::TempSync,
        1,
    );
    let draft = Draft {
        text: "new work".into(),
        recipient_ids: [f.agent].into(),
        ..Draft::default()
    };
    f.worker
        .command(BusCommand::Submit(f.room, draft), &mpsc::channel().0)
        .unwrap();
    let durable = f.worker.store.load().unwrap().unwrap();
    let [queued] = durable.queued_requests(f.agent) else {
        panic!("expected one durable follower")
    };
    assert!(durable.request(*queued).unwrap().send_interrupt.is_some());
    assert!(stop_keys(&f).is_empty());
}

#[test]
fn queued_cli_receipt_survives_stop_storage_errors() {
    use crate::messaging::storage::state_store::SaveStage;

    for (skip, expected_keys, outcome) in [(1, 0, "pending"), (2, 1, "unknown")] {
        let mut f = Fixture::new(Provider::Codex);
        running(&mut f, "old work", "turn-1");
        f.worker.store.fail_once_at_after(SaveStage::TempSync, skip);
        let params = send_params(&f, "only one follower");
        let response = send(&mut f, params);
        assert!(response.ok, "{response:?}");
        let request = sent_request(&response);
        assert_eq!(response.result["stage"], "queued");
        assert_eq!(response.result["interrupts"][0]["agent_id"], json!(f.agent));
        assert_eq!(response.result["interrupts"][0]["outcome"], outcome);
        let saved = f.worker.store.load().unwrap().unwrap();
        assert_eq!(saved.queued_requests(f.agent), &[request]);
        let row = saved.request(request).unwrap();
        assert_eq!(response.result["message_id"], json!(row.prompt.id));
        assert!(row.latency.queued_at_ms.is_some());
        assert_eq!(stop_keys(&f).len(), expected_keys);
    }
}

#[test]
fn queued_stop_replays_after_the_original_turn_binds() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = f.send("original input", true);
    f.deliver();
    f.status(RuntimeStatus::Working);
    let follower = f.send("queued next", true);
    let mut state = f.worker.state.clone();
    let intent = state.send_interrupt_target(f.agent).unwrap();
    assert_eq!(intent.target, Some(lead));
    assert!(intent.provider_turn_id.is_none());
    state.stage_send_interrupt(follower, intent).unwrap();
    f.worker.save(state).unwrap();

    reopen_after_interrupt(&mut f);
    f.status(RuntimeStatus::Working);
    f.started("turn-1", "original input");
    f.consume();
    assert_eq!(f.request(lead).provider_turn_id.as_deref(), Some("turn-1"));
    f.worker.replay_queued_send_interrupts(f.now).unwrap();
    assert_eq!(stop_keys(&f), [vec!["esc".to_string()]]);
}

#[test]
fn definite_claude_prewrite_refusal_releases_the_queued_stop_claim() {
    let mut f = Fixture::new(Provider::ClaudeDs);
    let lead = running(&mut f, "old work", "turn-1");
    let mut state = f.worker.state.clone();
    let bound = state.agent(f.agent).unwrap().runtime_identity.clone();
    let mut unbound = bound.clone();
    unbound.session_id = None;
    state.set_agent_runtime_identity(f.agent, unbound).unwrap();
    f.worker.save(state).unwrap();
    let params = send_params(&f, "first follower");
    let response = send(&mut f, params);
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result["interrupts"][0]["outcome"], "failed");
    assert!(stop_keys(&f).is_empty());
    let saved = f.worker.store.load().unwrap().unwrap();
    assert!(saved.request(lead).unwrap().send_interrupt_claim.is_none());

    let mut state = f.worker.state.clone();
    state.set_agent_runtime_identity(f.agent, bound).unwrap();
    f.worker.save(state).unwrap();
    let params = send_params(&f, "second follower");
    assert!(send(&mut f, params).ok);
    assert_eq!(stop_keys(&f), [vec!["esc".to_string()]]);
}

fn autonomous_started(f: &mut Fixture, agent: AgentId, turn: &str) {
    autonomous_started_with_prompt(f, agent, turn, "native work");
}

fn autonomous_started_with_prompt(f: &mut Fixture, agent: AgentId, turn: &str, prompt: &str) {
    let provider = f.worker.state.agent(agent).unwrap().provider;
    let directory = f.dir.join("callbacks/autonomous-launch");
    callbacks::initialize(
        &directory,
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(agent.0),
            provider,
            launch_id: "autonomous-launch".into(),
        },
    )
    .unwrap();
    callbacks::append(
        &directory,
        "autonomous-launch",
        provider,
        json!({
            "hook_event_name": "beforeSubmitPrompt", "conversation_id": "autonomous-session",
            "generation_id": turn, "prompt": prompt
        }),
        &[],
        false,
    )
    .unwrap();
    f.worker.consume_callbacks(agent, &directory).unwrap();
}

fn add_autonomous(f: &mut Fixture, authenticated: bool) -> AgentId {
    let mut state = f.worker.state.clone();
    let id = state
        .create_agent(f.room, "autonomous", Provider::Cursor, f.dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            id,
            AgentRuntimeIdentity {
                launch_id: Some("autonomous-launch".into()),
                terminal_id: Some("autonomous-terminal".into()),
                pane_id: Some("autonomous-pane".into()),
                session_id: Some("autonomous-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(id).unwrap();
    state
        .observe_status(id, RuntimeStatus::Working, f.now)
        .unwrap();
    state.observe_stop_activity(id, Some((4, 10)));
    f.worker.save(state).unwrap();
    if authenticated {
        autonomous_started(f, id, "autonomous-turn");
    }
    id
}

#[test]
fn autonomous_follower_has_a_durable_stop_intent_before_earlier_recipient_dispatch() {
    for (scenario, activity, new_turn, stale_at_admission) in [
        ("same", Some((4, 10)), false, false),
        ("new server", Some((5, 10)), false, false),
        ("new state", Some((4, 11)), false, false),
        ("new turn", Some((4, 10)), true, false),
        ("missing evidence", None, false, false),
        ("stale at admission", Some((4, 11)), false, true),
    ] {
        let mut f = Fixture::new(Provider::Codex);
        running(&mut f, "old work", "turn-1");
        let autonomous = add_autonomous(&mut f, true);
        if stale_at_admission {
            let mut state = f.worker.state.clone();
            state.observe_stop_activity(autonomous, activity);
            f.worker.save(state).unwrap();
        }
        f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
        let params =
            json!({"room":f.room.0.to_string(),"to":["builder","autonomous"],"text":"next"});
        let crashed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(&mut f, params)));
        assert!(crashed.is_err());
        let saved = f.worker.store.load().unwrap().unwrap();
        let follower = saved.queued_requests(autonomous)[0];
        let intent = saved
            .request(follower)
            .unwrap()
            .send_interrupt
            .as_ref()
            .unwrap();
        assert!(intent.target.is_none());
        assert_eq!(intent.provider_turn_id.as_deref(), Some("autonomous-turn"));
        assert_eq!(intent.activity, (!stale_at_admission).then_some((4, 10)));

        reopen_after_interrupt(&mut f);
        let mut state = f.worker.state.clone();
        state
            .observe_status(autonomous, RuntimeStatus::Working, f.now)
            .unwrap();
        state.observe_stop_activity(autonomous, activity);
        f.worker.save(state).unwrap();
        if new_turn {
            autonomous_started(&mut f, autonomous, "new-autonomous-turn");
        }
        f.worker.replay_queued_send_interrupts(f.now).unwrap();
        f.worker.replay_queued_send_interrupts(f.now).unwrap();
        let native = f.native.lock().unwrap();
        let stops: Vec<_> = native
            .iter()
            .filter_map(|method| match method {
                Method::AgentSendKeys(params) if params.target == "autonomous-pane" => Some(params),
                _ => None,
            })
            .collect();
        assert_eq!(stops.len(), usize::from(scenario == "same"), "{scenario}");
        if let Some(stop) = stops.first() {
            assert_eq!(
                stop.expected_terminal_id.as_deref(),
                Some("autonomous-terminal")
            );
            assert_eq!(stop.expected_activity.unwrap().state_change_seq, 10);
        }
        if scenario == "missing evidence" || stale_at_admission {
            let outcome = f.request(follower).send_interrupt_outcome.as_ref().unwrap();
            assert_eq!(
                outcome.reason.as_deref(),
                Some("The autonomous turn cannot be revalidated")
            );
        }
        assert_eq!(f.worker.state.queued_requests(autonomous), &[follower]);
    }
}

#[test]
fn autonomous_ambiguous_stop_claim_survives_coordinator_reopen() {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = add_autonomous(&mut f, true);
    f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"first"});
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(&mut f, params)));
    assert!(crashed.is_err());
    assert_eq!(stop_keys(&f).len(), 1);

    reopen_after_interrupt(&mut f);
    let mut state = f.worker.state.clone();
    state
        .observe_status(autonomous, RuntimeStatus::Working, f.now)
        .unwrap();
    state.observe_stop_activity(autonomous, Some((4, 10)));
    f.worker.save(state).unwrap();
    f.worker.replay_queued_send_interrupts(f.now).unwrap();
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"second"});
    let result = send(&mut f, params);
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["interrupts"][0]["outcome"], "unknown");
    assert_eq!(stop_keys(&f).len(), 1);
    assert_eq!(f.worker.state.queued_requests(autonomous).len(), 2);
}

struct AutonomousPoll(Arc<Mutex<Vec<Method>>>, schema::AgentInfo);

impl Transport for AutonomousPoll {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::AgentListWithDetectionContext(_)) {
            return Ok(ResponseResult::AgentList {
                agents: vec![self.1.clone()],
            });
        }
        Native(self.0.clone()).request(method)
    }
}

fn poll_autonomous(f: &mut Fixture, status: &str, seq: u64, skipped: bool, dialog: Option<&str>) {
    let info = serde_json::from_value(json!({
        "generation":4, "state_change_seq":seq, "terminal_id":"autonomous-terminal",
        "pane_id":"autonomous-pane", "agent":"cursor", "agent_status":status,
        "screen_detection_skipped":skipped, "dialog_id":dialog, "native_ready":true,
        "interactive_ready":true, "workspace_id":"w", "tab_id":"t",
        "focused":false, "revision":seq,
        "agent_session":{"source":"bus","agent":"cursor","kind":"id","value":"autonomous-session"}
    }))
    .unwrap();
    f.worker.transport = Box::new(AutonomousPoll(f.native.clone(), info));
    f.worker.poll().unwrap();
}

#[test]
fn verified_idle_retires_only_the_previous_unbound_autonomous_stop() {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = add_autonomous(&mut f, false);
    f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"first"});
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(&mut f, params)));
    assert!(crash.is_err());
    assert_eq!(stop_keys(&f).len(), 1);

    reopen_after_interrupt(&mut f);
    assert_eq!(
        f.worker.state.agent(autonomous).unwrap().status,
        RuntimeStatus::Unavailable
    );
    poll_autonomous(&mut f, "working", 10, false, None);
    autonomous_started(&mut f, autonomous, "late-first-start");
    for (status, skipped, dialog) in [
        ("working", false, None),
        ("unknown", true, None),
        ("idle", true, None),
        ("idle", false, Some("approval")),
    ] {
        poll_autonomous(&mut f, status, 10, skipped, dialog);
        poll_autonomous(&mut f, "working", 10, false, None);
        let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"same turn"});
        let result = send(&mut f, params);
        assert!(result.ok, "{result:?}");
        assert_eq!(result.result["interrupts"][0]["outcome"], "unknown");
        assert_eq!(stop_keys(&f).len(), 1);
    }

    poll_autonomous(&mut f, "idle", 11, false, None);
    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 12, false, None);
    autonomous_started(&mut f, autonomous, "later-turn");
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"later send"});
    let result = send(&mut f, params);
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["stage"], "queued");
    assert_eq!(result.result["interrupts"][0]["outcome"], "interrupting");
    assert_eq!(
        stop_keys(&f),
        [vec!["ctrl+c".to_string()], vec!["ctrl+c".to_string()]]
    );
    let follower = sent_request(&result);
    assert_eq!(
        f.worker.state.queued_requests(autonomous).last(),
        Some(&follower)
    );
    let native = f.native.lock().unwrap();
    let last = native
        .iter()
        .rev()
        .find_map(|method| match method {
            Method::AgentSendKeys(params) => Some(params),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        last.expected_terminal_id.as_deref(),
        Some("autonomous-terminal")
    );
    assert_eq!(last.expected_activity.unwrap().state_change_seq, 12);
}

fn interrupt_bound_autonomous(f: &mut Fixture, lose_receipt: bool) -> AgentId {
    let autonomous = add_autonomous(f, true);
    if lose_receipt {
        f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
    }
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"first"});
    if lose_receipt {
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(f, params)));
        assert!(crash.is_err());
        reopen_after_interrupt(f);
    } else {
        let result = send(f, params);
        assert!(result.ok, "{result:?}");
        assert_eq!(result.result["interrupts"][0]["outcome"], "interrupting");
    }
    assert_eq!(stop_keys(f).len(), 1);
    autonomous
}

fn assert_later_autonomous_send(f: &mut Fixture, autonomous: AgentId) {
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"later send"});
    let result = send(f, params);
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["stage"], "queued");
    assert_eq!(result.result["interrupts"][0]["outcome"], "interrupting");
    assert_eq!(
        f.worker.state.queued_requests(autonomous).last(),
        Some(&sent_request(&result))
    );
    let native = f.native.lock().unwrap();
    let stops: Vec<_> = native
        .iter()
        .filter_map(|method| match method {
            Method::AgentSendKeys(params) => Some(params),
            _ => None,
        })
        .collect();
    assert_eq!(stops.len(), 2);
    assert_eq!(stops[1].target, "autonomous-pane");
    assert_eq!(
        stops[1].expected_terminal_id.as_deref(),
        Some("autonomous-terminal")
    );
}

#[test]
fn later_autonomous_episode_stops_after_acknowledged_interrupt_without_final() {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = interrupt_bound_autonomous(&mut f, false);
    poll_autonomous(&mut f, "idle", 11, false, None);
    poll_autonomous(&mut f, "working", 12, false, None);
    assert_later_autonomous_send(&mut f, autonomous);
}

#[test]
fn later_autonomous_episode_stops_after_lost_receipt_without_final() {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = interrupt_bound_autonomous(&mut f, true);
    poll_autonomous(&mut f, "idle", 11, false, None);
    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 12, false, None);
    assert_later_autonomous_send(&mut f, autonomous);
}

#[test]
fn later_autonomous_episode_stops_after_an_intervening_bus_request() {
    for lose_receipt in [false, true] {
        let mut f = Fixture::new(Provider::Codex);
        let autonomous = interrupt_bound_autonomous(&mut f, lose_receipt);
        poll_autonomous(&mut f, "idle", 11, false, None);
        let mut state = f.worker.state.clone();
        let queued = state.queued_requests(autonomous)[0];
        state
            .begin_submission(queued, "autonomous-launch", 1)
            .unwrap();
        state
            .record_submission(
                queued,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("autonomous-session".into()),
                    provider_turn_id: None,
                },
            )
            .unwrap();
        assert_eq!(
            state.close_interrupted(autonomous, f.now).unwrap(),
            [queued]
        );
        f.worker.save(state).unwrap();
        poll_autonomous(&mut f, "working", 12, false, None);
        assert_later_autonomous_send(&mut f, autonomous);
    }
}

#[test]
fn authenticated_autonomous_claim_deduplicates_if_its_turn_reappears_after_idle() {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = interrupt_bound_autonomous(&mut f, true);
    poll_autonomous(&mut f, "idle", 11, false, None);
    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 12, false, None);
    let mut state = f.worker.state.clone();
    state.observe_autonomous_turn(autonomous, "autonomous-turn".into(), true);
    f.worker.save(state).unwrap();
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"same turn"});
    let result = send(&mut f, params);
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["stage"], "queued");
    assert_eq!(result.result["interrupts"][0]["outcome"], "unknown");
    assert_eq!(stop_keys(&f).len(), 1);
}

struct DeferredBusyJoin {
    native: Arc<Mutex<Vec<Method>>>,
    receipt: Option<mpsc::Receiver<NativeResult>>,
}

impl Transport for DeferredBusyJoin {
    fn request(&mut self, method: Method) -> NativeResult {
        Native(self.native.clone()).request(method)
    }

    fn submit(&mut self, method: Method) -> NativeCall {
        self.native.lock().unwrap().push(method);
        NativeCall::Pending(self.receipt.take().unwrap())
    }
}

fn claimed_busy_join() -> (Fixture, AgentId, RequestId, SendInterrupt) {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = add_autonomous(&mut f, true);
    let mut state = f.worker.state.clone();
    let original = state.send_interrupt_target(autonomous).unwrap();
    let head = state
        .submit_message_with(
            f.room,
            Draft {
                text: "join the running turn".into(),
                recipient_ids: [autonomous].into(),
                ..Draft::default()
            },
            Author::Developer,
            f.now,
            false,
        )
        .unwrap()[0];
    f.worker.save(state).unwrap();
    f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"stop"});
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(&mut f, params)));
    assert!(crash.is_err());
    assert_eq!(stop_keys(&f).len(), 1);
    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 10, false, None);
    assert!(saved_autonomous_claim(&f, autonomous, &original));
    (f, autonomous, head, original)
}

fn saved_autonomous_claim(f: &Fixture, autonomous: AgentId, original: &SendInterrupt) -> bool {
    f.worker
        .store
        .load()
        .unwrap()
        .unwrap()
        .send_interrupt_already_claimed(autonomous, original)
}

fn defer_busy_join(f: &mut Fixture, head: RequestId) -> (mpsc::Sender<NativeResult>, Method) {
    let (reply, receipt) = mpsc::channel();
    f.worker.transport = Box::new(DeferredBusyJoin {
        native: f.native.clone(),
        receipt: Some(receipt),
    });
    f.deliver();
    assert_eq!(f.request(head).phase, RequestPhase::Submitting);
    let method = f.native.lock().unwrap().last().unwrap().clone();
    match &method {
        Method::AgentPromptIfIdle(params) | Method::AgentPromptIfIdleAtGeneration(params) => {
            assert!(params.steer);
            assert_eq!(params.expected_terminal_id, "autonomous-terminal");
        }
        _ => panic!("expected a native busy join, got {method:?}"),
    }
    (reply, method)
}

fn busy_join_error(code: Option<&str>, definitely_rejected: bool) -> NativeResult {
    Err(TransportError {
        code: code.map(str::to_owned),
        message: "simulated busy-join outcome".into(),
        definitely_rejected,
        timestamps: Default::default(),
    })
}

fn refused_busy_join_preserves_autonomous_stop(code: Option<&str>) {
    let (mut f, autonomous, head, original) = claimed_busy_join();
    let before = f.worker.state.queued_requests(autonomous).to_vec();
    assert_eq!(before[0], head);
    let (reply, _) = defer_busy_join(&mut f, head);
    let saved_while_pending = saved_autonomous_claim(&f, autonomous, &original);
    reply.send(busy_join_error(code, true)).unwrap();
    f.worker.finish_native_submissions(false);
    assert_eq!(f.request(head).phase, RequestPhase::Queued);
    assert_eq!(f.worker.state.queued_requests(autonomous), before);
    assert_eq!(
        f.worker.state.agent(autonomous).unwrap().current_request,
        None
    );
    let saved_after_requeue = saved_autonomous_claim(&f, autonomous, &original);

    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 10, false, None);
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"still working"});
    let result = send(&mut f, params);
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["stage"], "queued");
    assert_eq!(
        (
            result.result["interrupts"][0]["outcome"].as_str(),
            stop_keys(&f).len()
        ),
        (Some("unknown"), 1)
    );
    assert!(saved_while_pending && saved_after_requeue);
}

#[test]
fn definite_busy_join_refusal_preserves_the_autonomous_stop_claim() {
    refused_busy_join_preserves_autonomous_stop(None);
}

#[test]
fn composer_busy_join_preserves_the_autonomous_stop_claim() {
    refused_busy_join_preserves_autonomous_stop(Some(crate::protocol::api::AGENT_COMPOSER_BUSY));
}

#[test]
fn busy_join_clears_the_autonomous_claim_only_after_positive_ownership() {
    for (native_confirmed, receipt_first) in [(true, true), (false, false), (false, true)] {
        let (mut f, autonomous, head, original) = claimed_busy_join();
        let (reply, method) = defer_busy_join(&mut f, head);
        assert!(saved_autonomous_claim(&f, autonomous, &original));
        let payload = match &method {
            Method::AgentPromptIfIdle(params) | Method::AgentPromptIfIdleAtGeneration(params) => {
                params.text.clone()
            }
            _ => unreachable!(),
        };
        let receipt = if native_confirmed {
            Native(Arc::default()).request(method)
        } else {
            busy_join_error(None, false)
        };
        if receipt_first {
            reply.send(receipt).unwrap();
            f.worker.finish_native_submissions(false);
            assert_eq!(
                saved_autonomous_claim(&f, autonomous, &original),
                !native_confirmed
            );
        }
        autonomous_started_with_prompt(&mut f, autonomous, "autonomous-turn", &payload);
        assert!(f.request(head).trusted_start_bound);
        assert_eq!(f.request(head).phase, RequestPhase::Active);
        assert!(!saved_autonomous_claim(&f, autonomous, &original));
        if !receipt_first {
            reply.send(busy_join_error(None, true)).unwrap();
            f.worker.finish_native_submissions(false);
            assert_eq!(f.request(head).phase, RequestPhase::Active);
            assert!(!saved_autonomous_claim(&f, autonomous, &original));
        }
    }
}
