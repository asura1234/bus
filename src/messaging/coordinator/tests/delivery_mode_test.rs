//! `bus send --mode`: who may choose how a message reaches a running turn.
use super::*;
use crate::messaging::control::Response;

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
    let mut providers = vec![Provider::ClaudeCode, Provider::Codex, Provider::Agy];
    #[cfg(unix)]
    providers.push(Provider::Kimi);
    providers
}

#[test]
fn agy_is_rejected_as_master_orchestrator_in_control_and_commands() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let master = fixture.worker.state.master_room().unwrap().id;
    let response = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "agy-master-reject".into(),
            method: "agent.add".into(),
            params: json!({
                "room": master.0.to_string(),
                "name": "agy-lead",
                "provider": "agy",
                "cwd": fixture.dir.to_string_lossy(),
                "orchestrates": fixture.room.0.to_string(),
                "consent_project_hooks": true,
            }),
        },
        None,
    );
    assert!(!response.ok, "{response:?}");
    let message = response
        .error
        .as_ref()
        .map(|err| err.message.as_str())
        .unwrap_or_default();
    assert!(
        message.contains("work-room worker only"),
        "expected worker-only error, got: {message}"
    );
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
