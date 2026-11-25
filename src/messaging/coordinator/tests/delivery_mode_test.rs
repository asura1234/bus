//! `bus send --mode`: who may choose how a message reaches a running turn.
use super::*;
use crate::messaging::{
    control::Response,
    model::{SendInterrupt, SendMode},
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

fn assert_interrupted_lead(fixture: &Fixture, lead: RequestId, replacement: RequestId) {
    let row = fixture.request(lead);
    if fixture.provider.uses_claude_harness() {
        assert_eq!(row.phase, RequestPhase::Queued);
        assert_eq!(row.interrupt_replay_before, Some(replacement));
        assert!(row.failure_reason.is_none());
    } else {
        assert_eq!(row.failure_reason.as_deref(), Some("interrupted"));
    }
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
        assert_interrupted_lead(&fixture, lead, message);
        assert!(
            !fixture
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
        assert!(queued.interrupt_delivery, "{provider:?}");
        assert!(!queued.steered, "{provider:?}");
        assert_eq!(queued.group, None, "{provider:?}");

        // Interrupt-mode sends deliver immediately without waiting for Idle.
        fixture.deliver();
        assert_typed(&fixture, "change course now", false);
        assert_eq!(
            fixture.request(message).group,
            provider.uses_claude_harness().then_some(lead),
            "{provider:?}"
        );
    }
}

/// The old execution cannot answer the requests replayed into a new turn.
#[test]
fn an_interrupted_claude_batch_settles_from_its_new_final() {
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
    assert_eq!(fixture.reply(lead).as_deref(), Some("new plan done"));
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
    let lead = running(&mut fixture, "run the old plan", "1");
    let follower = fixture.send("earlier queued work", true);
    let params = send_params(&fixture, "change course now");
    let response = send(&mut fixture, params);
    let message = sent_request(&response);
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[follower, lead, message]
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
        let saved = fixture.worker.store.load().unwrap().unwrap();
        assert_eq!(
            saved.request(message).unwrap().send_mode,
            Some(SendMode::Steer)
        );
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
        let saved = fixture.worker.store.load().unwrap().unwrap();
        assert_eq!(
            saved.request(message).unwrap().send_mode,
            Some(SendMode::Queue)
        );
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
        assert_interrupted_lead(&fixture, lead, message);
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
    assert_eq!(
        saved.request(message).unwrap().send_mode,
        Some(SendMode::Interrupt)
    );
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
    let message = *fixture
        .worker
        .state
        .queued_requests(fixture.agent)
        .last()
        .unwrap();
    assert_interrupted_lead(&fixture, lead, message);
    assert!(fixture.request(message).queue_only);
    assert!(fixture.request(message).interrupt_delivery);
    assert_eq!(fixture.request(message).group, None);
    fixture.deliver();
    assert_typed(&fixture, "change course now", false);
}

#[test]
fn composer_modes_steer_or_wait_without_stopping_a_running_turn() {
    for mode in [SendMode::Steer, SendMode::Queue] {
        let mut fixture = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut fixture, "write the parser", "1");
        let draft = Draft {
            text: "also add tests".into(),
            recipient_ids: [fixture.agent].into(),
            ..Draft::default()
        };
        fixture
            .worker
            .command(
                BusCommand::SubmitWithMode(fixture.room, draft, mode),
                &mpsc::channel().0,
            )
            .unwrap();
        assert!(stop_keys(&fixture).is_empty(), "{mode:?}");
        let [request] = fixture.worker.state.queued_requests(fixture.agent) else {
            panic!("expected a single composer request for {mode:?}");
        };
        let request = *request;
        let persisted = fixture.worker.store.load().unwrap().unwrap();
        let persisted = persisted.request(request).unwrap();
        assert_eq!(persisted.queue_only, mode == SendMode::Queue);
        assert_eq!(persisted.history_send_mode(), Some(mode));

        fixture.deliver();
        if mode == SendMode::Steer {
            assert_typed(&fixture, "also add tests", true);
            assert_eq!(fixture.request(request).group, Some(lead));
        } else {
            assert_eq!(fixture.typed().len(), 1, "queue must wait for the lead");
            assert_eq!(fixture.request(request).group, None);
            fixture.stopped("1", "parser written");
            fixture.consume();
            fixture.status(RuntimeStatus::Idle);
            fixture.deliver();
            assert_typed(&fixture, "also add tests", false);
        }
    }
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
            let message = *fixture
                .worker
                .state
                .queued_requests(fixture.agent)
                .last()
                .unwrap();
            assert_interrupted_lead(&fixture, lead, message);
            assert!(fixture.request(message).queue_only, "{provider:?}");
            assert!(fixture.request(message).interrupt_delivery, "{provider:?}");
            assert_eq!(fixture.request(message).group, None, "{provider:?}");
            fixture.deliver();
            assert_typed(&fixture, "change course now", false);
        }
    }
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
fn claude_reconciles_a_lost_stop_receipt_without_pressing_escape_again() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut f, "old work", "old-turn");
    f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
    let params = send_params(&f, "next work");
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(&mut f, params)));
    assert!(crashed.is_err());

    reopen_after_interrupt(&mut f);
    f.status(RuntimeStatus::Working);
    let replacement = f.worker.state.queued_requests(f.agent)[0];
    f.worker.replay_queued_send_interrupts(f.now).unwrap();
    assert_eq!(stop_keys(&f), [vec!["esc".to_owned()]]);
    assert_eq!(
        f.worker.state.queued_requests(f.agent),
        &[lead, replacement]
    );

    f.status(RuntimeStatus::Idle);
    f.deliver();
    let (payload, _) = last_typed(&f);
    assert!(payload.contains("old work"), "{payload:?}");
    assert!(payload.ends_with("next work"), "{payload:?}");
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
    assert_eq!(
        durable.request(*queued).unwrap().send_mode,
        Some(SendMode::Interrupt)
    );
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
    state.observe_autonomous_turn(autonomous, "autonomous-turn".into(), true, false);
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
    busy_join_with_autonomous_claim(true)
}

