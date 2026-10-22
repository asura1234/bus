//! Integration boundary: real App API, terminal submission writer, callback spool,
//! coordinator tick, journal and history. Only the provider's terminal is scripted.
use super::*;
use crate::agents::{AgentKind, AgentState};
use crate::messaging::{control::Request as ControlRequest, native::TransportError};
use crate::server::{
    app::{App, AppPolicy},
    workspaces::Workspace,
};
use crate::terminal::TerminalRuntime;
use serde_json::{json, Value};
use std::time::Instant;
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn sequence() -> u64 {
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn screen(provider: Provider, draft: &str) -> String {
    let body = match provider {
        Provider::ClaudeCode => {
            format!("────────────────────────\r\n❯ {draft}\r\n────────────────────────")
        }
        Provider::Cursor => format!(" > {draft}\r\n / commands · @ files"),
        _ => format!("› {draft}\r\n\r\n GPT-6 · /scratch\r\n ? for shortcuts"),
    };
    format!("\x1b[?2004h\x1b[2J\x1b[H{body}")
}

struct Terminal {
    pane: crate::utils::ids::PaneId,
    provider: Provider,
    input: tokio::sync::mpsc::Receiver<bytes::Bytes>,
    draft: String,
    images: Vec<String>,
    submitted: Vec<String>,
    submitted_images: Vec<Vec<String>>,
    captured_screen: Option<String>,
    clock: crate::terminal::runtime::SubmissionClock,
}

struct NativeServer {
    app: App,
    terminals: BTreeMap<AgentId, Terminal>,
    next_pump_pause: Duration,
    last_submission_response: Option<Value>,
}

impl NativeServer {
    fn new() -> Self {
        let (_, rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            app: App::new(
                &crate::utils::config::Config::default(),
                AppPolicy::TEST,
                None,
                rx,
                crate::server::api::EventHub::default(),
            ),
            terminals: BTreeMap::new(),
            next_pump_pause: Duration::ZERO,
            last_submission_response: None,
        }
    }

    fn add(&mut self, id: AgentId, room: RoomId, provider: Provider) -> AgentRuntimeIdentity {
        let ws = Workspace::test_new(&format!("reply-{}", id.0));
        let pane = ws.tabs[0].root_pane;
        let terminal_id = ws.tabs[0].panes[&pane].attached_terminal_id.clone();
        self.app.state.workspaces.push(ws);
        self.app.state.ensure_test_terminals();
        let kind = match provider {
            crate::agents::providers::ProviderKind::Grok => {
                panic!("This fixture covers the original providers; use dedicated Grok lane tests")
            }
            Provider::ClaudeCode => AgentKind::Claude,
            Provider::Codex => AgentKind::Codex,
            Provider::Cursor => AgentKind::Cursor,
        };
        let terminal = self.app.state.terminals.get_mut(&terminal_id).unwrap();
        let now = Instant::now();
        terminal.begin_managed_agent(
            format!("bus-r{}-a{}", room.0, id.0),
            kind,
            now,
            Duration::ZERO,
            Duration::from_secs(10),
        );
        terminal.set_detected_state(Some(kind), AgentState::Idle);
        terminal.reconcile_managed_agent_at(now + Duration::from_secs(1), false);
        let label = crate::agents::agent_label(kind);
        terminal.set_agent_session_ref_for_session_start(
            "bus".into(),
            label.into(),
            crate::agents::resume::catalog::AgentSessionRef::id(format!("session-{}", id.0)),
            Some(1),
            None,
            &[],
        );
        let (runtime, input) = TerminalRuntime::test_with_channel_capacity(240, 32, 32);
        let clock = runtime.test_submission_clock().unwrap();
        clock.pause();
        runtime.test_process_pty_bytes(screen(provider, "").as_bytes());
        self.app.state.insert_test_runtime(pane, runtime);
        let ResponseResult::AgentList { agents } =
            self.request(Method::AgentList(Default::default())).unwrap()
        else {
            panic!("agent.list")
        };
        let info = agents
            .into_iter()
            .find(|a| a.terminal_id == terminal_id.as_str())
            .unwrap();
        self.terminals.insert(
            id,
            Terminal {
                pane,
                provider,
                input,
                draft: String::new(),
                images: Vec::new(),
                submitted: Vec::new(),
                submitted_images: Vec::new(),
                captured_screen: None,
                clock,
            },
        );
        AgentRuntimeIdentity {
            launch_id: Some(format!("launch-{}", id.0)),
            terminal_id: Some(info.terminal_id),
            pane_id: Some(info.pane_id),
            session_id: Some(format!("session-{}", id.0)),
        }
    }

    fn pump(&mut self) {
        for terminal in self.terminals.values_mut() {
            while let Ok(bytes) = terminal.input.try_recv() {
                let text = std::str::from_utf8(&bytes).unwrap();
                if text.starts_with("\x1b[200~") {
                    let mut remaining = text;
                    while let Some(paste) = remaining.strip_prefix("\x1b[200~") {
                        let (paste, rest) = paste.split_once("\x1b[201~").unwrap();
                        // Codex attaches only an image pasted on its own. This
                        // oracle reads the image, independently of our matcher.
                        let image = paste.strip_prefix('"').and_then(|s| s.strip_suffix('"'));
                        if let Some(image) = image.filter(|path| {
                            terminal.provider == Provider::Codex
                                && image::image_dimensions(path).is_ok()
                        }) {
                            terminal.images.push(image.to_owned());
                        } else {
                            terminal.draft.push_str(paste);
                        }
                        remaining = rest;
                    }
                    assert!(remaining.is_empty(), "unexpected paste suffix");
                } else if text == "\r" {
                    let images = std::mem::take(&mut terminal.images);
                    let chips = (1..=images.len())
                        .map(|n| format!("[Image #{n}] "))
                        .collect::<String>();
                    terminal
                        .submitted
                        .push(format!("{chips}{}", std::mem::take(&mut terminal.draft)));
                    terminal.submitted_images.push(images);
                } else if text == "\x0b" {
                    terminal.draft.clear();
                    terminal.images.clear();
                } else {
                    assert_eq!(text, "\x01", "unexpected terminal input");
                }
                // An independently captured Claude composer is the image oracle;
                // never transform the draft with the observer's production rule.
                let display = if let Some(captured) = terminal
                    .captured_screen
                    .as_ref()
                    .filter(|_| !terminal.draft.is_empty() || !terminal.images.is_empty())
                {
                    format!("\x1b[2J\x1b[H{captured}")
                } else {
                    let chips = (1..=terminal.images.len())
                        .map(|n| format!("[Image #{n}] "))
                        .collect::<String>();
                    screen(terminal.provider, &format!("{chips}{}", terminal.draft))
                };
                self.app
                    .state
                    .runtime_for_pane_in_workspace(
                        &self.app.terminal_runtimes,
                        self.app
                            .state
                            .workspaces
                            .iter()
                            .position(|w| w.terminal_id(terminal.pane).is_some())
                            .unwrap(),
                        terminal.pane,
                    )
                    .unwrap()
                    .test_process_pty_bytes(display.as_bytes());
            }
        }
    }

    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let request = crate::protocol::api::schema::Request {
            id: "reply-integration".into(),
            method,
        };
        let (tx, rx) = mpsc::channel();
        let submission = crate::protocol::api::submission::method_budget(&request.method).is_some();
        let encoded = if self
            .app
            .handle_deferred_agent_api_request(request.clone(), tx)
        {
            let pause = std::mem::take(&mut self.next_pump_pause);
            if !pause.is_zero() {
                // Fault injection: model the fake provider losing CPU time.
                std::thread::sleep(pause);
            }
            let deadline = Instant::now() + crate::terminal::runtime::NATIVE_TEST_TIMEOUT;
            loop {
                self.pump();
                if let Ok(response) = rx.try_recv() {
                    break response;
                }
                assert!(
                    Instant::now() < deadline,
                    "native server submission did not finish"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        } else {
            self.app.handle_api_request(request)
        };
        let response: Value = serde_json::from_str(&encoded).unwrap();
        if submission {
            self.last_submission_response = Some(response.clone());
        }
        crate::protocol::api::client::parse_response_value(response)
            .map(|r| r.result)
            .map_err(|error| TransportError {
                code: match &error {
                    crate::protocol::api::client::ApiClientError::ErrorResponse(r) => {
                        Some(r.error.code.clone())
                    }
                    _ => None,
                },
                message: error.to_string(),
                definitely_rejected: true,
            })
    }
}

struct Native(Arc<Mutex<NativeServer>>);
impl Transport for Native {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.0.lock().unwrap().request(method)
    }
}

// The provider and its submission clock are scripted. Leave time for the
// fixture to be scheduled; production/native budget regressions use the real
// transport separately. Non-submission RPCs retain their real socket deadlines.
#[cfg(unix)]
struct FixtureSocketTransport(BusTransport);

#[cfg(unix)]
impl Transport for FixtureSocketTransport {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.0.request_using(method, |client, request, timeout| {
            let timeout =
                if crate::protocol::api::submission::method_budget(&request.method).is_some() {
                    crate::terminal::runtime::NATIVE_TEST_TIMEOUT
                } else {
                    timeout
                };
            client.request_value_with_timeout(request, timeout)
        })
    }
}

