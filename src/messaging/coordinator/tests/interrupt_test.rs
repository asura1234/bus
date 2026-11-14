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

/// The reported Kimi failure: its empty composer was classified Idle while a
/// bound foreground command was still running. Idle must not suppress the stop
/// key or make a room interrupt omit an agent whose native turn is still open.
#[test]
fn an_idle_observation_cannot_skip_interrupting_a_bound_open_turn() {
    for (provider, key) in [
        (Provider::ClaudeCode, "esc"),
        (Provider::Codex, "esc"),
        (Provider::Cursor, "ctrl+c"),
        (Provider::Grok, "ctrl+c"),
        #[cfg(unix)]
        (Provider::Kimi, "esc"),
    ] {
        for room in [false, true] {
            let mut f = Fixture::new(provider);
            #[cfg(unix)]
            if provider == Provider::Kimi {
                super::kimi_steering_tests::home(&f);
            }
            let lead = running(&mut f, "run sleep 120 in the foreground", "1");
            f.status(RuntimeStatus::Idle);
            assert!(f.request(lead).trusted_start_bound);
            assert_eq!(f.request(lead).phase, RequestPhase::Active);
            let params = if room {
                json!({"room": f.room.0.to_string()})
            } else {
                json!({"agent": "builder"})
            };
            let response = interrupt(&mut f, params);
            assert!(response.ok, "{provider:?}: {response:?}");
            assert_eq!(
                response.result["agents"],
                json!([{"agent_id":f.agent,"name":"builder","outcome":"interrupting","key":key}]),
                "{provider:?}, room={room}"
            );
            assert_eq!(keys(&f), [("pane".to_owned(), vec![key.to_owned()])]);
            assert!(!f.request(lead).settled(), "a key write is not a stop");
            f.status(RuntimeStatus::Idle);
            assert_eq!(
                f.request(lead).failure_reason.as_deref(),
                Some("interrupted")
            );
        }
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
fn the_interrupted_turn_closes_and_a_queued_message_runs_next() {
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
        for id in [lead, joined] {
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
        assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
        assert!(fixture.request(queued).failure_reason.is_none());
        let agent = fixture.worker.state.agent(fixture.agent).unwrap();
        assert!(!agent.interrupt_pending);
        assert_eq!(agent.current_request, None);
        assert_eq!(
            fixture.worker.state.queued_requests(fixture.agent),
            &[queued]
        );
        // The message that was waiting is the next turn, not a steer.
        fixture.deliver();
        assert_eq!(fixture.request(queued).group, None);
        assert_eq!(
            fixture.typed_content().last().unwrap(),
            &("then write the docs".into(), false),
            "{provider:?}"
        );
    }
}

#[test]
fn grok_interrupt_presses_ctrl_c_once() {
    // Grok 1.0.70 ignores Esc. Its foreground footer is Ctrl+c:cancel.
    // A second Ctrl+C paints "Ctrl+c:press again to quit".
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
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert!(fixture.request(lead).failure_reason.is_none());
    assert!(fixture.reply(lead).is_none());
    assert!(fixture.worker.state.requests().all(|request| {
        request
            .reply()
            .is_none_or(|reply| reply.text != "Parser written.")
    }));
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

    // A native Stop can end foreground work while Bus holds the request for
    // background agents. This positive end evidence needs no stop key.
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"","session_crons":[],
        "background_tasks":[{"id":"b1","type":"agent","status":"running","description":"Analyze the parser"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    assert!(fixture.request(lead).awaiting_background);
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
fn interrupting_a_bound_turn_with_an_idle_screen_records_the_interrupt_in_history() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "interrupting");
    fixture.status(RuntimeStatus::Idle);
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
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    assert!(fixture.request(lead).failure_reason.is_none());
    assert!(fixture.reply(lead).is_none());
    assert!(fixture.worker.state.requests().all(|request| {
        request
            .reply()
            .is_none_or(|reply| reply.text != "Parser written.")
    }));
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
fn interrupting_with_another_conversation_room_records_the_notice_where_the_turn_ran() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let mut state = fixture.worker.state.clone();
    let side = state.create_room("side").unwrap();
    fixture.worker.save(state).unwrap();
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.status(RuntimeStatus::Idle);
    let response = interrupt(&mut fixture, json!({"agent": "builder", "room": "side"}));
    assert!(response.ok, "{response:?}");
    fixture.status(RuntimeStatus::Idle);
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

#[test]
fn missing_launch_directory_preserves_explicit_interrupt() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Idle] {
        let mut f = Fixture::new(Provider::Codex);
        let lead = running(&mut f, "run a foreground command", "cwd-open-turn");
        f.status(status);
        f.worker
            .state
            .observe_working_directory_error(f.agent, Some("launch directory was removed".into()))
            .unwrap();
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
        assert!(f.request(lead).trusted_start_bound);
        let response = interrupt(&mut f, json!({"agent":"builder"}));
        assert!(response.ok, "{response:?}");
        assert_eq!(
            response.result["agents"][0]["outcome"], "interrupting",
            "{status:?}: {response:?}"
        );
        assert_eq!(keys(&f), [("pane".to_owned(), vec!["esc".to_owned()])]);
        assert!(
            !f.request(lead).settled(),
            "sending a stop key is not completion"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
    }
}

#[test]
fn missing_launch_directory_preserves_room_interrupt() {
    for status in [RuntimeStatus::Working, RuntimeStatus::Idle] {
        let mut f = Fixture::new(Provider::Codex);
        let lead = running(&mut f, "run a foreground command", "cwd-open-turn");
        f.status(status);
        f.worker
            .state
            .observe_working_directory_error(f.agent, Some("launch directory was removed".into()))
            .unwrap();
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
        assert!(f.request(lead).trusted_start_bound);
        let room = f.room.0.to_string();
        let response = interrupt(&mut f, json!({"room":room}));
        assert!(response.ok, "{response:?}");
        assert_eq!(
            response.result["agents"],
            json!([{
                "agent_id":f.agent,"name":"builder","outcome":"interrupting","key":"esc"
            }]),
            "{status:?}: {response:?}"
        );
        assert_eq!(keys(&f), [("pane".to_owned(), vec!["esc".to_owned()])]);
        assert!(
            !f.request(lead).settled(),
            "sending a stop key is not completion"
        );
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().shown_status(),
            RuntimeStatus::Unavailable
        );
    }
}

#[test]
fn provider_flake_review_idle_presentation_cannot_skip_the_native_member_stop_key() {
    for room in [false, true] {
        let mut f = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut f, "original task", "turn-1");
        let member = f.send("native queued input", false);
        f.deliver();
        assert!(!f.request(member).trusted_start_bound);
        f.record(json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1","error":"server_error","last_assistant_message":"API Error: 529 Overloaded."}));
        f.consume();
        f.started("native-2", "native queued input");
        f.consume();
        assert_eq!(
            f.worker.state.agent(f.agent).unwrap().current_request,
            Some(member)
        );
        f.status(RuntimeStatus::Idle);
        let params = if room {
            json!({"room":f.room.0.to_string()})
        } else {
            json!({"agent":"builder"})
        };
        let response = interrupt(&mut f, params);
        assert!(response.ok, "{response:?}");
        assert_eq!(keys(&f), [("pane".to_owned(), vec!["esc".to_owned()])]);
        assert_eq!(response.result["agents"][0]["outcome"], "interrupting");
        f.status(RuntimeStatus::Idle);
        for id in [lead, member] {
            assert_eq!(f.request(id).failure_reason.as_deref(), Some("interrupted"));
        }
    }
}

#[test]
fn room_interrupt_cancels_idle_and_unavailable_provider_holds() {
    for status in [RuntimeStatus::Idle, RuntimeStatus::Unavailable] {
        let mut f = Fixture::new(Provider::ClaudeCode);
        let lead = running(&mut f, "owned task", "turn-1");
        f.record(json!({"hook_event_name":"StopFailure","session_id":"session","prompt_id":"turn-1",
            "error":"server_error","last_assistant_message":"Model overloaded (server_overloaded)"}));
        f.consume();
        f.status(status);
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_some());
        let room = f.room.0.to_string();
        let response = interrupt(&mut f, json!({"room":room}));
        assert!(response.ok, "{response:?}");
        assert_eq!(
            f.request(lead).failure_reason.as_deref(),
            Some("interrupted"),
            "room interrupt skipped {status:?}: {response:?}"
        );
        assert!(f
            .worker
            .state
            .agent(f.agent)
            .unwrap()
            .provider_retry
            .is_none());
        assert!(
            keys(&f).is_empty(),
            "an inactive hold needs cancellation, not a stop key"
        );
    }
}

// Native completion is stronger than a key admission, and is bound to the
// exact terminal/session and interrupted obligation rather than presentation Idle.
struct InterruptReceipt(Result<ResponseResult, TransportError>);
impl Transport for InterruptReceipt {
    fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
        std::mem::replace(&mut self.0, Ok(ResponseResult::Ok {}))
    }
}