fn busy_join_with_autonomous_claim(
    authenticated: bool,
) -> (Fixture, AgentId, RequestId, SendInterrupt) {
    let mut f = Fixture::new(Provider::Codex);
    let autonomous = add_autonomous(&mut f, authenticated);
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
    if !authenticated {
        callbacks::initialize(
            &f.dir.join("callbacks/autonomous-launch"),
            &callbacks::Manifest {
                routing_key: callbacks::RoutingKey(autonomous.0),
                provider: Provider::Cursor,
                launch_id: "autonomous-launch".into(),
            },
        )
        .unwrap();
    }
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

fn assert_provisional_stop_was_deduplicated(
    state: &crate::messaging::model::BusState,
    recipient: AgentId,
) {
    let queued = *state.queued_requests(recipient).last().unwrap();
    let request = state.request(queued).unwrap();
    assert_eq!(request.phase, RequestPhase::Queued);
    assert!(
        request.send_interrupt.is_none(),
        "the stop outcome is settled"
    );
    let outcome = request.send_interrupt_outcome.as_ref().unwrap();
    assert_eq!(outcome.outcome, "unknown");
    assert_eq!(
        outcome.reason.as_deref(),
        Some("The prior stop may have reached the terminal")
    );
}

fn recorded_stop_terminals(f: &Fixture) -> Vec<String> {
    f.native
        .lock()
        .unwrap()
        .iter()
        .filter_map(|method| match method {
            Method::AgentSendKeys(params) => params.expected_terminal_id.clone(),
            _ => None,
        })
        .collect()
}

fn pending_busy_join_deduplicates_stop(composer: bool) {
    let (mut f, autonomous, head, original) = claimed_busy_join();
    let other = second_running(&mut f);
    let (reply, _) = defer_busy_join(&mut f, head);
    if composer {
        let draft = Draft {
            text: "queue for both recipients".into(),
            recipient_ids: [autonomous, other].into(),
            ..Draft::default()
        };
        f.worker
            .command(BusCommand::Submit(f.room, draft), &mpsc::channel().0)
            .unwrap();
    } else {
        let params = json!({
            "room": f.room.0.to_string(), "to": ["autonomous", "second"], "text": "queue for both"
        });
        let result = send(&mut f, params);
        assert!(result.ok, "{result:?}");
        assert_eq!(result.result["stage"], "queued");
        assert_eq!(result.result["request_ids"].as_array().unwrap().len(), 2);
        assert_eq!(
            result.result["interrupts"][0]["agent_id"],
            json!(autonomous)
        );
        assert_eq!(result.result["interrupts"][0]["outcome"], "unknown");
        assert_eq!(result.result["interrupts"][1]["agent_id"], json!(other));
        assert_eq!(result.result["interrupts"][1]["outcome"], "interrupting");
    }
    assert_eq!(stop_keys(&f).len(), 2);
    assert_eq!(
        recorded_stop_terminals(&f),
        ["autonomous-terminal", "second-terminal"]
    );
    let saved = f.worker.store.load().unwrap().unwrap();
    assert_provisional_stop_was_deduplicated(&saved, autonomous);
    let [other_request] = saved.queued_requests(other) else {
        panic!("the unrelated recipient's request must remain queued");
    };
    assert_eq!(
        saved
            .request(*other_request)
            .unwrap()
            .send_interrupt_outcome
            .as_ref()
            .unwrap()
            .outcome,
        "interrupting"
    );
    assert_eq!(saved.agent(autonomous).unwrap().current_request, Some(head));
    assert_eq!(saved.request(head).unwrap().phase, RequestPhase::Submitting);
    assert!(saved.send_interrupt_already_claimed(autonomous, &original));

    reply.send(busy_join_error(None, true)).unwrap();
    f.worker.finish_native_submissions(false);
    assert_eq!(f.request(head).phase, RequestPhase::Queued);
    f.worker.replay_queued_send_interrupts(f.now).unwrap();
    assert_eq!(stop_keys(&f).len(), 2);
}

#[test]
fn pending_busy_join_deduplicates_cli_stop_without_blocking_other_recipients() {
    pending_busy_join_deduplicates_stop(false);
}

#[test]
fn pending_busy_join_deduplicates_composer_stop_without_blocking_other_recipients() {
    pending_busy_join_deduplicates_stop(true);
}

fn uncertain_busy_join(f: &mut Fixture, head: RequestId) -> String {
    let (reply, method) = defer_busy_join(f, head);
    reply.send(busy_join_error(None, false)).unwrap();
    f.worker.finish_native_submissions(false);
    assert!(f.request(head).uncertain_outcome);
    let (Method::AgentPromptIfIdle(params) | Method::AgentPromptIfIdleAtGeneration(params)) =
        method
    else {
        unreachable!("defer_busy_join verified the native method");
    };
    params.text
}

fn uncertain_busy_join_deduplicates_stop(reopen: bool) {
    let (mut f, autonomous, head, original) = claimed_busy_join();
    uncertain_busy_join(&mut f, head);
    if reopen {
        reopen_after_interrupt(&mut f);
        poll_autonomous(&mut f, "working", 10, false, None);
    }
    assert_eq!(
        f.worker.state.agent(autonomous).unwrap().current_request,
        Some(head)
    );
    assert_eq!(f.request(head).phase, RequestPhase::Submitting);
    assert!(f.request(head).uncertain_outcome);
    let params =
        json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"after uncertain join"});
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
    let saved = f.worker.store.load().unwrap().unwrap();
    assert_provisional_stop_was_deduplicated(&saved, autonomous);
    assert!(saved.send_interrupt_already_claimed(autonomous, &original));
}