#[cfg(unix)]
struct SocketServer {
    path: PathBuf,
    socket: Option<crate::server::api::socket::reply_delivery_socket::Socket>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    paused: Arc<std::sync::atomic::AtomicBool>,
    app: Option<std::thread::JoinHandle<()>>,
}
#[cfg(unix)]
impl SocketServer {
    fn new(path: PathBuf, native: Arc<Mutex<NativeServer>>) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (tx, mut rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::server::api::ApiRequestMessage>();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        let pause = paused.clone();
        let app = std::thread::spawn(move || {
            while !quit.load(Ordering::Relaxed) {
                if pause.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                if let Ok(message) = rx.try_recv() {
                    let result = native.lock().unwrap().request(message.request.method);
                    let response = match result {
                        Ok(result) => json!({"id":message.request.id,"result":result}),
                        Err(error) => {
                            json!({"id":message.request.id,"error":{"code":error.code.unwrap_or_else(|| "transport".into()),"message":error.message}})
                        }
                    };
                    let _ = message.respond_to.send(response.to_string());
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });
        let socket = Some(
            crate::server::api::socket::reply_delivery_socket::Socket::start(path.clone(), tx),
        );
        Self {
            path,
            socket,
            stop,
            paused,
            app: Some(app),
        }
    }
}
#[cfg(unix)]
impl Drop for SocketServer {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.app.take().unwrap().join().unwrap();
        drop(self.socket.take());
    }
}

pub(crate) struct Delivery {
    worker: Option<Worker>,
    native: Arc<Mutex<NativeServer>>,
    #[cfg(unix)]
    socket: Option<SocketServer>,
    pub(crate) dir: PathBuf,
    pub(crate) work: RoomId,
    pub(crate) master: RoomId,
    pub(crate) agents: Vec<AgentId>,
}

impl Delivery {
    pub(crate) fn set_reply_waiter_time(&mut self, now: Instant) {
        self.worker_mut().test_reply_waiter_now = Some(now);
    }

