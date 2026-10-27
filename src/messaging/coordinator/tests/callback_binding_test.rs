//! Callback-binding regressions from the unbound-callback investigation; no live provider.
use super::*;

#[test]
fn empty_stop_settles_without_sender_errors_or_orchestrator_notices() {
    for provider in [Provider::Codex, Provider::ClaudeCode] {
        for text in [None, Some(json!(null)), Some(json!(""))] {
            let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
            let mut state = worker.state.clone();
            let orch = state
                .create_agent(
                    state.master_room().unwrap().id,
                    "orch",
                    Provider::ClaudeCode,
                    dir.clone(),
                    None,
                )
                .unwrap();
            state.bind_orchestrator(orch, room).unwrap();
            let request = state
                .submit_message_from(
                    room,
                    Draft {
                        text: "Do not reply".into(),
                        files: vec![],
                        recipient_ids: [agent].into(),
                    },
                    Author::Agent(orch),
                    io::now_ms(),
                )
                .unwrap()[0];
            state.begin_submission(request, "launch", 0).unwrap();
            let prompt = state.render_request_payload(request).unwrap();
            worker.save(state).unwrap();
            record(
                &dir,
                provider,
                json!({"hook_event_name":"UserPromptSubmit",
                "session_id":"session","turn_id":"turn","prompt_id":"turn","prompt":prompt}),
            );
            consume(&mut worker, agent, &dir);
            let unread = worker.state.room(room).unwrap().unread_count;
            let mut stop = json!({"hook_event_name":"Stop","session_id":"session",
                "turn_id":"turn","prompt_id":"turn"});
            if let Some(text) = text {
                stop["last_assistant_message"] = text;
            }
            record(&dir, provider, stop);
            consume(&mut worker, agent, &dir);
            worker
                .state
                .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
                .unwrap();
            let result = worker.state.request(request).unwrap();
            assert_eq!(result.phase, RequestPhase::Completed);
            assert_eq!(result.reply().unwrap().text, "");
            assert!(result.failure_reason.is_none());
            assert!(worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .is_none());
            assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
            assert_eq!(worker.state.room(room).unwrap().unread_count, unread);
            worker
                .state
                .route_worker_replies(io::now_ms() + 10_000, &BTreeSet::new())
                .unwrap();
            assert!(
                !worker.state.requests().any(|r| r.agent_id == orch),
                "an intentional empty reply must not wake the orchestrator"
            );
            assert!(callbacks::records(&dir.join("callbacks/launch"))
                .unwrap()
                .is_empty());
            drop(worker);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[test]
fn empty_stop_preexisting_retained_record_recovers_after_restart_by_turn_id() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let request = queue(&mut worker, room, agent, "Do not reply");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit",
        "session_id":"session","turn_id":"owned","prompt":"Do not reply"}),
    );
    consume(&mut worker, agent, &dir);
    let spool = dir.join("callbacks/launch");
    // Old Bus retained these bytes as event-*.json after rejecting a null Stop.
    // Write the old envelope directly, without the current hook publisher.
    let mut old = json!({"id":"abcd","sequence":2,"at_ms":io::now_ms(),
        "manifest":{"agent_id":agent.0,"provider":"codex","launch_id":"launch"},
        "value":{"hook_event_name":"Stop","session_id":"session","turn_id":"owned",
            "transcript_path":"/tmp/retained.jsonl","last_assistant_message":null}});
    std::fs::write(
        spool.join("event-abcd.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    worker
        .state
        .set_agent_error(agent, Some("Hook missing last_assistant_message".into()))
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    drop(worker);
    let mut worker = reopen_saved(&dir);
    consume(&mut worker, agent, &dir);
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker
            .state
            .request(request)
            .unwrap()
            .reply()
            .unwrap()
            .provider_turn_id
            .as_deref(),
        Some("owned")
    );
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .is_none());
    assert!(callbacks::records(&spool).unwrap().is_empty());
    let next = queue(&mut worker, room, agent, "next task");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit",
        "session_id":"session","turn_id":"next","prompt":"next task"}),
    );
    consume(&mut worker, agent, &dir);
    // Even a different callback ID for the old turn cannot complete newer work.
    old["id"] = json!("dcba");
    old["sequence"] = json!(3);
    std::fs::write(
        spool.join("event-dcba.json"),
        serde_json::to_vec(&old).unwrap(),
    )
    .unwrap();
    consume(&mut worker, agent, &dir);
    assert!(worker.state.request(next).unwrap().pending_final.is_none());
    assert_eq!(
        worker
            .state
            .request(next)
            .unwrap()
            .provider_turn_id
            .as_deref(),
        Some("next")
    );
    assert!(callbacks::records(&spool).unwrap().is_empty());
    assert!(!matches!(
        worker.state.request(next).unwrap().phase,
        RequestPhase::Completed | RequestPhase::Abandoned
    ));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn empty_stop_retained_abandoned_turn_is_consumed_after_restart_without_rebinding() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    let old = queue(&mut worker, room, agent, "old task");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit",
        "session_id":"session","turn_id":"old","prompt":"old task"}),
    );
    consume(&mut worker, agent, &dir);
    let at = io::now_ms();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, at)
        .unwrap();
    assert_eq!(
        worker.state.settle_ended_request(
            agent,
            at + crate::messaging::model::TURN_SETTLE_MS,
            |_| None
        ),
        Some(old)
    );
    assert_eq!(
        worker.state.request(old).unwrap().failure_reason.as_deref(),
        Some("reply_not_captured")
    );
    let next = queue(&mut worker, room, agent, "new task");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit",
        "session_id":"session","turn_id":"new","prompt":"new task"}),
    );
    consume(&mut worker, agent, &dir);
    let spool = dir.join("callbacks/launch");
    let retained = json!({"id":"beef","sequence":3,"at_ms":at,
        "manifest":{"agent_id":agent.0,"provider":"codex","launch_id":"launch"},
        "value":{"hook_event_name":"Stop","session_id":"session","turn_id":"old",
            "transcript_path":"/tmp/retained.jsonl","last_assistant_message":null}});
    std::fs::write(
        spool.join("event-beef.json"),
        serde_json::to_vec(&retained).unwrap(),
    )
    .unwrap();
    drop(worker);
    let mut worker = reopen_saved(&dir);
    consume(&mut worker, agent, &dir);
    let current = worker.state.request(next).unwrap();
    assert_eq!(current.phase, RequestPhase::Active);
    assert_eq!(current.provider_turn_id.as_deref(), Some("new"));
    assert!(current.pending_final.is_none());
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .is_none());
    assert!(callbacks::records(&spool).unwrap().is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn empty_stop_invalid_payload_is_logged_and_retained_once_across_polls_and_restart() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, _, dir, _) = fixture(Provider::Codex, vec![]);
        let invalid = json!({"hook_event_name":"Stop","session_id":"session","turn_id":"bad",
            "last_assistant_message":42});
        record(&dir, Provider::Codex, invalid.clone());
        let spool = dir.join("callbacks/launch");
        let (original, envelope) = callbacks::records(&spool).unwrap().pop().unwrap();
        let bytes = std::fs::read(&original).unwrap();
        for _ in 0..3 {
            consume(&mut worker, agent, &dir);
        }
        drop(worker);
        let mut worker = reopen_saved(&dir);
        // A retry of exactly the same hook must not republish a rejected record.
        record(&dir, Provider::Codex, invalid);
        consume(&mut worker, agent, &dir);
        let logs = capture.text();
        assert_eq!(
            logs.matches("reason=\"invalid_payload\"").count(),
            1,
            "{logs}"
        );
        assert_eq!(
            logs.matches("event=\"bus.callback.observed\"").count(),
            1,
            "{logs}"
        );
        assert!(callbacks::records(&spool).unwrap().is_empty());
        assert_eq!(
            std::fs::read(spool.join(format!("rejected-{}.json", envelope.id))).unwrap(),
            bytes
        );
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_some());
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