fn native_receipt(f: &mut Fixture, terminal: &str, session: &str) {
    f.worker.transport = Box::new(InterruptReceipt(Ok(ResponseResult::AgentInterrupted {
        terminal_id: terminal.into(),
        session_id: session.into(),
        turn_id: Some("turn-1".into()),
    })));
}

#[test]
fn native_interrupt_completion_closes_its_turn_and_keeps_a_blocked_drafts_queue() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    let queued = f.send("later", true);
    f.status(RuntimeStatus::Blocked);
    native_receipt(&mut f, "terminal", "session");
    let response = interrupt(&mut f, json!({"agent":"builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "stopped");
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("interrupted")
    );
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert!(f.request(queued).failure_reason.is_none());
    assert_eq!(f.worker.state.queued_requests(f.agent), &[queued]);
    let agent = f.worker.state.agent(f.agent).unwrap();
    assert!(!agent.interrupt_pending);
    assert_eq!(agent.current_request, None);
    assert_eq!(agent.activity_status(), RuntimeStatus::Blocked);
}

#[test]
fn native_interrupt_mismatched_identity_cannot_settle_or_poll_visual_idle() {
    for (terminal, session) in [
        ("other", "session"),
        ("terminal", "other"),
        ("", "session"),
        ("terminal", ""),
    ] {
        let mut f = Fixture::new(Provider::Codex);
        let lead = running(&mut f, "work", "turn-1");
        native_receipt(&mut f, terminal, session);
        let response = interrupt(&mut f, json!({"agent":"builder"}));
        assert_eq!(response.result["agents"][0]["outcome"], "failed");
        assert!(!f.worker.state.agent(f.agent).unwrap().interrupt_pending);
        f.status(RuntimeStatus::Idle);
        assert_eq!(f.request(lead).failure_reason, None);
        assert!(!f.request(lead).settled());
    }
}

#[test]
fn native_interrupt_unconfirmed_cannot_become_stopped_from_visual_idle() {
    for code in [Some("agent_interrupt_unconfirmed".into()), None] {
        let mut f = Fixture::new(Provider::Codex);
        let lead = running(&mut f, "work", "turn-1");
        f.worker.transport = Box::new(InterruptReceipt(Err(TransportError {
            code,
            message: "Native interrupt is unconfirmed".into(),
            definitely_rejected: false,
            timestamps: Box::default(),
        })));
        let response = interrupt(&mut f, json!({"agent":"builder"}));
        assert_eq!(response.result["agents"][0]["outcome"], "failed");
        assert!(!f.worker.state.agent(f.agent).unwrap().interrupt_pending);
        f.status(RuntimeStatus::Idle);
        assert!(!f.request(lead).settled());
        assert_eq!(f.request(lead).failure_reason, None);
    }
}

#[test]
fn native_interrupt_completion_settles_a_final_without_creating_a_reply() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    let unclaimed = f.send("steered input without a start hook", false);
    f.deliver();
    f.stopped("turn-1", "Work done.");
    f.consume();
    assert!(f.request(lead).pending_final.is_some());
    f.status(RuntimeStatus::Blocked);
    native_receipt(&mut f, "terminal", "session");
    let response = interrupt(&mut f, json!({"agent":"builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "stopped");
    assert!(f.reply(lead).is_none());
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert_eq!(f.request(lead).failure_reason, None);
    assert_eq!(
        f.request(unclaimed).failure_reason.as_deref(),
        Some("interrupted")
    );
}

#[test]
fn native_interrupt_completion_preserves_an_explicit_reply() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    let unclaimed = f.send("steered input without a start hook", false);
    f.deliver();
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    let replied = f.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "reply-before-native-interrupt".into(),
            method: "message.send".into(),
            params: json!({"reply_to": message.0.to_string(), "as": agent.0.to_string(), "text": "explicit answer"}),
        },
        None,
    );
    assert!(replied.ok, "{replied:?}");
    assert_eq!(replied.result["stage"], "replied");
    f.stopped("turn-1", "different turn-final text");
    f.consume();
    assert!(f.request(lead).pending_final.is_some());
    f.status(RuntimeStatus::Blocked);
    native_receipt(&mut f, "terminal", "session");
    let response = interrupt(&mut f, json!({"agent":"builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "stopped");
    assert_eq!(f.reply(lead).as_deref(), Some("explicit answer"));
    assert_eq!(f.request(lead).failure_reason, None);
    assert_eq!(
        f.request(unclaimed).failure_reason.as_deref(),
        Some("interrupted")
    );
}

#[test]
fn late_native_interrupt_completion_cannot_close_a_successor_in_the_same_session() {
    let mut f = Fixture::new(Provider::Codex);
    let old = running(&mut f, "old work", "turn-1");
    assert_eq!(
        interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
        "interrupting"
    );
    let fence = f
        .worker
        .native_interrupt_fence(&f.worker.state, f.agent)
        .unwrap();
    let mut state = f.worker.state.clone();
    state.close_interrupted(f.agent, f.now).unwrap();
    f.worker.save(state).unwrap();
    f.status(RuntimeStatus::Idle);
    let fresh = running(&mut f, "new work", "turn-2");
    let queued = f.send("new queued work", true);
    let mut state = f.worker.state.clone();
    assert!(!f
        .worker
        .complete_native_interrupt(
            &mut state,
            f.agent,
            &fence,
            "terminal",
            "session",
            Some("turn-1"),
            f.now
        )
        .unwrap());
    f.worker.save(state).unwrap();
    assert_eq!(
        f.request(old).failure_reason.as_deref(),
        Some("interrupted")
    );
    assert!(!f.request(fresh).settled());
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert_eq!(
        f.worker.state.agent(f.agent).unwrap().current_request,
        Some(fresh)
    );
}

#[test]
fn native_interrupt_fence_rejects_a_new_queue_or_same_request_new_submission() {
    for changed_queue in [true, false] {
        let mut f = Fixture::new(Provider::Codex);
        let lead = running(&mut f, "work", "turn-1");
        assert_eq!(
            interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
            "interrupting"
        );
        let fence = f
            .worker
            .native_interrupt_fence(&f.worker.state, f.agent)
            .unwrap();
        if changed_queue {
            f.send("fresh queue", true);
        }
        let mut state = f.worker.state.clone();
        if !changed_queue {
            // Deserialize a persisted state to model a late completion crossing
            // a re-submission epoch without changing terminal/session/request ID.
            let mut value = serde_json::to_value(&state).unwrap();
            let request = value["requests"]
                .as_object_mut()
                .unwrap()
                .get_mut(&lead.0.to_string())
                .unwrap();
            request["submission_status_revision"] =
                json!(f.request(lead).submission_status_revision + 1);
            state = serde_json::from_value(value).unwrap();
        }
        assert!(!f
            .worker
            .complete_native_interrupt(
                &mut state,
                f.agent,
                &fence,
                "terminal",
                "session",
                Some("turn-1"),
                f.now
            )
            .unwrap());
        assert!(!state.request(lead).unwrap().settled());
    }
}

#[test]
fn native_interrupt_receipt_for_a_replacement_turn_cannot_close_the_original_request() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    f.worker.transport = Box::new(InterruptReceipt(Ok(ResponseResult::AgentInterrupted {
        terminal_id: "terminal".into(),
        session_id: "session".into(),
        turn_id: Some("replacement".into()),
    })));
    let response = interrupt(&mut f, json!({"agent":"builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "failed");
    assert!(!f.request(lead).settled());
    assert!(!f.worker.state.agent(f.agent).unwrap().interrupt_pending);
}

#[test]
fn native_unconfirmed_interrupt_disarms_an_older_visual_stop_wait() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    let mut state = f.worker.state.clone();
    state.begin_interrupt(f.agent).unwrap();
    f.worker.save(state).unwrap();
    f.worker.transport = Box::new(InterruptReceipt(Err(TransportError {
        code: Some("agent_interrupt_unconfirmed".into()),
        message: "unconfirmed".into(),
        definitely_rejected: false,
        timestamps: Box::default(),
    })));
    let response = interrupt(&mut f, json!({"agent":"builder"}));
    assert_eq!(response.result["agents"][0]["outcome"], "failed");
    assert!(
        !f.worker.state.agent(f.agent).unwrap().interrupt_pending,
        "a previous typed admission cannot promote newer native uncertainty through visual Idle"
    );
    f.status(RuntimeStatus::Idle);
    assert!(!f.request(lead).settled());
}

#[test]
fn late_unconfirmed_native_interrupt_preserves_a_newer_same_obligation_stop_wait() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    assert_eq!(
        interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
        "interrupting"
    );
    let old = f
        .worker
        .native_interrupt_fence(&f.worker.state, f.agent)
        .unwrap();
    // A second, ordinary typed admission of the same obligation creates its
    // own stop wait and durable interruption notice through the real control.
    assert_eq!(
        interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
        "interrupting"
    );
    let mut state = f.worker.state.clone();
    assert!(!f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &old)
        .unwrap());
    assert!(state.agent(f.agent).unwrap().interrupt_pending);
    assert!(!state.request(lead).unwrap().settled());
    assert_eq!(state.agent(f.agent).unwrap().current_request, Some(lead));
}

#[test]
fn late_unconfirmed_native_interrupt_preserves_a_successor_stop_wait() {
    let mut f = Fixture::new(Provider::Codex);
    running(&mut f, "old work", "turn-1");
    assert_eq!(
        interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
        "interrupting"
    );
    let old = f
        .worker
        .native_interrupt_fence(&f.worker.state, f.agent)
        .unwrap();
    let mut state = f.worker.state.clone();
    state.close_interrupted(f.agent, f.now).unwrap();
    f.worker.save(state).unwrap();
    f.status(RuntimeStatus::Idle);
    let fresh = running(&mut f, "fresh work", "turn-2");
    assert_eq!(
        interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
        "interrupting"
    );
    let mut state = f.worker.state.clone();
    assert!(!f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &old)
        .unwrap());
    assert!(state.agent(f.agent).unwrap().interrupt_pending);
    assert!(!state.request(fresh).unwrap().settled());
    assert_eq!(state.agent(f.agent).unwrap().current_request, Some(fresh));
}

/// Live session 2b704f6e8df6bedd, message 13324 / request 13325: a send accepted
/// as queued while an interrupt was still outstanding was abandoned when the
/// turn later went idle. `message status` showed stage abandoned, reason
/// interrupted, delivery_error null, and the text was never typed.
#[test]
fn queued_messages_survive_an_interrupt_closed_turn_and_deliver_next() {
    for provider in [
        Provider::ClaudeCode,
        Provider::Codex,
        Provider::Cursor,
        Provider::Grok,
    ] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "write the parser", "turn-1");
        // Grok does not steer a same-room send into a running turn; that send
        // is already an ordinary queued message and is covered below.
        let steered = (provider != Provider::Grok).then(|| {
            let steered = fixture.send("use the streaming API instead", false);
            fixture.deliver();
            assert_eq!(
                fixture.request(steered).group,
                Some(lead),
                "{provider:?} steering did not join the open turn"
            );
            steered
        });
        assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
        // The stop key was pressed; the provider is still working, which is
        // the interrupt_failed window. A normal send must queue, not join.
        fixture.status(RuntimeStatus::Working);
        let first = fixture.send("first follow-up", false);
        let second = fixture.send("second follow-up", true);
        let typed_before = fixture.typed_content().len();
        fixture.deliver();
        assert_eq!(
            fixture.typed_content().len(),
            typed_before,
            "{provider:?} typed a follow-up into the stopped turn"
        );
        fixture.status(RuntimeStatus::Idle);
        assert_eq!(
            fixture.request(lead).failure_reason.as_deref(),
            Some("interrupted"),
            "{provider:?}"
        );
        if let Some(steered) = steered {
            assert_eq!(
                fixture.request(steered).failure_reason.as_deref(),
                Some("interrupted"),
                "{provider:?} steered member"
            );
        }
        assert_eq!(
            fixture.worker.state.queued_requests(fixture.agent),
            &[first, second],
            "{provider:?}"
        );
        for id in [first, second] {
            let request = fixture.request(id);
            assert_eq!(request.phase, RequestPhase::Queued, "{provider:?} {id:?}");
            assert!(
                request.failure_reason.is_none(),
                "{provider:?} queued request closed: {request:?}"
            );
            let message = request.prompt.id;
            let status = fixture.worker.control_message(message).unwrap();
            assert_eq!(status["complete"], false, "{provider:?} {status}");
            assert_eq!(
                status["requests"][0]["stage"], "queued",
                "{provider:?} {status}"
            );
            assert_ne!(
                status["requests"][0]["reason"], "interrupted",
                "{provider:?} silent abandon: {status}"
            );
        }
        fixture.deliver();
        assert_eq!(
            fixture
                .typed_content()
                .last()
                .map(|(text, steer)| (text.as_str(), *steer)),
            Some(("first follow-up", false)),
            "{provider:?}"
        );
        assert_ne!(fixture.request(first).phase, RequestPhase::Queued);
        assert_eq!(fixture.request(second).phase, RequestPhase::Queued);
        assert_eq!(
            fixture.worker.state.queued_requests(fixture.agent),
            &[second]
        );
    }
}