#[test]
fn uncertain_busy_join_deduplicates_stop_after_the_native_receipt() {
    uncertain_busy_join_deduplicates_stop(false);
}

#[test]
fn uncertain_busy_join_deduplicates_stop_after_reopen() {
    uncertain_busy_join_deduplicates_stop(true);
}

#[test]
fn uncertain_busy_join_allows_a_stop_after_a_new_turn_binds() {
    let (mut f, autonomous, head, original) = claimed_busy_join();
    let payload = uncertain_busy_join(&mut f, head);
    autonomous_started_with_prompt(&mut f, autonomous, "new-authenticated-turn", &payload);
    assert_eq!(
        f.request(head).provider_turn_id.as_deref(),
        Some("new-authenticated-turn")
    );
    assert!(f.request(head).trusted_start_bound);
    assert!(!saved_autonomous_claim(&f, autonomous, &original));
    f.worker.replay_queued_send_interrupts(f.now).unwrap();
    assert_eq!(stop_keys(&f).len(), 1);
    assert_later_autonomous_send(&mut f, autonomous);
}

fn assert_head_stays_provisional(f: &Fixture, agent: AgentId, head: RequestId) {
    assert_eq!(
        f.worker.state.agent(agent).unwrap().current_request,
        Some(head)
    );
    let request = f.request(head);
    assert_eq!(request.phase, RequestPhase::Submitting);
    assert!(!request.trusted_start_bound);
    assert!(
        request.provider_turn_id.is_none(),
        "the provider turn does not own the Bus prompt"
    );
}

