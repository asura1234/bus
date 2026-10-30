use super::*;

fn interrupt(fixture: &mut Fixture, params: Value) -> crate::messaging::control::Response {
    // Each call is a new request: a repeated ID would replay the first receipt.
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = format!("interrupt-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id,
            method: "agent.interrupt".into(),
            params,
        },
        None,
    )
}

/// The keys Bus sent with `agent.send_keys`, per target pane.
fn keys(fixture: &Fixture) -> Vec<(String, Vec<String>)> {
    fixture
        .native
        .lock()
        .unwrap()
        .iter()
        .filter_map(|method| match method {
            Method::AgentSendKeys(params) => Some((params.target.clone(), params.keys.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn interrupt_presses_each_providers_stop_key_once_and_records_who_interrupted_whom() {
    // Live: one Esc stops Claude Code and Codex; a second one would open
    // Claude's rewind or Codex's edit-previous-message. Cursor stops on
    // Ctrl+C, and a second Ctrl+C quits it.
    for (provider, key) in [
        (Provider::ClaudeCode, "esc"),
        (Provider::Codex, "esc"),
        (Provider::Cursor, "ctrl+c"),
    ] {
        let mut fixture = Fixture::new(provider);
        running(&mut fixture, "write the parser", "turn-1");
        let response = interrupt(&mut fixture, json!({"agent": "builder"}));
        assert!(response.ok, "{provider:?}: {response:?}");
        assert_eq!(
            response.result["agents"],
            json!([{"agent_id": fixture.agent, "name": "builder", "outcome": "interrupting", "key": key}]),
            "{provider:?}"
        );
        assert_eq!(response.result["room_id"], json!(fixture.room));
        assert_eq!(keys(&fixture), [("pane".to_owned(), vec![key.to_owned()])]);
        let agent = fixture.worker.state.agent(fixture.agent).unwrap();
        assert!(agent.interrupt_pending, "{provider:?}");
        let notice = fixture.worker.state.notices(fixture.room).last().unwrap();
        assert_eq!(notice.author, Author::Developer);
        assert_eq!(notice.text, "Interrupted builder.");
        assert!(notice.recipient_ids.is_empty());
    }
}

#[test]
fn an_orchestrator_interrupt_is_recorded_as_written_by_it() {
    let mut fixture = Fixture::new(Provider::Codex);
    let mut state = fixture.worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orchestrator = state
        .create_agent(
            master,
            "orch",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orchestrator, fixture.room).unwrap();
    fixture.worker.save(state).unwrap();
    running(&mut fixture, "write the parser", "turn-1");
    let response = interrupt(&mut fixture, json!({"agent": "builder", "as": "orch"}));
    assert!(response.ok, "{response:?}");
    let notice = fixture.worker.state.notices(fixture.room).last().unwrap();
    assert_eq!(notice.author, Author::Agent(orchestrator));
    assert_eq!(notice.text, "Interrupted builder.");
}

#[test]
fn the_interrupted_turn_and_the_queue_close_as_interrupted_once_the_agent_is_idle() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "write the parser", "turn-1");
        let joined = fixture.send("use the streaming API instead", false);
        fixture.deliver();
        let queued = fixture.send("then write the docs", true);
        assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
        // Still working: nothing closes before the provider stops.
        fixture.status(RuntimeStatus::Working);
        assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
        // No Stop hook follows a provider interrupt; the idle screen ends it,
        // before the reply settlement could call it reply_not_captured.
        fixture.status(RuntimeStatus::Idle);
        for id in [lead, joined, queued] {
            let request = fixture.request(id);
            assert_eq!(
                request.phase,
                RequestPhase::Abandoned,
                "{provider:?} {id:?}"
            );
            assert_eq!(
                request.failure_reason.as_deref(),
                Some("interrupted"),
                "{provider:?} {id:?}"
            );
        }
        let agent = fixture.worker.state.agent(fixture.agent).unwrap();
        assert!(!agent.interrupt_pending);
        assert_eq!(agent.current_request, None);
        assert!(fixture
            .worker
            .state
            .queued_requests(fixture.agent)
            .is_empty());
        // A message sent after the stop is a normal new turn.
        let next = fixture.send("pause and report your status", false);
        fixture.deliver();
        assert_eq!(fixture.request(next).group, None);
        assert_eq!(
            fixture.typed_content().last().unwrap(),
            &("pause and report your status".into(), false),
            "{provider:?}"
        );
    }
}

#[test]
fn grok_interrupt_presses_ctrl_c_once() {
    // Grok 1.0.70 ignores Esc. Its working hint is "Press Ctrl+c to cancel
    // the turn", and a second Ctrl+C escalates toward quit.
    let mut fixture = Fixture::new(Provider::Grok);
    fixture.status(RuntimeStatus::Working);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert!(response.ok, "{response:?}");
    assert_eq!(
        response.result["agents"],
        json!([{
            "agent_id": fixture.agent,
            "name": "builder",
            "outcome": "interrupting",
            "key": "ctrl+c"
        }])
    );
    assert_eq!(
        keys(&fixture),
        [("pane".to_owned(), vec!["ctrl+c".to_owned()])]
    );
    assert!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending
    );
}

#[test]
fn message_status_reports_the_interrupt_reason() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
    fixture.status(RuntimeStatus::Idle);
    let message = fixture.request(lead).prompt.id;
    let status = fixture.worker.control_message(message).unwrap();
    assert_eq!(status["complete"], true);
    assert_eq!(status["requests"][0]["stage"], "abandoned");
    assert_eq!(status["requests"][0]["reason"], "interrupted");
}

