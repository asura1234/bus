use super::*;
use crate::{
    agents::providers::spool as callbacks,
    messaging::{native::TransportError, storage::state_store::JsonStore},
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};

#[path = "turn_settlement_test.rs"]
mod turn_settlement_tests;

#[path = "delivery_batch_test.rs"]
mod delivery_batch_tests;

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Records every native method; prompt writes succeed.
struct Native(Arc<Mutex<Vec<Method>>>);

impl Transport for Native {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        self.0.lock().unwrap().push(method);
        Ok(ResponseResult::Ok {})
    }
}

struct Fixture {
    worker: Worker,
    agent: AgentId,
    room: RoomId,
    dir: PathBuf,
    provider: Provider,
    native: Arc<Mutex<Vec<Method>>>,
    now: u64,
}

impl Fixture {
    fn new(provider: Provider) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bus-steering-{}-{}-{}",
            std::process::id(),
            crate::messaging::storage::io::now_ns(),
            NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let native = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::open(dir.clone(), Box::new(Native(native.clone()))).unwrap();
        let mut state = worker.state.clone();
        let room = state.create_room("room").unwrap();
        let agent = state
            .create_agent(room, "builder", provider, dir.clone(), None)
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
        Self {
            worker,
            agent,
            room,
            dir,
            provider,
            native,
            now: crate::messaging::storage::io::now_ms(),
        }
    }

    fn send(&mut self, text: &str, queue_only: bool) -> RequestId {
        let mut state = self.worker.state.clone();
        let ids = state
            .submit_message_with(
                self.room,
                Draft {
                    text: text.into(),
                    files: Vec::new(),
                    recipient_ids: [self.agent].into(),
                },
                Author::Human,
                self.now,
                queue_only,
            )
            .unwrap();
        self.worker.save(state).unwrap();
        ids[0]
    }

    fn status(&mut self, status: RuntimeStatus) {
        self.now += 1;
        let mut state = self.worker.state.clone();
        state.observe_status(self.agent, status, self.now).unwrap();
        self.worker.save(state).unwrap();
    }

    fn deliver(&mut self) {
        self.worker.submit_ready().unwrap();
    }

    /// The prompt texts Bus typed, and whether each was a steering write.
    fn typed(&self) -> Vec<(String, bool)> {
        self.native
            .lock()
            .unwrap()
            .iter()
            .filter_map(|method| match method {
                Method::AgentPromptIfIdle(params) => Some((params.text.clone(), params.steer)),
                _ => None,
            })
            .collect()
    }

    fn started(&self, turn: &str, prompt: &str) {
        let value = match self.provider {
            Provider::ClaudeCode => {
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":turn,"prompt":prompt})
            }
            Provider::Codex => {
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":turn,"prompt":prompt,"transcript_path":"/tmp/rollout.jsonl"})
            }
            Provider::Cursor => {
                json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":turn,"prompt":prompt})
            }
        };
        self.record(value);
    }

    fn stopped(&self, turn: &str, text: &str) {
        match self.provider {
            Provider::ClaudeCode => self.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,"last_assistant_message":text,"background_tasks":[],"session_crons":[]})),
            Provider::Codex => self.record(json!({"hook_event_name":"Stop","session_id":"session","turn_id":turn,"last_assistant_message":text,"transcript_path":"/tmp/rollout.jsonl"})),
            Provider::Cursor => {
                self.record(json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":turn,"text":text}));
                self.record(json!({"hook_event_name":"stop","conversation_id":"session","generation_id":turn,"status":"completed"}));
            }
        }
    }

    fn record(&self, value: Value) {
        callbacks::append(
            &self.dir.join("callbacks/launch"),
            "launch",
            self.provider,
            value,
        )
        .unwrap();
    }

    fn consume(&mut self) {
        let spool = self.dir.join("callbacks/launch");
        self.worker.consume_callbacks(self.agent, &spool).unwrap();
        self.worker.settle_requests().unwrap();
    }

    fn request(&self, id: RequestId) -> &Request {
        self.worker.state.request(id).unwrap()
    }

    fn reply(&self, id: RequestId) -> Option<String> {
        let request = self.request(id);
        (request.phase == RequestPhase::Completed)
            .then(|| {
                request
                    .pending_final
                    .as_ref()
                    .map(|reply| reply.text.clone())
            })
            .flatten()
    }

    fn error(&self) -> Option<String> {
        self.worker
            .state
            .agent(self.agent)
            .unwrap()
            .actionable_error
            .clone()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Starts `task` as a bound, running turn `turn`.
fn running(fixture: &mut Fixture, task: &str, turn: &str) -> RequestId {
    let lead = fixture.send(task, false);
    fixture.deliver();
    fixture.started(turn, task);
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    assert!(fixture.request(lead).trusted_start_bound);
    lead
}

#[test]
fn a_message_sent_while_working_is_typed_into_the_turn_and_shares_its_reply() {
    // Live: Claude Code and Codex report the typed input mid-turn under the
    // running turn's id; Cursor runs it as a generation of its own.
    for (provider, steered_turn) in [
        (Provider::ClaudeCode, "turn-1"),
        (Provider::Codex, "turn-1"),
        (Provider::Cursor, "turn-2"),
    ] {
        let mut fixture = Fixture::new(provider);
        let lead = running(&mut fixture, "write the parser", "turn-1");
        let correction = fixture.send("use the streaming API instead", false);
        fixture.deliver();
        assert_eq!(
            fixture.typed(),
            [
                ("write the parser".to_owned(), false),
                ("use the streaming API instead".to_owned(), true)
            ],
            "{provider:?}"
        );
        assert_eq!(fixture.request(correction).group, Some(lead));
        assert!(fixture
            .worker
            .state
            .queued_requests(fixture.agent)
            .is_empty());

        fixture.started(steered_turn, "use the streaming API instead");
        if steered_turn != "turn-1" {
            // The first generation still ends, with the uncorrected answer.
            fixture.stopped("turn-1", "parser written");
        }
        fixture.stopped(steered_turn, "parser written with the streaming API");
        fixture.consume();
        // Idle seen before the typing does not settle the extended turn.
        assert_eq!(
            fixture.request(lead).phase,
            RequestPhase::Active,
            "{provider:?}"
        );
        fixture.status(RuntimeStatus::Idle);
        for id in [lead, correction] {
            assert_eq!(
                fixture.reply(id).as_deref(),
                Some("parser written with the streaming API"),
                "{provider:?}"
            );
        }
        assert_eq!(fixture.error(), None, "{provider:?}");
        let status = fixture
            .worker
            .dev_message(fixture.request(correction).prompt.id)
            .unwrap();
        assert_eq!(status["complete"], true);
        assert_eq!(status["requests"][0]["group"], lead.0);
        assert_eq!(
            status["requests"][0]["reply"]["text"],
            "parser written with the streaming API"
        );
    }
}

#[test]
fn typed_input_the_provider_runs_as_its_own_turn_still_answers_the_group() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    // The first turn ends and the agent looks idle for a moment...
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    // ...then runs the typed input as the next turn.
    fixture.started("turn-2", "also add tests");
    fixture.stopped("turn-2", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("parser and tests written")
    );
    assert_eq!(
        fixture.reply(correction).as_deref(),
        Some("parser and tests written")
    );
    assert_eq!(fixture.error(), None);
}

