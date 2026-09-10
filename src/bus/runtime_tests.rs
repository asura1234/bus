use super::*;
use crate::api::schema::{Method, ResponseResult};
use crate::bus::{
    callbacks, io,
    store::JsonStore,
    transport::{Transport, TransportError},
};
use serde_json::json;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct FakeTransport {
    replies: VecDeque<Result<ResponseResult, TransportError>>,
    calls: Arc<Mutex<Vec<&'static str>>>,
    state_path: PathBuf,
}

impl Transport for FakeTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.calls
            .lock()
            .unwrap()
            .push(crate::api::api_method_name(&method));
        if matches!(
            method,
            Method::AgentPromptIfIdle(_) | Method::AgentPromptIfUnbound(_)
        ) {
            let state = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            assert!(state
                .requests()
                .any(|r| r.phase == RequestPhase::Submitting));
        }
        self.replies
            .pop_front()
            .unwrap_or(Ok(ResponseResult::Ok {}))
    }
}

fn fixture(
    provider: Provider,
    replies: Vec<Result<ResponseResult, TransportError>>,
) -> (
    Worker,
    AgentId,
    RoomId,
    PathBuf,
    Arc<Mutex<Vec<&'static str>>>,
) {
    let dir = std::env::temp_dir().join(format!("bus-worker-{}", io::now_ns()));
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeTransport {
        replies: replies.into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut worker = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    let mut state = worker.state.clone();
    let room = state.create_room("room").unwrap();
    let agent = state
        .create_agent(room, "author", provider, dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(agent).unwrap();
    state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
    worker.save(state).unwrap();
    callbacks::initialize(
        &dir.join("callbacks/launch"),
        &callbacks::Manifest {
            agent_id: agent,
            provider,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    (worker, agent, room, dir, calls)
}

fn queue(worker: &mut Worker, room: RoomId, agent: AgentId, text: &str) -> RequestId {
    let mut state = worker.state.clone();
    state.set_draft_text(room, text).unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, 2).unwrap()[0];
    worker.save(state).unwrap();
    request
}

#[test]
fn codex_first_room_prompt_does_not_wait_for_its_deferred_session_start() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    queue(&mut worker, room, agent, "first real prompt");
    worker.submit_ready().unwrap();
    assert_eq!(*calls.lock().unwrap(), vec!["agent.prompt_if_unbound"]);
    // An uncertain first submit remains owned; it must not bootstrap twice.
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    let recovered_calls = Arc::new(Mutex::new(Vec::new()));
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: recovered_calls.clone(),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    queue(&mut recovered, room, agent, "second must wait");
    recovered.submit_ready().unwrap();
    assert!(recovered_calls.lock().unwrap().is_empty());
    assert!(recovered
        .state
        .agent(agent)
        .unwrap()
        .current_request
        .is_some());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

fn record(dir: &std::path::Path, provider: Provider, mut value: serde_json::Value) {
    if provider == Provider::Codex {
        // Root interactive Codex callbacks have a transcript; explicit null
        // fixtures model its title/memory background sessions instead.
        value
            .as_object_mut()
            .unwrap()
            .entry("transcript_path")
            .or_insert(json!("/tmp/bus-root-transcript.jsonl"));
    }
    callbacks::append(&dir.join("callbacks/launch"), "launch", provider, value).unwrap();
}

#[test]
fn deferred_codex_start_and_final_survive_background_sessions_without_rebinding() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "first room prompt");
    worker.submit_ready().unwrap();
    for value in [
        json!({"hook_event_name":"SessionStart","session_id":"title-session","transcript_path":null}),
        json!({"hook_event_name":"SessionStart","session_id":"root-session"}),
        json!({"hook_event_name":"UserPromptSubmit","session_id":"root-session","turn_id":"root-turn","prompt":"first room prompt"}),
        json!({"hook_event_name":"Stop","session_id":"root-session","turn_id":"root-turn","last_assistant_message":"ROOT_FINAL"}),
        json!({"hook_event_name":"SessionStart","session_id":"memory-session","transcript_path":null}),
        json!({"hook_event_name":"Stop","session_id":"title-session","turn_id":"title-turn","last_assistant_message":"wrong title","transcript_path":null}),
    ] {
        record(&dir, Provider::Codex, value);
    }
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "ROOT_FINAL"
    );
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("root-session")
    );
    assert!(
        !worker
            .state
            .agent(agent)
            .unwrap()
            .session_binding_invalidated
    );
    worker.submit_ready().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["agent.prompt_if_unbound", "pane.report_agent_session"]
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn session_start_reports_bind_through_native_provider_session_adapter() {
    struct NativeSessionTransport(Arc<Mutex<Option<crate::agent_resume::AgentSessionRef>>>);
    impl Transport for NativeSessionTransport {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            if let Method::PaneReportAgentSession(params) = method {
                // The real API returns Ok even if this consumer rejects the source.
                // A transport-only mock would miss a silently discarded identity.
                *self.0.lock().unwrap() = crate::agent_resume::session_ref_from_report(
                    &params.source,
                    &params.agent,
                    params.agent_session_id,
                    params.agent_session_path,
                );
            }
            Ok(ResponseResult::Ok {})
        }
    }
    for provider in [Provider::Codex, Provider::ClaudeCode, Provider::Cursor] {
        let (mut worker, agent, _, dir, _) = fixture(provider, vec![]);
        let native_session = Arc::new(Mutex::new(None));
        worker.transport = Box::new(NativeSessionTransport(native_session.clone()));
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        let callback = if provider == Provider::Cursor {
            json!({"hook_event_name":"sessionStart","conversation_id":"native-session"})
        } else {
            json!({"hook_event_name":"SessionStart","session_id":"native-session"})
        };
        record(&dir, provider, callback);
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
        assert_eq!(
            native_session
                .lock()
                .unwrap()
                .as_ref()
                .map(|session| session.value.as_str()),
            Some("native-session"),
            "native session adapter must retain {provider:?} identity"
        );
        assert_eq!(
            worker
                .state
                .agent(agent)
                .unwrap()
                .runtime_identity
                .session_id
                .as_deref(),
            Some("native-session")
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn failed_draft_command_keeps_its_outcome_after_later_success() {
    // Removing command-specific results must fail: an aggregate acknowledgement
    // cannot tell the shell whether it may discard a pending local draft edit.
    let (worker, _, room, dir, _) = fixture(Provider::Codex, vec![]);
    let snapshots = Arc::new(Mutex::new(Arc::new(worker.snapshot())));
    let (commands, receiver) = mpsc::sync_channel(8);
    let (events, outcomes) = mpsc::channel();
    commands
        .send((
            11,
            BusCommand::SetDraftText(RoomId(u64::MAX), "unsaved".into()),
        ))
        .unwrap();
    commands
        .send((12, BusCommand::SetNotes(room, "saved notes".into())))
        .unwrap();
    drop(commands);
    worker.run(receiver, events, Arc::clone(&snapshots));
    let results: Vec<_> = outcomes
        .try_iter()
        .filter_map(|event| match event {
            BusEvent::CommandFinished { command_id, result } => Some((command_id, result)),
            _ => None,
        })
        .collect();
    assert_eq!(
        results.len(),
        2,
        "each processed command needs its own outcome"
    );
    assert_eq!(results[0].0, 11);
    assert!(results[0].1.is_err());
    assert_eq!(results[1], (12, Ok(())));
    let snapshot = snapshots.lock().unwrap();
    assert_eq!(snapshot.last_command_id, 12);
    assert_eq!(snapshot.state.room(room).unwrap().notes, "saved notes");
    drop(snapshot);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn initial_recovery_tick_keeps_detached_room_reply_unread_until_selected() {
    // A saved visible room belongs to the detached client. The first coordinator
    // tick must not mark its newly spooled reply seen before any UI selection.
    let (mut worker, agent, room_b, dir, _) = fixture(Provider::Codex, vec![]);
    let room_a = worker.state.create_room("other room").unwrap();
    worker.state.select_room(room_b).unwrap();
    let request = queue(&mut worker, room_b, agent, "finish while detached");
    worker.state.begin_submission(request, "launch", 0).unwrap();
    worker
        .state
        .record_submission(
            request,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("session".into()),
                provider_turn_id: Some("turn".into()),
            },
        )
        .unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"turn","prompt":"finish while detached"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.request(request).unwrap().trusted_start_bound);
    assert_eq!(worker.state.room(room_b).unwrap().unread_count, 0);
    drop(worker);
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"turn","last_assistant_message":"completed while detached"}),
    );
    let info = serde_json::from_value(json!({
        "terminal_id": "terminal", "agent": "codex", "agent_status": "idle",
        "agent_session": {"source": "herdr:codex", "agent": "codex", "kind": "id", "value": "session"},
        "workspace_id": "workspace", "tab_id": "tab", "pane_id": "pane",
        "focused": false, "interactive_ready": true, "revision": 1
    }))
    .unwrap();
    let fake = FakeTransport {
        replies: vec![Ok(ResponseResult::AgentList { agents: vec![info] })].into(),
        calls: Arc::new(Mutex::new(Vec::new())),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered.tick().unwrap();
    assert_eq!(
        recovered.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        recovered.state.room(room_b).unwrap().latest_replies[&agent].text,
        "completed while detached"
    );
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    let (events, _) = mpsc::channel();
    recovered
        .command(BusCommand::SelectRoom(room_a), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 1);
    assert_eq!(
        recovered
            .store
            .load()
            .unwrap()
            .unwrap()
            .room(room_b)
            .unwrap()
            .unread_count,
        1
    );
    recovered
        .command(BusCommand::SelectRoom(room_b), &events)
        .unwrap();
    assert_eq!(recovered.state.room(room_b).unwrap().unread_count, 0);
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_submission_survives_restart_and_never_retries() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "lost response".into(),
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    let next = queue(&mut worker, room, agent, "next");
    worker.submit_ready().unwrap();
    assert!(worker.state.request(request).unwrap().uncertain_outcome);
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    let (transport_calls, no_replies) = (Arc::new(Mutex::new(Vec::new())), VecDeque::new());
    let fake = FakeTransport {
        replies: no_replies,
        calls: transport_calls.clone(),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 3)
        .unwrap();
    recovered.submit_ready().unwrap();
    assert!(transport_calls.lock().unwrap().is_empty());
    assert_eq!(recovered.state.queued_requests(agent), &[next]);
    assert_eq!(
        recovered.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn uncertain_submission_rejects_wrong_session_start_binding() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "lost response".into(),
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"wrong-session","turn_id":"wrong-turn","prompt":"same"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"wrong-session","turn_id":"wrong-turn","last_assistant_message":"wrong reply"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(!worker.state.request(request).unwrap().trusted_start_bound);
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn changed_session_start_cannot_erase_and_rebind_known_identity() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "held after session changed");
    for _ in 0..2 {
        record(
            &dir,
            Provider::Codex,
            json!({"hook_event_name":"SessionStart","session_id":"other-session"}),
        );
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("session")
    );
    assert!(calls.lock().unwrap().is_empty());
    // A stale server identity/Idle observation must not reopen this launch.
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.state.confirm_hook_setup(agent).is_err());
    worker.save(worker.state.clone()).unwrap();
    let mut reloaded = worker.store.load().unwrap().unwrap();
    assert!(reloaded.agent(agent).unwrap().session_binding_invalidated);
    assert!(reloaded.confirm_hook_setup(agent).is_err());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn busy_rejection_restores_exact_fifo_and_blocked_does_not_submit() {
    let (mut worker, agent, room, dir, calls) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "busy".into(),
            definitely_rejected: true,
        })],
    );
    let first = queue(&mut worker, room, agent, "first");
    let second = queue(&mut worker, room, agent, "second");
    worker.submit_ready().unwrap();
    assert_eq!(worker.state.queued_requests(agent), &[first, second]);
    assert!(worker.state.agent(agent).unwrap().current_request.is_none());
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 3)
        .unwrap();
    worker.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cursor_stop_before_response_and_start_survives_detach_without_duplicate_reply() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Cursor,
        vec![Err(TransportError {
            message: "unknown".into(),
            definitely_rejected: false,
        })],
    );
    let request = queue(&mut worker, room, agent, "same");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn","text":"real final"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    assert_eq!(
        callbacks::records(&dir.join("callbacks/launch"))
            .unwrap()
            .len(),
        2
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":"turn","prompt":"same"}),
    );
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 4)
        .unwrap();
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    worker
        .state
        .observe_status(agent, RuntimeStatus::Working, 5)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Blocked, 6)
        .unwrap();
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 7)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "real final"
    );
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    // Replayed raw callbacks cannot increment unread again after a crash/delete boundary.
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"stop","conversation_id":"session","generation_id":"turn","status":"completed"}),
    );
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":"turn","text":"real final"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(worker.state.room(room).unwrap().unread_count, 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn storage_failure_prevents_send_and_startup_failure_releases_coordinator() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::Codex, vec![]);
    queue(&mut worker, room, agent, "prompt");
    io::atomic_write(&dir.join("state.json"), b"corrupt").unwrap();
    assert!(worker.submit_ready().is_err());
    assert!(calls.lock().unwrap().is_empty());
    assert!(worker.storage_failed);
    drop(worker);
    let fake = || {
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: calls.clone(),
            state_path: dir.join("state.json"),
        })
    };
    assert!(Worker::open(dir.clone(), fake()).is_err());
    assert!(io::lock(&dir.join("coordinator.lock")).is_ok());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn late_old_identical_codex_final_cannot_bind_and_provider_error_holds_queue() {
    let (mut worker, agent, room, dir, _) = fixture(
        Provider::Codex,
        vec![Err(TransportError {
            message: "unknown".into(),
            definitely_rejected: false,
        })],
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"old","prompt":"same"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let request = queue(&mut worker, room, agent, "same");
    let next = queue(&mut worker, room, agent, "next");
    worker.submit_ready().unwrap();
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"Stop","session_id":"session","turn_id":"old","last_assistant_message":"stale same prompt final"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":"current","prompt":"same"}),
    );
    record(
        &dir,
        Provider::Codex,
        json!({"hook_event_name":"StopFailure","session_id":"session","turn_id":"current"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    assert_eq!(worker.state.queued_requests(agent), &[next]);
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .actionable_error
        .as_ref()
        .unwrap()
        .contains("failed"));
    assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn claude_still_requires_real_session_start_before_submission() {
    let (mut worker, agent, room, dir, calls) = fixture(Provider::ClaudeCode, vec![]);
    queue(&mut worker, room, agent, "queued");
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.save(worker.state.clone()).unwrap();
    worker.submit_ready().unwrap();
    assert!(calls.lock().unwrap().is_empty());
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"fresh-session"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    assert_eq!(
        worker
            .state
            .agent(agent)
            .unwrap()
            .runtime_identity
            .session_id
            .as_deref(),
        Some("fresh-session")
    );
    assert_eq!(*calls.lock().unwrap(), vec!["pane.report_agent_session"]);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