fn provider_started_during_provisional_join(uncertain: bool) {
    let (mut f, autonomous, head, original) = claimed_busy_join();
    let pending = if uncertain {
        uncertain_busy_join(&mut f, head);
        None
    } else {
        Some(defer_busy_join(&mut f, head).0)
    };
    autonomous_started_with_prompt(
        &mut f,
        autonomous,
        "autonomous-turn",
        "more of the same turn",
    );
    let params = json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"same episode"});
    let same_episode = send(&mut f, params);
    assert!(same_episode.ok, "{same_episode:?}");
    assert_eq!(same_episode.result["stage"], "queued");
    assert_eq!(same_episode.result["interrupts"][0]["outcome"], "unknown");
    assert_eq!(stop_keys(&f).len(), 1);

    autonomous_started_with_prompt(&mut f, autonomous, "provider-turn-b", "provider work B");
    if uncertain {
        reopen_after_interrupt(&mut f);
    }
    poll_autonomous(&mut f, "working", 11, false, None);
    assert_head_stays_provisional(&f, autonomous, head);
    assert_eq!(f.request(head).uncertain_outcome, uncertain);
    assert!(saved_autonomous_claim(&f, autonomous, &original));
    assert_later_autonomous_send(&mut f, autonomous);
    assert_head_stays_provisional(&f, autonomous, head);
    assert!(saved_autonomous_claim(&f, autonomous, &original));

    if let Some(reply) = pending {
        reply.send(busy_join_error(None, true)).unwrap();
        f.worker.finish_native_submissions(false);
        assert_eq!(f.request(head).phase, RequestPhase::Queued);
    }
}

#[test]
fn provider_started_while_join_is_in_flight_allows_one_pinned_stop() {
    provider_started_during_provisional_join(false);
}

#[test]
fn provider_started_after_uncertain_join_survives_reopen_and_allows_one_pinned_stop() {
    provider_started_during_provisional_join(true);
}

#[test]
fn provider_started_claim_deduplicates_its_turn_and_allows_the_next_foreign_turn() {
    let (mut f, autonomous, head, original) = claimed_busy_join();
    uncertain_busy_join(&mut f, head);
    autonomous_started_with_prompt(&mut f, autonomous, "provider-turn-b", "provider work B");
    poll_autonomous(&mut f, "working", 11, false, None);

    f.worker.transport = Box::new(FailAtFirstStop(f.native.clone()));
    let params = json!({"room": f.room.0.to_string(),"to":["autonomous"],"text":"stop B"});
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| send(&mut f, params)));
    assert!(
        crash.is_err(),
        "the new provider episode should reach native"
    );
    assert_eq!(stop_keys(&f).len(), 2);

    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 11, false, None);
    let params =
        json!({"room": f.room.0.to_string(),"to":["autonomous"],"text":"retry while B runs"});
    let duplicate = send(&mut f, params);
    assert!(duplicate.ok, "{duplicate:?}");
    assert_eq!(duplicate.result["stage"], "queued");
    assert_eq!(duplicate.result["interrupts"][0]["outcome"], "unknown");
    assert_eq!(stop_keys(&f).len(), 2);

    autonomous_started_with_prompt(&mut f, autonomous, "provider-turn-c", "provider work C");
    poll_autonomous(&mut f, "working", 12, false, None);
    let params = json!({"room": f.room.0.to_string(),"to":["autonomous"],"text":"stop C"});
    let next = send(&mut f, params);
    assert!(next.ok, "{next:?}");
    assert_eq!(next.result["stage"], "queued");
    assert_eq!(next.result["interrupts"][0]["outcome"], "interrupting");
    assert_eq!(stop_keys(&f).len(), 3);
    assert_eq!(recorded_stop_terminals(&f), ["autonomous-terminal"; 3]);
    assert_head_stays_provisional(&f, autonomous, head);
    assert!(saved_autonomous_claim(&f, autonomous, &original));
}