#[test]
fn typed_input_without_a_submit_hook_settles_after_a_short_wait() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Active);
    fixture.now =
        crate::messaging::storage::io::now_ms() + crate::messaging::model::STEERING_SETTLE_MS;
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.reply(correction).as_deref(),
        Some("parser and tests written")
    );
}

#[test]
fn blocked_agents_dialogs_and_queue_only_messages_are_never_typed_into_a_turn() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let waiting = fixture.send("wait for your own turn", true);
    fixture.deliver();
    fixture.status(RuntimeStatus::Blocked);
    let correction = fixture.send("stop and use streaming", false);
    fixture.deliver();
    let mut state = fixture.worker.state.clone();
    state
        .observe_status(fixture.agent, RuntimeStatus::Working, 99)
        .unwrap();
    state.observe_dialog(fixture.agent, true).unwrap();
    fixture.worker.save(state).unwrap();
    fixture.deliver();
    assert_eq!(fixture.typed().len(), 1, "only the task itself was typed");
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[waiting, correction]
    );

    // The --queue message gets its own turn once the agent is idle again.
    let mut state = fixture.worker.state.clone();
    state.observe_dialog(fixture.agent, false).unwrap();
    fixture.worker.save(state).unwrap();
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("parser written"));
    fixture.deliver();
    assert_eq!(
        fixture.typed()[1],
        ("wait for your own turn".to_owned(), false)
    );
    assert_eq!(fixture.request(waiting).group, None);
}