#[test]
fn interrupting_an_idle_agent_keeps_a_queue_that_never_started() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let first = fixture.send("first queued task", false);
    let second = fixture.send("second queued task", true);
    let response = interrupt(&mut fixture, json!({"agent": "builder"}));
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result["agents"][0]["outcome"], "already_idle");
    assert!(keys(&fixture).is_empty());
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[first, second]
    );
    for id in [first, second] {
        assert_eq!(fixture.request(id).phase, RequestPhase::Queued);
        assert!(fixture.request(id).failure_reason.is_none());
    }
    let notices: Vec<_> = fixture
        .worker
        .state
        .notices(fixture.room)
        .map(|notice| notice.text.clone())
        .collect();
    assert!(
        notices.iter().all(|text| !text.starts_with("Interrupted ")),
        "an idle queue is not an interrupted turn: {notices:?}"
    );
    fixture.deliver();
    assert_eq!(
        fixture
            .typed_content()
            .last()
            .map(|(text, steer)| (text.as_str(), *steer)),
        Some(("first queued task", false))
    );
    assert_eq!(fixture.request(second).phase, RequestPhase::Queued);
}

/// A relaunch observes Unavailable and clears `interrupt_pending` without
/// draining the queue. The bound turn continues; a message that never joined
/// it stays queued behind that continuation.
#[test]
fn a_queued_message_survives_restart_while_an_interrupt_is_pending() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    assert!(interrupt(&mut fixture, json!({"agent": "builder"})).ok);
    assert!(
        fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending
    );
    let queued = fixture.send("survive the restart", true);
    fixture.deliver();
    assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
    let placeholder = Worker::open(
        fixture.dir.join("restart-placeholder"),
        Box::new(super::Native(fixture.native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut fixture.worker, placeholder));
    fixture.worker = Worker::open(
        fixture.dir.clone(),
        Box::new(super::Native(fixture.native.clone())),
    )
    .unwrap();
    fixture.worker.serving_generation = 2;
    assert!(
        !fixture
            .worker
            .state
            .agent(fixture.agent)
            .unwrap()
            .interrupt_pending,
        "a relaunch ends the stop; it must not abandon the queue as interrupted"
    );
    fixture.status(RuntimeStatus::Idle);
    fixture.worker.settle_requests().unwrap();
    let continued = fixture.request(lead);
    assert!(
        continued.restart_continuation && !continued.settled(),
        "{continued:?}"
    );
    assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
    assert!(fixture.request(queued).failure_reason.is_none());
    fixture.deliver();
    let notice = fixture.typed().last().unwrap().0.clone();
    assert!(
        notice.contains("Bus restarted"),
        "the bound turn continues after restart: {notice}"
    );
    assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
    assert!(fixture.request(queued).failure_reason.is_none());
}