#[test]
fn provider_started_without_proof_does_not_advance_a_provisional_episode() {
    use crate::messaging::model::{CallbackEventKind, ProviderCallback};

    let (mut f, autonomous, head, original) = claimed_busy_join();
    let payload = uncertain_busy_join(&mut f, head);
    let boundary = f.request(head).submission_boundary.unwrap();
    let fresh = ProviderCallback {
        callback_id: "provider-start".into(),
        sequence: boundary + 1,
        occurred_at_ms: f.now,
        agent_id: autonomous,
        launch_id: "autonomous-launch".into(),
        provider_session_id: Some("autonomous-session".into()),
        provider_turn_id: Some("different-turn".into()),
        provider_prompt_id: None,
        prompt_payload: Some("different payload".into()),
        kind: CallbackEventKind::PromptStarted,
    };
    for callback in [
        ProviderCallback {
            provider_turn_id: None,
            ..fresh.clone()
        },
        ProviderCallback {
            provider_turn_id: Some(String::new()),
            ..fresh.clone()
        },
        ProviderCallback {
            prompt_payload: None,
            ..fresh.clone()
        },
        ProviderCallback {
            prompt_payload: Some(payload),
            ..fresh.clone()
        },
        ProviderCallback {
            provider_session_id: None,
            ..fresh.clone()
        },
        ProviderCallback {
            provider_session_id: Some("other-session".into()),
            ..fresh.clone()
        },
        ProviderCallback {
            launch_id: "other-launch".into(),
            ..fresh.clone()
        },
        ProviderCallback {
            sequence: boundary,
            ..fresh
        },
    ] {
        assert!(
            f.worker.state.autonomous_callback_turn(&callback).is_none(),
            "{callback:?}"
        );
        let params =
            json!({"room":f.room.0.to_string(),"to":["autonomous"],"text":"no new episode"});
        let result = send(&mut f, params);
        assert!(result.ok, "{result:?}");
        assert_eq!(result.result["stage"], "queued");
        assert_eq!(result.result["interrupts"][0]["outcome"], "unknown");
        assert_eq!(stop_keys(&f).len(), 1);
    }
    assert_head_stays_provisional(&f, autonomous, head);
    assert!(saved_autonomous_claim(&f, autonomous, &original));
}

#[test]
fn initially_unbound_stop_claim_only_covers_the_first_authenticated_episode() {
    let (mut f, autonomous, head, original) = busy_join_with_autonomous_claim(false);
    assert!(original.provider_turn_id.is_none());
    uncertain_busy_join(&mut f, head);

    for payload in ["first late start", "another start in the same turn"] {
        autonomous_started_with_prompt(&mut f, autonomous, "late-turn-a", payload);
        let params =
            json!({"room": f.room.0.to_string(), "to": ["autonomous"], "text": "same turn"});
        let send_result = send(&mut f, params);
        assert!(send_result.ok, "{send_result:?}");
        assert_eq!(send_result.result["stage"], "queued");
        assert_eq!(send_result.result["interrupts"][0]["outcome"], "unknown");
        assert_eq!(recorded_stop_terminals(&f), ["autonomous-terminal"]);
        assert!(saved_autonomous_claim(&f, autonomous, &original));
    }

    autonomous_started_with_prompt(&mut f, autonomous, "provider-turn-b", "provider work B");
    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 11, false, None);
    assert_head_stays_provisional(&f, autonomous, head);
    assert_later_autonomous_send(&mut f, autonomous);
    let scoped_claim = SendInterrupt {
        provider_turn_id: Some("late-turn-a".into()),
        ..original
    };
    assert!(saved_autonomous_claim(&f, autonomous, &scoped_claim));
    assert_head_stays_provisional(&f, autonomous, head);
}