#[test]
fn messages_that_waited_for_an_unavailable_agent_are_coalesced_into_one_prompt() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.status(RuntimeStatus::Launching);
    let first = fixture.send("first", false);
    let second = fixture.send("second", false);
    let own_turn = fixture.send("own turn", true);
    fixture.deliver();
    assert!(fixture.typed().is_empty());
    fixture.status(RuntimeStatus::Idle);
    fixture.deliver();
    let typed = fixture.typed();
    assert_eq!(typed.len(), 1);
    let text = &typed[0].0;
    assert!(text.starts_with("[1/2 from the human at "), "{text}");
    assert!(
        text.contains("]\nfirst\n\n[2/2 from the human at "),
        "{text}"
    );
    assert!(text.ends_with("]\nsecond"), "{text}");
    assert_eq!(fixture.request(second).group, Some(first));
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[own_turn]
    );

    fixture.started("turn-1", text);
    fixture.stopped("turn-1", "both done");
    fixture.consume();
    assert_eq!(fixture.error(), None);
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(first).as_deref(), Some("both done"));
    assert_eq!(fixture.reply(second).as_deref(), Some("both done"));
    // Messages queued while the agent was ready go one turn each, as before.
    fixture.deliver();
    assert_eq!(fixture.typed()[1], ("own turn".to_owned(), false));
}

#[test]
fn a_steered_group_survives_a_restart() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("use streaming", false);
    fixture.deliver();
    let saved = JsonStore::new(fixture.dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(saved.request(correction).unwrap().group, Some(lead));
    assert!(saved.request(correction).unwrap().steered);

    let dir = fixture.dir.clone();
    let native = fixture.native.clone();
    let worker = std::mem::replace(
        &mut fixture.worker,
        Worker::open(
            std::env::temp_dir().join(format!(
                "bus-steering-swap-{}",
                crate::messaging::storage::io::now_ns()
            )),
            Box::new(Native(native.clone())),
        )
        .unwrap(),
    );
    let swap = fixture.worker.data_dir.clone();
    drop(worker);
    fixture.worker = Worker::open(dir, Box::new(Native(native))).unwrap();
    let _ = std::fs::remove_dir_all(swap);
    fixture.started("turn-1", "use streaming");
    fixture.stopped("turn-1", "streaming parser");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(
        fixture.reply(correction).as_deref(),
        Some("streaming parser")
    );
}

#[test]
fn old_saved_requests_load_without_group_fields() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let request = fixture.send("old", false);
    let mut value = serde_json::to_value(&fixture.worker.state).unwrap();
    let saved = &mut value["requests"][request.0.to_string()];
    for field in [
        "queue_only",
        "group",
        "steered",
        "submitted_payload",
        "turn_ended_at_ms",
        "awaiting_background",
    ] {
        saved.as_object_mut().unwrap().remove(field);
    }
    let loaded: BusState = serde_json::from_value(value).unwrap();
    let request = loaded.request(request).unwrap();
    assert!(!request.queue_only && !request.steered);
    assert!(request.turn_ended_at_ms.is_none() && !request.awaiting_background);
    assert_eq!(
        (request.group, request.submitted_payload.as_deref()),
        (None, None)
    );
}