fn master_fixture() -> (Worker, AgentId, RoomId, PathBuf) {
    let (mut worker, previous, work, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut state = worker.state.clone();
    let identity = state.agent(previous).unwrap().runtime_identity.clone();
    state.delete_agent(previous, 0).unwrap();
    let master = state.master_room().unwrap().id;
    let agent = state
        .create_agent(master, "lead", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(agent, work).unwrap();
    state.set_agent_runtime_identity(agent, identity).unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    worker.save(state).unwrap();
    callbacks::initialize(
        &dir.join("callbacks/launch"),
        &callbacks::Manifest {
            routing_key: callbacks::RoutingKey(agent.0),
            provider: Provider::ClaudeCode,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    (worker, agent, master, dir)
}

#[test]
fn same_turn_claude_input_is_progress_without_exposing_content() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir) = master_fixture();
        let request = queue(&mut worker, room, agent, "PRIVATE_ORIGINAL");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("turn", "PRIVATE_ORIGINAL"),
        );
        consume(&mut worker, agent, &dir);
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("turn", "PRIVATE_OTHER_INPUT"),
        );
        consume(&mut worker, agent, &dir);
        let logs = capture.text();
        assert!(logs.contains("disposition=AcceptedProgress"), "{logs}");
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(request)
        );
        assert!(worker.state.request(request).unwrap().trusted_start_bound);
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        assert!(!logs.contains("Rejected("), "{logs}");
        let progress = logs
            .lines()
            .find(|line| line.contains("disposition=AcceptedProgress"))
            .unwrap();
        for field in [
            "bus.callback.binding",
            "trusted_start_bound=true",
            "expected_session_id=Some(\"session\")",
            "callback_session_id=Some(\"session\")",
            "expected_turn_id=Some(\"turn\")",
            "callback_turn_id=Some(\"turn\")",
            "expected_prompt_id=Some(\"turn\")",
            "callback_prompt_id=Some(\"turn\")",
            "raw_payload_matches=Some(false)",
            "submission_boundary=Some(0)",
        ] {
            assert!(progress.contains(field), "missing {field}: {progress}");
        }
        assert!(!logs.contains("PRIVATE_ORIGINAL") && !logs.contains("PRIVATE_OTHER_INPUT"));
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("turn", "PRIVATE_REPLY", json!([])),
        );
        consume(&mut worker, agent, &dir);
        // More input in the same owned turn must not clear a captured answer,
        // even after restoring the durable request binding.
        drop(worker);
        let mut worker = reopen_saved(&dir);
        record(
            &dir,
            Provider::ClaudeCode,
            claude_start("turn", "PRIVATE_LATE_INPUT"),
        );
        consume(&mut worker, agent, &dir);
        assert_eq!(
            worker.state.request(request).unwrap().reply().unwrap().text,
            "PRIVATE_REPLY"
        );
        assert!(worker
            .state
            .agent(agent)
            .unwrap()
            .actionable_error
            .is_none());
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        assert_eq!(
            worker.state.request(request).unwrap().phase,
            RequestPhase::Completed
        );
        assert_eq!(
            worker.state.room(room).unwrap().latest_replies[&agent].text,
            "PRIVATE_REPLY"
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn claude_compaction_and_delayed_submit_preserve_request_binding() {
    for compact_before_start in [true, false] {
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| {
            let (mut worker, agent, room, dir) = master_fixture();
            let request = queue(&mut worker, room, agent, "owned prompt");
            worker.submit_ready().unwrap();
            if !compact_before_start {
                record(&dir, Provider::ClaudeCode, claude_start("turn", "owned prompt"));
                consume(&mut worker, agent, &dir);
            }
            for _ in 0..3 {
                record(&dir, Provider::ClaudeCode, json!({"hook_event_name":"SessionStart", "session_id":"session", "source":"compact"}));
                consume(&mut worker, agent, &dir);
            }
            record(&dir, Provider::ClaudeCode, claude_stop("turn", "reply", json!([])));
            consume(&mut worker, agent, &dir);
            if compact_before_start {
                assert_eq!(worker.state.agent(agent).unwrap().current_request, Some(request));
                assert_eq!(callbacks::records(&dir.join("callbacks/launch")).unwrap().len(), 1);
                // Recover a delayed hook after reopening the persisted scratch state.
                drop(worker);
                worker = reopen_saved(&dir);
                record(&dir, Provider::ClaudeCode, claude_start("turn", "owned prompt"));
                consume(&mut worker, agent, &dir);
            }
            worker.state.observe_status(agent, RuntimeStatus::Idle, io::now_ms()).unwrap();
            assert_eq!(worker.state.request(request).unwrap().phase, RequestPhase::Completed);
            assert_eq!(worker.state.room(room).unwrap().latest_replies[&agent].text, "reply");
            assert!(worker.state.agent(agent).unwrap().actionable_error.is_none());
            assert_eq!(worker.state.agent(agent).unwrap().compactions.count, 3);
            assert!(callbacks::records(&dir.join("callbacks/launch")).unwrap().is_empty());
            drop(worker);
            std::fs::remove_dir_all(dir).unwrap();
        });
    }
}