fn requeued_join_keeps_the_foreign_stop_claim(code: Option<&str>) {
    let (mut f, autonomous, head, _) = claimed_busy_join();
    let (reply, _) = defer_busy_join(&mut f, head);
    autonomous_started_with_prompt(&mut f, autonomous, "provider-turn-b", "provider work B");
    poll_autonomous(&mut f, "working", 11, false, None);

    assert_later_autonomous_send(&mut f, autonomous);
    assert_eq!(
        f.request(head)
            .send_interrupt_claim
            .as_ref()
            .unwrap()
            .provider_turn_id
            .as_deref(),
        Some("provider-turn-b")
    );
    let expected_queue: Vec<_> = std::iter::once(head)
        .chain(f.worker.state.queued_requests(autonomous).iter().copied())
        .collect();
    reply.send(busy_join_error(code, true)).unwrap();
    f.worker.finish_native_submissions(false);
    assert_eq!(f.request(head).phase, RequestPhase::Queued);
    assert_eq!(f.worker.state.queued_requests(autonomous), expected_queue);

    reopen_after_interrupt(&mut f);
    poll_autonomous(&mut f, "working", 11, false, None);
    assert_eq!(
        f.worker.state.agent(autonomous).unwrap().current_request,
        None
    );
    assert_eq!(f.worker.state.queued_requests(autonomous), expected_queue);
    let foreign_episode = f.worker.state.send_interrupt_target(autonomous).unwrap();
    assert!(foreign_episode.target.is_none());
    assert_eq!(
        foreign_episode.provider_turn_id.as_deref(),
        Some("provider-turn-b")
    );
    let params = json!({"room": f.room.0.to_string(), "to": ["autonomous"], "text": "still in B"});
    let repeated = send(&mut f, params);
    assert!(repeated.ok, "{repeated:?}");
    assert_eq!(repeated.result["stage"], "queued");
    assert_eq!(
        (
            repeated.result["interrupts"][0]["outcome"].as_str(),
            stop_keys(&f).len()
        ),
        (Some("unknown"), 2)
    );
    let state = f.worker.store.load().unwrap().unwrap();
    assert!(state.request(head).unwrap().send_interrupt_claim.is_none());
    assert!(state.send_interrupt_already_claimed(autonomous, &foreign_episode));
    assert_eq!(
        state.queued_requests(autonomous).last(),
        Some(&sent_request(&repeated))
    );

    autonomous_started_with_prompt(&mut f, autonomous, "provider-turn-c", "provider work C");
    poll_autonomous(&mut f, "working", 12, false, None);
    let params = json!({"room": f.room.0.to_string(), "to": ["autonomous"], "text": "new turn C"});
    let next = send(&mut f, params);
    assert!(next.ok, "{next:?}");
    assert_eq!(next.result["stage"], "queued");
    assert_eq!(next.result["interrupts"][0]["outcome"], "interrupting");
    assert_eq!(stop_keys(&f).len(), 3);
    assert_eq!(recorded_stop_terminals(&f), ["autonomous-terminal"; 3]);
}

#[test]
fn definitely_requeued_join_keeps_the_foreign_stop_claim_after_reopen() {
    requeued_join_keeps_the_foreign_stop_claim(None);
}

#[test]
fn composer_busy_requeued_join_keeps_the_foreign_stop_claim_after_reopen() {
    requeued_join_keeps_the_foreign_stop_claim(Some(crate::protocol::api::AGENT_COMPOSER_BUSY));
}
