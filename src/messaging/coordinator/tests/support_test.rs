use super::*;

pub(super) static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct FakeTransport {
    pub(super) replies: VecDeque<Result<ResponseResult, TransportError>>,
    pub(super) calls: Arc<Mutex<Vec<&'static str>>>,
    pub(super) state_path: PathBuf,
}

impl Transport for FakeTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.calls
            .lock()
            .unwrap()
            .push(crate::protocol::api::api_method_name(&method));
        if let Method::PaneCloseIfIdentity(params) = &method {
            let state = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            let agent = state
                .agents()
                .find(|agent| {
                    agent.runtime_identity.terminal_id.as_deref()
                        == Some(&params.expected_terminal_id)
                })
                .unwrap();
            assert!(
                agent.deletion_pending,
                "delivery must stop durably before terminal shutdown"
            );
            assert_eq!(
                params.pane_id,
                agent.runtime_identity.pane_id.clone().unwrap()
            );
            assert_eq!(
                params.expected_session_id,
                agent.runtime_identity.session_id
            );
            assert_eq!(
                params.expected_managed_name,
                format!("bus-r{}-a{}", agent.room_id.0, agent.id.0)
            );
        }
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
        if let Method::PaneReportAgentSession(params) = &method {
            if let Some(session) = &params.agent_session_id {
                REPORTED_SESSION.set(Some((params.agent.clone(), session.clone())));
            }
        }
        if let (Method::AgentGet(target), None, Some((agent, session))) = (
            &method,
            self.replies.front(),
            REPORTED_SESSION.with_borrow(Clone::clone),
        ) {
            return Ok(ResponseResult::AgentInfo {
                agent: native_info(&target.target, &agent, &session),
            });
        }
        self.replies
            .pop_front()
            .unwrap_or(Ok(ResponseResult::Ok {}))
    }
}

thread_local! {
    /// The last session reported to the fake server; `agent.get` answers with
    /// it when no reply is queued, as a server that accepts the report would.
    static REPORTED_SESSION: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn native_info(
    pane: &str,
    agent: &str,
    session: &str,
) -> crate::protocol::api::schema::AgentInfo {
    serde_json::from_value(json!({
        "terminal_id": "terminal",
        "pane_id": pane,
        "name": "bus-r1-a2",
        "agent": agent,
        "agent_status": "idle",
        "agent_session": {
            "source": format!("herdr:{agent}"),
            "agent": agent,
            "kind": "id",
            "value": session
        },
        "workspace_id": "w1",
        "tab_id": "t1",
        "focused": false,
        "revision": 1
    }))
    .unwrap()
}

pub(super) fn fixture(
    provider: Provider,
    replies: Vec<Result<ResponseResult, TransportError>>,
) -> (
    Worker,
    AgentId,
    RoomId,
    PathBuf,
    Arc<Mutex<Vec<&'static str>>>,
) {
    let dir = std::env::temp_dir().join(format!(
        "bus-worker-{}-{}-{}",
        std::process::id(),
        io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
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
            routing_key: callbacks::RoutingKey(agent.0),
            provider,
            launch_id: "launch".into(),
        },
    )
    .unwrap();
    (worker, agent, room, dir, calls)
}

pub(super) fn queue(worker: &mut Worker, room: RoomId, agent: AgentId, text: &str) -> RequestId {
    let mut state = worker.state.clone();
    state.set_draft_text(room, text).unwrap();
    state.set_draft_recipients(room, [agent]).unwrap();
    let request = state.submit_draft(room, io::now_ms()).unwrap()[0];
    worker.save(state).unwrap();
    request
}

pub(super) fn identity_changed() -> Result<ResponseResult, TransportError> {
    Err(TransportError {
        message: "identity changed".into(),
        code: Some("terminal_identity_changed".into()),
        definitely_rejected: true,
    })
}

/// The fixture's terminal as `agent.list` reports it, with or without the Bus managed name.
pub(super) fn native_agents(
    managed_name: Option<String>,
) -> Result<ResponseResult, TransportError> {
    let info = serde_json::from_value(json!({
        "terminal_id":"terminal", "name":managed_name, "agent":"codex", "agent_status":"idle",
        "workspace_id":"workspace", "tab_id":"tab", "pane_id":"pane",
        "focused":false, "revision":1
    }))
    .unwrap();
    Ok(ResponseResult::AgentList { agents: vec![info] })
}

pub(super) fn delete_failure_suspends_while_native_ownership_is_unproven(
    ownership_lookup: Result<ResponseResult, TransportError>,
) {
    let (mut worker, agent, room, dir, calls) =
        fixture(Provider::Codex, vec![identity_changed(), ownership_lookup]);
    assert_eq!((room.0, agent.0), (1, 2), "native_agents names this agent");
    let request = queue(
        &mut worker,
        room,
        agent,
        "must never send after delete confirmation",
    );
    let (events, _) = mpsc::channel();
    assert!(worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .is_err());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    assert!(worker.state.request(request).is_some());
    worker.submit_ready().unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec![
            "pane.close_if_identity",
            "agent.list",
            "agent.get",
            "agent.list"
        ]
    );
    drop(worker);
    let fake = FakeTransport {
        replies: VecDeque::new(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    };
    let mut recovered = Worker::open(dir.clone(), Box::new(fake)).unwrap();
    recovered
        .state
        .observe_status(agent, RuntimeStatus::Idle, 10)
        .unwrap();
    recovered.submit_ready().unwrap();
    assert_eq!(calls.lock().unwrap().len(), 4);
    recovered
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(recovered.state.agent(agent).is_none());
    assert!(recovered.state.request(request).is_none());
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}

pub(super) struct NativeBoundClose {
    pub(super) session: String,
    pub(super) calls: Arc<Mutex<Vec<Option<String>>>>,
    pub(super) state_path: PathBuf,
}

impl Transport for NativeBoundClose {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        if matches!(method, Method::AgentList(_)) {
            // The native server bound a session, but the terminal is still Bus-owned.
            let saved = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            let agent = saved.agents().next().unwrap();
            return native_agents(Some(format!("bus-r{}-a{}", agent.room_id.0, agent.id.0)));
        }
        if let Method::AgentGet(target) = &method {
            let saved = JsonStore::new(self.state_path.clone())
                .load()
                .unwrap()
                .unwrap();
            let kind = crate::agents::providers::launch::provider_kind(
                saved.agents().next().unwrap().provider,
            );
            return Ok(ResponseResult::AgentInfo {
                agent: native_info(&target.target, kind, &self.session),
            });
        }
        let Method::PaneCloseIfIdentity(params) = method else {
            panic!("Deletion must not report native sessions or deliver prompts: {method:?}");
        };
        let saved = JsonStore::new(self.state_path.clone())
            .load()
            .unwrap()
            .unwrap();
        let saved_agent = saved.agents().next().unwrap();
        assert!(saved_agent.deletion_pending);
        assert_eq!(
            saved_agent.runtime_identity.session_id,
            params.expected_session_id
        );
        self.calls
            .lock()
            .unwrap()
            .push(params.expected_session_id.clone());
        if params.expected_session_id.as_deref() == Some(&self.session) {
            Ok(ResponseResult::Ok {})
        } else {
            Err(TransportError {
                message: "terminal_identity_changed".into(),
                code: Some("terminal_identity_changed".into()),
                definitely_rejected: true,
            })
        }
    }
}