#[test]
fn callback_binding_diagnostics_explain_missing_submit() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir) = master_fixture();
        let request = queue(&mut worker, room, agent, "PRIVATE_PROMPT");
        worker.submit_ready().unwrap();
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("turn", "PRIVATE_REPLY", json!([])),
        );
        consume(&mut worker, agent, &dir);
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(request)
        );
        assert!(!worker.state.request(request).unwrap().trusted_start_bound);
        let logs = capture.text();
        let deferred = logs
            .lines()
            .find(|line| line.contains("trusted_start_missing"))
            .unwrap();
        for field in [
            "bus.callback.binding",
            "trusted_start_bound=false",
            "expected_turn_id=None",
            "callback_turn_id=Some(\"turn\")",
            "compactions=0",
        ] {
            assert!(deferred.contains(field), "missing {field}: {deferred}");
        }
        assert!(!logs.contains("PRIVATE_PROMPT") && !logs.contains("PRIVATE_REPLY"));
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn changed_claude_session_preserves_the_active_request_and_logs_both_sessions() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        let (mut worker, agent, room, dir) = master_fixture();
        let request = queue(&mut worker, room, agent, "owned prompt");
        worker.submit_ready().unwrap();
        record(&dir, Provider::ClaudeCode, claude_start("turn", "owned prompt"));
        consume(&mut worker, agent, &dir);
        record(&dir, Provider::ClaudeCode, json!({"hook_event_name":"SessionStart", "session_id":"changed-session", "source":"compact"}));
        consume(&mut worker, agent, &dir);
        let owner = worker.state.agent(agent).unwrap();
        assert_eq!(owner.current_request, Some(request));
        assert_eq!(owner.runtime_identity.session_id.as_deref(), Some("session"));
        assert!(owner.session_binding_invalidated);
        assert_eq!(worker.state.request(request).unwrap().provider_session_id.as_deref(), Some("session"));
        let logs = capture.text();
        let mismatch = logs.lines().find(|line| line.contains("session_mismatch")).unwrap();
        assert!(mismatch.contains("expected_session_id=Some(\"session\")"), "{mismatch}");
        assert!(mismatch.contains("callback_session_id=changed-session"), "{mismatch}");
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    });
}