#[test]
fn grok_user_interrupt_closes_the_bound_turn_and_keeps_the_queue() {
    let mut fixture = Fixture::new(Provider::Grok);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let queued = fixture.send("next task", false);
    fixture.deliver();
    assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
    fixture.record(json!({
        "hookEventName": "stop_cancelled",
        "sessionId": "session",
        "promptId": "turn-1",
        "reason": "user_interrupt"
    }));
    fixture.consume();
    let lead_request = fixture.request(lead);
    assert_eq!(lead_request.phase, RequestPhase::Abandoned);
    assert_eq!(
        lead_request.failure_reason.as_deref(),
        Some("provider_error: user_interrupt")
    );
    let lead_message = lead_request.prompt.id;
    let status = fixture.worker.control_message(lead_message).unwrap();
    assert_eq!(status["requests"][0]["stage"], "abandoned", "{status}");
    assert_eq!(
        status["requests"][0]["reason"], "provider_error: user_interrupt",
        "{status}"
    );
    let error = fixture
        .worker
        .state
        .agent(fixture.agent)
        .unwrap()
        .actionable_error
        .clone()
        .unwrap_or_default();
    assert!(
        error.contains("user_interrupt"),
        "the sender-visible agent error omitted the interrupt: {error}"
    );
    assert_eq!(fixture.request(queued).phase, RequestPhase::Queued);
    assert!(fixture.request(queued).failure_reason.is_none());
    let queued_message = fixture.request(queued).prompt.id;
    let queued_status = fixture.worker.control_message(queued_message).unwrap();
    assert_eq!(
        queued_status["requests"][0]["stage"], "queued",
        "{queued_status}"
    );
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    assert_eq!(
        fixture
            .typed_content()
            .last()
            .map(|(text, steer)| (text.as_str(), *steer)),
        Some(("next task", false))
    );
}