pub(super) fn record(dir: &std::path::Path, provider: Provider, mut value: serde_json::Value) {
    if provider == Provider::Codex {
        // Root interactive Codex callbacks have a transcript; explicit null
        // fixtures model its title/memory background sessions instead.
        value
            .as_object_mut()
            .unwrap()
            .entry("transcript_path")
            .or_insert(json!("/tmp/bus-root-transcript.jsonl"));
    }
    callbacks::append(
        &dir.join("callbacks/launch"),
        "launch",
        provider,
        value,
        &[],
    )
    .unwrap();
}

pub(super) fn open_saved_document(document: serde_json::Value) -> (Worker, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "bus-worker-saved-{}-{}-{}",
        std::process::id(),
        io::now_ns(),
        NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("state.json"),
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    let worker = reopen_saved(&dir);
    (worker, dir)
}

pub(super) fn reopen_saved(dir: &std::path::Path) -> Worker {
    Worker::open(
        dir.to_path_buf(),
        Box::new(FakeTransport {
            replies: VecDeque::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
            state_path: dir.join("state.json"),
        }),
    )
    .unwrap()
}

pub(super) fn claude_start(prompt_id: &str, prompt: &str) -> serde_json::Value {
    json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":prompt_id,"prompt":prompt})
}

pub(super) fn claude_stop(
    prompt_id: &str,
    text: &str,
    background: serde_json::Value,
) -> serde_json::Value {
    json!({"hook_event_name":"Stop","session_id":"session","prompt_id":prompt_id,"last_assistant_message":text,"background_tasks":background,"session_crons":[]})
}

/// The turn a background shell's completion starts on its own.
pub(super) fn claude_task_notification_turn(
    prompt_id: &str,
    background: serde_json::Value,
) -> Vec<serde_json::Value> {
    vec![
        json!({"hook_event_name":"SessionStart","session_id":"session","source":"resume"}),
        claude_start(
            prompt_id,
            "<task-notification>\n<task-id>shell-1</task-id>\n<status>completed</status>\n</task-notification>",
        ),
        claude_stop(prompt_id, "Notification handled", background),
    ]
}

pub(super) fn agent_of(worker: &Worker, agent: AgentId) -> &crate::messaging::model::RoomAgent {
    worker.state.agent(agent).unwrap()
}

pub(super) fn claude(event: &str, turn: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut value = json!({"hook_event_name":event,"session_id":"session","prompt_id":turn});
    value
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    value
}

pub(super) fn consume(worker: &mut Worker, agent: AgentId, dir: &Path) {
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
}

/// Bus typed a request; the agent ran turns of its own and never started it.
pub(super) fn unstarted(
    worker: &mut Worker,
    agent: AgentId,
    room: RoomId,
    dir: &Path,
) -> RequestId {
    let request = queue(worker, room, agent, "Lost prompt");
    worker.submit_ready().unwrap();
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    record(
        dir,
        Provider::ClaudeCode,
        claude(
            "UserPromptSubmit",
            "own-1",
            json!({"prompt":"<task-notification>"}),
        ),
    );
    record(
        dir,
        Provider::ClaudeCode,
        claude(
            "Stop",
            "own-1",
            json!({"last_assistant_message":"Own work"}),
        ),
    );
    consume(worker, agent, dir);
    assert_eq!(
        worker.state.agent(agent).unwrap().current_request,
        Some(request)
    );
    request
}