/// A work-room Claude worker whose room orchestrator sends it async requests.
fn orchestrated_worker() -> (Worker, AgentId, AgentId, RoomId, PathBuf) {
    struct ConfirmedPrompts(FakeTransport);
    impl Transport for ConfirmedPrompts {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if matches!(
                method,
                Method::AgentPromptIfIdle(_) | Method::AgentPromptIfUnbound(_)
            ) {
                return Ok(ResponseResult::AgentPrompted {
                    agent: native_info("pane", "claude", "session"),
                });
            }
            self.0.request(method)
        }
    }
    let (mut worker, agent, room, dir, calls) = fixture(Provider::ClaudeCode, vec![]);
    worker.transport = Box::new(ConfirmedPrompts(FakeTransport {
        replies: VecDeque::new(),
        calls,
        state_path: dir.join("state.json"),
    }));
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let orch = state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(orch, room).unwrap();
    worker.save(state).unwrap();
    (worker, agent, orch, room, dir)
}

fn send_from(
    worker: &mut Worker,
    room: RoomId,
    orch: AgentId,
    agent: AgentId,
    text: &str,
) -> RequestId {
    let mut state = worker.state.clone();
    let id = state
        .submit_message_from(
            room,
            crate::messaging::model::Draft {
                text: text.into(),
                files: vec![],
                recipient_ids: [agent].into(),
            },
            crate::messaging::model::Author::Agent(orch),
            io::now_ms() - 4_000,
        )
        .unwrap()[0];
    worker.save(state).unwrap();
    id
}