#[test]
fn unconfirmed_native_interrupt_clears_only_the_matching_visual_wait() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    f.send("unclaimed steering", false);
    f.deliver();
    f.stopped("turn-1", "Captured final.");
    f.consume();
    let queued = f.send("queued work", true);
    assert!(f.request(lead).pending_final.is_some());
    let before = f.request(lead).clone();
    let before_queued = f.request(queued).clone();
    assert_eq!(
        interrupt(&mut f, json!({"agent":"builder"})).result["agents"][0]["outcome"],
        "interrupting"
    );
    let mut state = f.worker.state.clone();
    let status = state.agent(f.agent).unwrap().activity_status();
    let fence = f.worker.native_interrupt_fence(&state, f.agent).unwrap();
    state
        .post_bus_notice(f.room, None, "unrelated room traffic".into(), f.now)
        .unwrap();
    assert!(f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &fence)
        .unwrap());
    assert!(!state.agent(f.agent).unwrap().interrupt_pending);
    assert_eq!(state.request(lead).unwrap(), &before);
    assert_eq!(state.request(queued).unwrap(), &before_queued);
    assert_eq!(state.agent(f.agent).unwrap().activity_status(), status);
}

#[test]
fn late_native_completion_preserves_a_newer_same_obligation_interrupt() {
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    interrupt(&mut f, json!({"agent":"builder"}));
    let old = f
        .worker
        .native_interrupt_fence(&f.worker.state, f.agent)
        .unwrap();
    interrupt(&mut f, json!({"agent":"builder"}));
    let mut state = f.worker.state.clone();
    assert!(!f
        .worker
        .complete_native_interrupt(
            &mut state,
            f.agent,
            &old,
            "terminal",
            "session",
            Some("turn-1"),
            f.now
        )
        .unwrap());
    assert!(state.agent(f.agent).unwrap().interrupt_pending);
    assert!(!state.request(lead).unwrap().settled());
}