    pub(crate) fn real_submission_time(&self, agent: AgentId) {
        self.native.lock().unwrap().terminals[&agent].clock.resume();
    }

    pub(crate) fn pause_next_provider_pump(&self, pause: Duration) {
        self.native.lock().unwrap().next_pump_pause = pause;
    }

    pub(crate) fn last_submission_response(&self) -> Option<Value> {
        self.native.lock().unwrap().last_submission_response.clone()
    }

    pub(crate) fn new(providers: &[Provider], in_master: bool) -> Self {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "brd-{}-{}",
            std::process::id(),
            sequence()
        ));
        let native = Arc::new(Mutex::new(NativeServer::new()));
        let mut worker = Worker::open(dir.clone(), Box::new(Native(native.clone()))).unwrap();
        worker.dev_enabled = true;
        let mut state = worker.state.clone();
        let work = state.create_room("work").unwrap();
        let master = state.master_room().unwrap().id;
        let room = if in_master { master } else { work };
        let mut agents = Vec::new();
        for (i, &provider) in providers.iter().enumerate() {
            let id = state
                .create_agent(room, &format!("agent-{i}"), provider, dir.clone(), None)
                .unwrap();
            if in_master && i == 0 {
                state.bind_orchestrator(id, work).unwrap();
            }
            let identity = native.lock().unwrap().add(id, room, provider);
            state.set_agent_runtime_identity(id, identity).unwrap();
            state.confirm_hook_setup(id).unwrap();
            callbacks::initialize(
                &dir.join(format!("callbacks/launch-{}", id.0)),
                &callbacks::Manifest {
                    routing_key: callbacks::RoutingKey(id.0),
                    provider,
                    launch_id: format!("launch-{}", id.0),
                },
            )
            .unwrap();
            agents.push(id);
        }
        worker.save(state).unwrap();
        let mut f = Self {
            worker: Some(worker),
            native,
            #[cfg(unix)]
            socket: None,
            dir,
            work,
            master,
            agents,
        };
        #[cfg(unix)]
        f.use_socket();
        for agent in f.agents.clone() {
            f.record(
                agent,
                json!({"hook_event_name":"SessionStart","source":"startup"}),
            );
        }
        f.tick();
        f
    }

    fn worker(&self) -> &Worker {
        self.worker.as_ref().unwrap()
    }
    fn worker_mut(&mut self) -> &mut Worker {
        self.worker.as_mut().unwrap()
    }
    pub(crate) fn state(&self) -> &BusState {
        &self.worker().state
    }
    pub(crate) fn call(&mut self, method: &str, params: Value) -> Value {
        let response = self.worker_mut().control_response_with_events(
            &ControlRequest {
                id: format!("integration-{}", sequence()),
                method: method.into(),
                params,
            },
            None,
        );
        assert!(response.ok, "{response:?}");
        response.result
    }
    pub(crate) fn send(
        &mut self,
        agent: AgentId,
        room: RoomId,
        text: &str,
        queue: bool,
        author: Option<AgentId>,
    ) -> RequestId {
        let mut params =
            json!({"room":room.0.to_string(),"to":[agent.0.to_string()],"text":text,"queue":queue});
        if let Some(author) = author {
            params["as"] = json!(author.0.to_string());
        }
        let sent = self.call("message.send", params);
        self.state()
            .requests()
            .find(|r| r.prompt.id.0 == sent["message_id"].as_u64().unwrap())
            .unwrap()
            .id
    }
    pub(crate) fn add_owned(
        &mut self,
        room: RoomId,
        provider: Provider,
        name: &str,
        orchestrates: bool,
    ) -> AgentId {
        let mut state = self.state().clone();
        let id = state
            .create_agent(room, name, provider, self.dir.clone(), None)
            .unwrap();
        if orchestrates {
            state.bind_orchestrator(id, self.work).unwrap();
        }
        let identity = self.native.lock().unwrap().add(id, room, provider);
        state.set_agent_runtime_identity(id, identity).unwrap();
        state.confirm_hook_setup(id).unwrap();
        callbacks::initialize(
            &self.dir.join(format!("callbacks/launch-{}", id.0)),
            &callbacks::Manifest {
                routing_key: callbacks::RoutingKey(id.0),
                provider,
                launch_id: format!("launch-{}", id.0),
            },
        )
        .unwrap();
        self.worker_mut().save(state).unwrap();
        self.record(
            id,
            json!({"hook_event_name":"SessionStart","source":"startup"}),
        );
        self.tick();
        id
    }
    pub(crate) fn add_orchestrator(&mut self) -> AgentId {
        self.add_owned(self.master, Provider::ClaudeCode, "orch", true)
    }
    pub(crate) fn blocked_worker_notice(&mut self, orchestrator: AgentId) -> RequestId {
        let worker = self.add_owned(self.work, Provider::Codex, "blocked-worker", false);
        self.draw(
            worker,
            Some(include_str!(
                "../../../../tests/fixtures/codex-update/chooser-0.162.ansi"
            )),
        );
        self.tick();
        self.tick();
        self.state()
            .requests()
            .find(|r| {
                r.agent_id == orchestrator
                    && r.room_id == self.work
                    && r.prompt.author == crate::messaging::model::Author::Bus
                    && r.prompt.text.contains("Blocked, needs help")
            })
            .expect("real dialog poll must route a work-room request to its orchestrator")
            .id
    }
    pub(crate) fn draw(&mut self, agent: AgentId, text: Option<&str>) {
        let server = self.native.lock().unwrap();
        let terminal = &server.terminals[&agent];
        let index = server
            .app
            .state
            .workspaces
            .iter()
            .position(|w| w.terminal_id(terminal.pane).is_some())
            .unwrap();
        let normal = screen(terminal.provider, "");
        server
            .app
            .state
            .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, index, terminal.pane)
            .unwrap()
            .test_process_pty_bytes(text.unwrap_or(&normal).as_bytes());
    }
    pub(crate) fn request_for_message(&self, id: u64) -> RequestId {
        self.state()
            .requests()
            .find(|r| r.prompt.id.0 == id)
            .unwrap()
            .id
    }
    pub(crate) fn nested_session(&self, agent: AgentId) {
        let provider = self.state().agent(agent).unwrap().provider;
        callbacks::append(&self.dir.join(format!("callbacks/launch-{}",agent.0)), &format!("launch-{}",agent.0), provider, json!({"hook_event_name":"SessionStart","session_id":"nested-session","source":"startup","transcript_path":self.dir.join("nested.jsonl")}), &[], true).unwrap();
    }
    pub(crate) fn fail_next_save(&mut self) {
        self.worker_mut()
            .store
            .fail_once_at(crate::messaging::storage::state_store::SaveStage::TempWrite);
    }
    pub(crate) fn failed_tick(&mut self) {
        assert!(self.worker_mut().tick().is_err());
    }
    pub(crate) fn assert_no_stalls(&self) {
        let notices: Vec<_> = self
            .state()
            .notices(self.master)
            .filter(|n| n.text.contains("Agent stalled"))
            .collect();
        assert!(notices.is_empty(), "unexpected stall alerts: {notices:?}");
    }
    pub(crate) fn age_request_progress(&mut self, millis: u64) {
        let mut document = serde_json::to_value(self.state()).unwrap();
        for request in document["requests"].as_object_mut().unwrap().values_mut() {
            for field in ["progress_at_ms", "completed_at_ms"] {
                if let Some(at) = request[field].as_u64() {
                    request[field] = json!(at.saturating_sub(millis));
                }
            }
        }
        if let Some(events) = document
            .get_mut("worker_events")
            .and_then(Value::as_object_mut)
        {
            for event in events.values_mut() {
                if let Some(at) = event["at_ms"].as_u64() {
                    event["at_ms"] = json!(at.saturating_sub(millis));
                }
            }
        }
        for agent in document["agents"].as_object_mut().unwrap().values_mut() {
            if let Some(at) = agent["status_since_ms"].as_u64() {
                agent["status_since_ms"] = json!(at.saturating_sub(millis));
            }
        }
        let state = serde_json::from_value::<BusState>(document)
            .unwrap()
            .with_history(Arc::clone(self.state().history()));
        self.worker_mut().save(state).unwrap();
    }
    /// One durable marker is also the guard for one structured diagnostic.
    pub(crate) fn stall_diagnostics(&self) -> usize {
        serde_json::to_value(self.state()).unwrap()["attention_episodes"]
            .as_array()
            .map_or(0, |episodes| {
                episodes
                    .iter()
                    .filter(|key| key.as_str().is_some_and(|key| key.starts_with("stall:")))
                    .count()
            })
    }
    pub(crate) fn tick(&mut self) {
        self.worker_mut().tick().unwrap();
    }
    pub(crate) fn status(&mut self, agent: AgentId, status: AgentState) {
        let mut server = self.native.lock().unwrap();
        let pane = server.terminals[&agent].pane;
        let id = server
            .app
            .state
            .workspaces
            .iter()
            .find_map(|w| w.terminal_id(pane))
            .unwrap()
            .clone();
        let provider = server.terminals[&agent].provider;
        let kind = match provider {
            crate::agents::providers::ProviderKind::Grok => {
                panic!("This fixture covers the original providers; use dedicated Grok lane tests")
            }
            Provider::ClaudeCode => AgentKind::Claude,
            Provider::Codex => AgentKind::Codex,
            Provider::Cursor => AgentKind::Cursor,
        };
        server
            .app
            .state
            .terminals
            .get_mut(&id)
            .unwrap()
            .set_detected_state(Some(kind), status);
    }
    pub(crate) fn typed(&self, agent: AgentId) -> Vec<String> {
        self.native.lock().unwrap().terminals[&agent]
            .submitted
            .clone()
    }
    pub(crate) fn typed_content(&self, agent: AgentId) -> Vec<String> {
        self.typed(agent)
            .iter()
            .map(|text| super::tests::support::test_prompt_content(text))
            .collect()
    }
    pub(crate) fn attached_images(&self, agent: AgentId) -> Vec<Vec<String>> {
        self.native.lock().unwrap().terminals[&agent]
            .submitted_images
            .clone()
    }
    pub(crate) fn correlated_text(&self, request: RequestId) -> String {
        let request = self.state().request(request).unwrap();
        format!(
            "[Bus message {}; answer with bus send --reply-to {} --as {} --text 'YOUR ANSWER'; run in the foreground and check ok:true and stage:\"replied\" before claiming delivery; if it fails, check the existing answer with bus message status {} before resending]\n{}",
            request.prompt.id.0, request.prompt.id.0, request.agent_id.0, request.prompt.id.0, request.prompt.text,
        )
    }
    pub(crate) fn record(&self, agent: AgentId, mut value: Value) {
        let provider = self.state().agent(agent).unwrap().provider;
        value["session_id"] = json!(format!("session-{}", agent.0));
        if provider == Provider::Cursor {
            value["conversation_id"] = value["session_id"].clone();
            if value["hook_event_name"] == "SessionStart" {
                value["hook_event_name"] = json!("sessionStart");
            }
        }
        if provider != Provider::ClaudeCode {
            value["transcript_path"] = json!(self.dir.join(format!("rollout-{}.jsonl", agent.0)));
        }
        callbacks::append(
            &self.dir.join(format!("callbacks/launch-{}", agent.0)),
            &format!("launch-{}", agent.0),
            provider,
            value,
            &[],
            false,
        )
        .unwrap();
    }
    pub(crate) fn append_transcript(&self, agent: AgentId, row: Value) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(format!("rollout-{}.jsonl", agent.0)))
            .unwrap();
        writeln!(file, "{row}").unwrap();
    }
    /// Shows `screen`, a composer captured from the real provider, for any draft.
    pub(crate) fn captured_composer(&mut self, agent: AgentId, screen: &str) {
        self.native
            .lock()
            .unwrap()
            .terminals
            .get_mut(&agent)
            .unwrap()
            .captured_screen = Some(screen.to_owned());
    }
    pub(crate) fn provider_startup(&mut self) {
        for agent in self.agents.clone() {
            self.record(
                agent,
                json!({"hook_event_name":"SessionStart","source":"startup"}),
            );
        }
        self.tick();
    }
    pub(crate) fn human_start(&mut self, agent: AgentId, turn: &str, text: &str) {
        let path = self.dir.join(format!("rollout-{}.jsonl", agent.0));
        self.append_transcript(
            agent,
            json!({"type":"user", "sessionId":format!("session-{}", agent.0),
            "promptId":turn, "promptSource":"typed", "turnOrigin":"human",
            "message":{"role":"user","content":text}}),
        );
        self.record(
            agent,
            json!({"hook_event_name":"UserPromptSubmit", "prompt_id":turn,
            "prompt":text, "transcript_path":path}),
        );
        self.status(agent, AgentState::Working);
        self.tick();
    }
    pub(crate) fn start(&mut self, agent: AgentId, turn: &str, text: &str) {
        self.spool_start(agent, turn, text);
        self.tick();
    }
    pub(crate) fn spool_start(&mut self, agent: AgentId, turn: &str, text: &str) {
        let text = self
            .typed(agent)
            .into_iter()
            .rev()
            .find(|payload| super::tests::support::test_prompt_content(payload) == text)
            .unwrap_or_else(|| text.to_owned());
        let provider = self.state().agent(agent).unwrap().provider;
        let key = match provider {
            Provider::Codex => "turn_id",
            Provider::Cursor => "generation_id",
            _ => "prompt_id",
        };
        let mut value = json!({"hook_event_name":if provider==Provider::Cursor {"beforeSubmitPrompt"} else {"UserPromptSubmit"},"prompt":text});
        value[key] = json!(turn);
        let provider = self.state().agent(agent).unwrap().provider;
        let row = if provider == Provider::Codex {
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":turn}})
        } else if provider == Provider::Cursor {
            json!({"role":"user","message":{"content":[{"type":"text","text":text}]}})
        } else {
            json!({"type":"user","uuid":turn,"message":{"role":"user","content":text}})
        };
        self.append_transcript(agent, row);
        self.record(agent, value);
        self.status(agent, AgentState::Working);
    }
    pub(crate) fn stop(
        &mut self,
        agent: AgentId,
        turn: &str,
        reply: &str,
        background: Value,
        crons: Value,
    ) {
        self.spool_stop(agent, turn, reply, background, crons);
        self.tick();
    }
    pub(crate) fn spool_stop(
        &mut self,
        agent: AgentId,
        turn: &str,
        reply: &str,
        background: Value,
        crons: Value,
    ) {
        let provider = self.state().agent(agent).unwrap().provider;
        let key = match provider {
            Provider::Codex => "turn_id",
            Provider::Cursor => "generation_id",
            _ => "prompt_id",
        };
        let mut value = json!({"hook_event_name":"Stop","last_assistant_message":reply,"background_tasks":background,"session_crons":crons});
        value[key] = json!(turn);
        let provider = self.state().agent(agent).unwrap().provider;
        let row = if provider == Provider::Codex {
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,"last_agent_message":reply}})
        } else if provider == Provider::Cursor {
            json!({"role":"assistant","message":{"content":[{"type":"text","text":reply}]}})
        } else {
            json!({"type":"assistant","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":reply}]}})
        };
        self.append_transcript(agent, row);
        if provider == Provider::Cursor {
            self.record(
                agent,
                json!({"hook_event_name":"afterAgentResponse","generation_id":turn,"text":reply}),
            );
            value["hook_event_name"] = json!("stop");
            value["status"] = json!("completed");
        }
        self.record(agent, value);
        self.status(agent, AgentState::Idle);
    }
    /// Move persisted deadline evidence, then run the real coordinator settlement;
    /// no sleeps or calls to matcher/model completion methods.
    pub(crate) fn elapse(&mut self, millis: u64) {
        let mut document = serde_json::to_value(self.state()).unwrap();
        for request in document["requests"].as_object_mut().unwrap().values_mut() {
            if let Some(pending) = request
                .get_mut("pending_final")
                .and_then(Value::as_object_mut)
            {
                if let Some(at) = pending.get("received_at_ms").and_then(Value::as_u64) {
                    pending.insert("received_at_ms".into(), json!(at.saturating_sub(millis)));
                }
            }
            if let Some(at) = request.get_mut("turn_ended_at_ms") {
                if let Some(value) = at.as_u64() {
                    *at = json!(value.saturating_sub(millis));
                }
            }
        }
        let state = serde_json::from_value::<BusState>(document)
            .unwrap()
            .with_history(Arc::clone(self.state().history()));
        self.worker_mut().save(state).unwrap();
        for at in self.worker_mut().own_turns.values_mut() {
            *at -= Duration::from_millis(millis);
        }
        self.tick();
    }

    #[cfg(unix)]
    pub(crate) fn use_socket(&mut self) {
        if self.socket.is_some() {
            return;
        }
        #[cfg(target_os = "macos")]
        let path = PathBuf::from(format!(
            "/private/tmp/brs-{}-{}",
            std::process::id(),
            sequence()
        ));
        #[cfg(not(target_os = "macos"))]
        let path = self.dir.join("api.sock");
        self.socket = Some(SocketServer::new(path.clone(), self.native.clone()));
        self.worker_mut().transport = Box::new(FixtureSocketTransport(BusTransport::new(
            crate::protocol::api::client::ConnectionTarget::SocketPath(path),
        )));
    }
    #[cfg(unix)]
    pub(crate) fn pause_server(&self, paused: bool) {
        self.socket
            .as_ref()
            .unwrap()
            .paused
            .store(paused, std::sync::atomic::Ordering::Relaxed);
    }
    pub(crate) fn reopen(&mut self) {
        drop(self.worker.take());
        let mut worker =
            Worker::open(self.dir.clone(), Box::new(Native(self.native.clone()))).unwrap();
        #[cfg(unix)]
        if let Some(socket) = &self.socket {
            worker.transport = Box::new(FixtureSocketTransport(BusTransport::new(
                crate::protocol::api::client::ConnectionTarget::SocketPath(socket.path.clone()),
            )));
        }
        worker.dev_enabled = true;
        self.worker = Some(worker);
        self.tick();
    }
    pub(crate) fn stall_diagnostics_for(&self, request: RequestId) -> usize {
        let marker = format!("stall:{}", request.0);
        usize::from(
            serde_json::to_value(self.state()).unwrap()["attention_episodes"]
                .as_array()
                .is_some_and(|episodes| {
                    episodes
                        .iter()
                        .any(|key| key.as_str() == Some(marker.as_str()))
                }),
        )
    }
    /// A reply group contributes one publication per source room. The unread
    /// counters are independent observable effects, unlike map-key uniqueness.
    pub(crate) fn assert_publications(&self) {
        use std::collections::BTreeSet;
        for room in self.state().rooms() {
            let replies: BTreeSet<_> = self
                .state()
                .requests()
                .filter(|r| r.room_id == room.id && r.phase == RequestPhase::Completed)
                .filter_map(|r| r.reply().map(|p| p.callback_id.clone()))
                .collect();
            let sends: BTreeSet<_> = self
                .state()
                .requests()
                .filter(|r| {
                    r.room_id == room.id
                        && matches!(r.prompt.author, Author::Agent(_) | Author::Bus)
                })
                .map(|r| r.prompt.id)
                .collect();
            let notices = self.state().notices(room.id).count();
            assert_eq!(
                room.unread_count as usize,
                replies.len() + sends.len() + notices,
                "publication count in room {}: {} reply groups, {} agent messages, {} notices",
                room.name,
                replies.len(),
                sends.len(),
                notices
            );
        }
    }
    pub(crate) fn assert_reply(&mut self, request: RequestId, text: &str) {
        let r = self.state().request(request).unwrap().clone();
        assert_eq!(
            r.phase,
            RequestPhase::Completed,
            "request {request:?}: {r:?}"
        );
        assert_eq!(r.reply().unwrap().text, text);
        self.assert_publications();
        for room in self.state().rooms() {
            assert_eq!(
                self.state()
                    .notices(room.id)
                    .filter(|n| n.author == Author::Agent(r.agent_id) && n.text == text)
                    .count(),
                0,
                "reply mirrored as a notice in {}",
                room.name
            );
        }
        let before: Vec<_> = self
            .state()
            .rooms()
            .map(|r| (r.id, r.unread_count))
            .collect();
        let history = self.call("room.history", json!({"room":r.room_id.0.to_string()}));
        let after: Vec<_> = self
            .state()
            .rooms()
            .map(|r| (r.id, r.unread_count))
            .collect();
        assert_eq!(before, after, "reading delivery must not republish it");
        let rows = history["messages"].as_array().unwrap();
        let occurrences: Vec<_> = rows
            .iter()
            .filter(|row| row["prompt"]["id"].as_u64() == Some(r.prompt.id.0))
            .collect();
        assert_eq!(
            occurrences.len(),
            1,
            "message missing/duplicated in history: {history}"
        );
        let requests = occurrences[0]["delivery"]["requests"].as_array().unwrap();
        let replies: Vec<_> = requests
            .iter()
            .filter(|row| row["request_id"].as_u64() == Some(request.0))
            .collect();
        assert_eq!(replies.len(), 1, "reply missing/duplicated: {history}");
        assert_eq!(replies[0]["reply"]["text"], text);
    }
}
impl Drop for Delivery {
    fn drop(&mut self) {
        drop(self.worker.take());
        #[cfg(unix)]
        drop(self.socket.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