fn orchestrator_notices(worker: &Worker, orch: AgentId, agent: AgentId) -> Vec<String> {
    worker
        .state
        .requests()
        .filter(|r| {
            r.agent_id == orch && r.prompt.author == crate::messaging::model::Author::Agent(agent)
        })
        .map(|r| r.prompt.text.clone())
        .collect()
}

fn route_at(worker: &mut Worker, at_ms: u64) {
    let mut state = worker.state.clone();
    state
        .route_worker_replies(at_ms, &std::collections::BTreeSet::new())
        .unwrap();
    worker.save(state).unwrap();
}

// These hooks report the exact attributed input typed into the worker, as a
// real provider does; the late-final assertions below remain unchanged.
fn authored_start(worker: &Worker, author: AgentId, turn: &str, text: &str) -> serde_json::Value {
    let sender = worker.state.agent(author).unwrap();
    claude_start(
        turn,
        &format!("{} (room {}):\n{text}", sender.name, sender.room_id.0),
    )
}

/// The request completes on an interim Stop (a Codex-style Stop without a
/// task census) and the async sender has already seen that interim.
fn completed_on_interim(
    worker: &mut Worker,
    agent: AgentId,
    orch: AgentId,
    room: RoomId,
    dir: &Path,
) -> RequestId {
    let source = send_from(worker, room, orch, agent, "open work");
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(source)
    );
    record(
        dir,
        Provider::ClaudeCode,
        authored_start(worker, orch, "p1", "open work"),
    );
    consume(worker, agent, dir);
    record(
        dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Waiting for CI.", json!([])),
    );
    consume(worker, agent, dir);
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    assert_eq!(
        worker.state.request(source).unwrap().phase,
        RequestPhase::Completed
    );
    let mut state = worker.state.clone();
    state.see_worker_reply(source, orch, io::now_ms()).unwrap();
    worker.save(state).unwrap();
    source
}