#[test]
fn a_reply_captured_before_the_stop_took_effect_completes_normally() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.stopped("turn-1", "Parser written.");
    fixture.consume();
    assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("Parser written."));
    assert!(
        !fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending
    );
}

#[test]
fn interrupting_an_idle_agent_types_nothing_and_closes_what_it_still_holds() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result["agents"][0]["outcome"], "already_idle");
    assert!(keys(&fixture).is_empty());
    assert!(
        !fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending
    );

    // A turn the provider already ended but Bus still holds (settling, or
    // waiting on background shells) closes as interrupted, with no key.
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "already_idle");
    assert!(keys(&fixture).is_empty());
    assert_eq!(fixture.request(lead).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
}

#[test]
fn an_agent_that_is_not_running_cannot_be_interrupted() {
    let mut fixture = Fixture::new(Provider::Codex);
    fixture.status(RuntimeStatus::Unavailable);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert!(response.ok, "{response:?}");
    let row = &response.result["agents"][0];
    assert_eq!(row["outcome"], "failed");
    assert!(
        row["reason"].as_str().unwrap().contains("unavailable"),
        "{row}"
    );
    assert!(keys(&fixture).is_empty());
}

#[test]
fn a_room_interrupt_stops_every_busy_agent_but_the_author() {
    let mut fixture = Fixture::new(Provider::Codex);
    let mut state = fixture.worker.state.clone();
    let idle = state
        .create_agent(
            fixture.room,
            "idler",
            Provider::Codex,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.confirm_hook_setup(idle).unwrap();
    state.observe_status(idle, RuntimeStatus::Idle, 1).unwrap();
    let author = state
        .create_agent(
            fixture.room,
            "lead",
            Provider::Codex,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    state.confirm_hook_setup(author).unwrap();
    state
        .observe_status(author, RuntimeStatus::Working, 1)
        .unwrap();
    fixture.worker.save(state).unwrap();
    running(&mut fixture, "write the parser", "turn-1");
    let response = interrupt(&mut fixture, json!({"room": "room", "as": "lead"}));
    assert!(response.ok, "{response:?}");
    assert_eq!(
        response.result["agents"],
        json!([{"agent_id": fixture.agent, "name": "builder", "outcome": "interrupting", "key": "esc"}])
    );
    assert_eq!(keys(&fixture).len(), 1);
}

#[test]
fn room_history_shows_the_interrupt_with_its_author_and_time() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
    let history = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "history".into(),
            method: "room.history".into(),
            params: json!({"room": "room"}),
        },
        None,
    );
    assert!(history.ok, "{history:?}");
    let messages = history.result["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert_eq!(
        messages[0]["prompt"]["id"],
        json!(fixture.request(lead).prompt.id)
    );
    let notice = &messages[1];
    assert_eq!(notice["prompt"]["author"], "human");
    assert_eq!(notice["prompt"]["text"], "Interrupted builder.");
    assert!(notice["prompt"]["submitted_at_ms"].as_u64().unwrap() > 0);
    assert!(notice["delivery"].is_null());
}

#[test]
fn closing_an_idle_agents_held_turn_as_interrupted_records_the_interrupt_in_history() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "already_idle");
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
    // `wait` on this message says "the room history shows who stopped it".
    let notices: Vec<String> = fixture
        .worker
        .state
        .notices(fixture.room)
        .map(|notice| notice.text.clone())
        .collect();
    assert_eq!(notices, ["Interrupted builder."]);
}

#[test]
fn a_message_sent_after_the_agent_restarted_is_not_closed_by_a_stale_interrupt() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "write the parser", "turn-1");
    assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
    // The provider died before Bus saw it idle; the CLI reported failed.
    fixture.status(RuntimeStatus::Unavailable);
    fixture.status(RuntimeStatus::Launching);
    let later = fixture.send("start over on the lexer", false);
    fixture.status(RuntimeStatus::Idle);
    assert_ne!(
        fixture.request(later).failure_reason.as_deref(),
        Some("interrupted"),
        "{:?}",
        fixture.request(later)
    );
}

#[test]
fn a_reply_captured_after_the_key_but_before_idle_completes_normally() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
    fixture.stopped("turn-1", "Parser written.");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("Parser written."));
}

#[test]
fn interrupting_with_another_conversation_room_records_the_notice_where_the_turn_ran() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let mut state = fixture.worker.state.clone();
    let side = state.create_room("side").unwrap();
    fixture.worker.save(state).unwrap();
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    let response = interrupt(&mut fixture, json!({"agent": "builder", "room": "side"}));
    assert!(response.ok, "{response:?}");
    assert_eq!(
        fixture.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
    // `wait` on the closed message points to its room's history for who stopped it.
    let in_turn_room: Vec<String> = fixture
        .worker
        .state
        .notices(fixture.room)
        .map(|notice| notice.text.clone())
        .collect();
    let in_side: usize = fixture.worker.state.notices(side).count();
    assert_eq!(
        in_turn_room,
        ["Interrupted builder."],
        "notice went to the --room conversation instead ({in_side} there)"
    );
}