#[test]
fn native_interrupt_attempt_is_not_reused_after_pruning_or_coordinator_reopen() {
    let mut f = Fixture::new(Provider::Codex);
    running(&mut f, "work", "turn-1");
    interrupt(&mut f, json!({"agent":"builder"}));
    let old = f
        .worker
        .native_interrupt_fence(&f.worker.state, f.agent)
        .unwrap();
    let saved = f.worker.state.clone();
    f.worker.interrupt_attempts.latest.clear();
    let mut state = saved.clone();
    assert!(!f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &old)
        .unwrap());
    interrupt(&mut f, json!({"agent":"builder"}));
    assert!(!f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &old)
        .unwrap());
    // Drop the original lease while retaining the old fence and its epoch.
    let dummy = Worker::open(f.dir.join("dummy"), Box::new(Native(f.native.clone()))).unwrap();
    drop(std::mem::replace(&mut f.worker, dummy));
    f.worker = Worker::open(f.dir.clone(), Box::new(Native(f.native.clone()))).unwrap();
    f.worker.save(saved.clone()).unwrap();
    assert!(f.worker.native_interrupt_fence(&saved, f.agent).is_none());
    interrupt(&mut f, json!({"agent":"builder"}));
    assert_eq!(
        f.worker.interrupt_attempts.serial, 1,
        "force the same serial in a new epoch"
    );
    let mut state = f.worker.state.clone();
    assert!(!f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &old)
        .unwrap());
    assert!(state.agent(f.agent).unwrap().interrupt_pending);
}

#[cfg(unix)]
#[test]
fn native_interrupt_attempt_does_not_roll_back_when_model_save_fails() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new(Provider::Codex);
    let lead = running(&mut f, "work", "turn-1");
    interrupt(&mut f, json!({"agent":"builder"}));
    let old = f
        .worker
        .native_interrupt_fence(&f.worker.state, f.agent)
        .unwrap();
    std::fs::set_permissions(&f.dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = f.worker.control_interrupt(&json!({"agent":"builder"}));
    std::fs::set_permissions(&f.dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.is_err(), "the admission's model save must fail");
    let mut state = f.worker.state.clone();
    assert!(!f
        .worker
        .clear_unconfirmed_native_interrupt(&mut state, f.agent, &old)
        .unwrap());
    assert!(state.agent(f.agent).unwrap().interrupt_pending);
    assert!(!state.request(lead).unwrap().settled());
}