#[test]
fn late_changed_final_after_newer_typed_request_reaches_sender_without_touching_new_request() {
    for next_started in [false, true] {
        let (mut worker, agent, orch, room, dir) = orchestrated_worker();
        let source = completed_on_interim(&mut worker, agent, orch, room, &dir);
        let next = send_from(&mut worker, room, orch, agent, "second work");
        worker.submit_ready().unwrap();
        // The fake transport's bare Ok is not an AgentPrompted reply; clear
        // that harness-only error so only callback handling can set one.
        let mut state = worker.state.clone();
        state.set_agent_error(agent, None).unwrap();
        worker.save(state).unwrap();
        assert_eq!(
            worker.state.agent(agent).unwrap().current_request,
            Some(next)
        );
        if next_started {
            record(
                &dir,
                Provider::ClaudeCode,
                authored_start(&worker, orch, "p2", "second work"),
            );
            consume(&mut worker, agent, &dir);
            assert!(worker.state.request(next).unwrap().trusted_start_bound);
        }
        // The old turn's real final is spooled after Bus typed the newer request.
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("p1", "CI passed: real result.", json!([])),
        );
        consume(&mut worker, agent, &dir);
        assert!(
            callbacks::records(&dir.join("callbacks/launch"))
                .unwrap()
                .is_empty(),
            "next_started={next_started}: late final must not stay spooled"
        );
        assert!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .actionable_error
                .is_none(),
            "next_started={next_started}: {:?}",
            worker.state.agent(agent).unwrap().actionable_error
        );
        assert!(worker.state.request(next).unwrap().pending_final.is_none());
        route_at(&mut worker, io::now_ms() + 10_000);
        let notices = orchestrator_notices(&worker, orch, agent);
        assert_eq!(notices.len(), 1, "next_started={next_started}: {notices:?}");
        assert!(notices[0].contains(&format!(
            "Late reply to message {}",
            worker.state.request(source).unwrap().prompt.id.0
        )));
        assert!(notices[0].contains("CI passed: real result."));
        // The newer request still binds and answers with its own turn.
        if !next_started {
            record(
                &dir,
                Provider::ClaudeCode,
                authored_start(&worker, orch, "p2", "second work"),
            );
            consume(&mut worker, agent, &dir);
        }
        record(
            &dir,
            Provider::ClaudeCode,
            claude_stop("p2", "Second reply.", json!([])),
        );
        consume(&mut worker, agent, &dir);
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        let next_request = worker.state.request(next).unwrap();
        assert_eq!(
            next_request.phase,
            RequestPhase::Completed,
            "next_started={next_started}"
        );
        assert_eq!(next_request.reply().unwrap().text, "Second reply.");
        assert_eq!(
            worker.state.request(source).unwrap().reply().unwrap().text,
            "Waiting for CI."
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn successive_late_finals_after_completion_each_wake_the_sender_once() {
    let (mut worker, agent, orch, room, dir) = orchestrated_worker();
    completed_on_interim(&mut worker, agent, orch, room, &dir);
    let now = io::now_ms();
    record(
        &dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Still waiting for the reviewer.", json!([])),
    );
    consume(&mut worker, agent, &dir);
    route_at(&mut worker, now + 10_000);
    assert_eq!(orchestrator_notices(&worker, orch, agent).len(), 1);
    record(
        &dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Reviewer approved.", json!([])),
    );
    // The identical interim and a repeat of an already-delivered update are not news.
    record(
        &dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Still waiting for the reviewer.", json!([])),
    );
    record(
        &dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Waiting for CI.", json!([])),
    );
    consume(&mut worker, agent, &dir);
    route_at(&mut worker, now + 30_000);
    let notices = orchestrator_notices(&worker, orch, agent);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[1].contains("Reviewer approved."));
    assert!(!notices[1].contains("Still waiting") && !notices[1].contains("Waiting for CI."));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn abandoned_request_does_not_spend_its_turn_on_a_first_interim_late_final() {
    let (mut worker, agent, orch, room, dir) = orchestrated_worker();
    let source = send_from(&mut worker, room, orch, agent, "open work");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        authored_start(&worker, orch, "p1", "open work"),
    );
    consume(&mut worker, agent, &dir);
    let now = io::now_ms();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, now - 9_000)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, now - 8_000)
        .unwrap();
    let mut state = worker.state.clone();
    assert_eq!(
        state.settle_ended_request(agent, now, |_| None),
        Some(source)
    );
    worker.save(state).unwrap();
    assert_eq!(
        worker.state.request(source).unwrap().phase,
        RequestPhase::Abandoned
    );
    record(
        &dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Still waiting for the reviewer.", json!([])),
    );
    consume(&mut worker, agent, &dir);
    route_at(&mut worker, now + 10_000);
    record(
        &dir,
        Provider::ClaudeCode,
        claude_stop("p1", "Reviewer approved.", json!([])),
    );
    consume(&mut worker, agent, &dir);
    route_at(&mut worker, now + 30_000);
    let notices = orchestrator_notices(&worker, orch, agent);
    assert!(
        notices.iter().any(|n| n.contains("Reviewer approved.")),
        "the later changed final of an abandoned turn was dropped: {notices:?}"
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
