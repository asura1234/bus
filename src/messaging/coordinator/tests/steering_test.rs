use super::tests::support::{delivered_test_prompt, test_prompt_content};
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

#[path = "dialog_priority_test.rs"]
mod dialog_priority_tests;

#[path = "dialog_reconciliation_test.rs"]
mod dialog_reconciliation_tests;

#[path = "steer_failure_test.rs"]
mod steer_failure_tests;

#[path = "turn_settlement_test.rs"]
mod turn_settlement_tests;

#[path = "rollout_attribution_test.rs"]
mod rollout_attribution_tests;

#[path = "interrupt_test.rs"]
mod interrupt_tests;

#[path = "delivery_batch_test.rs"]
mod delivery_batch_tests;

#[path = "dispatch_shell_hold_test.rs"]
mod dispatch_shell_hold_tests;

#[path = "reply_to_test.rs"]
mod reply_to_tests;

#[cfg(unix)]
#[path = "kimi_steering_test.rs"]
mod kimi_steering_tests;

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Records every native method; prompt writes succeed.
struct Native(Arc<Mutex<Vec<Method>>>);

impl Transport for Native {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let result = match &method {
            Method::AgentPromptIfIdle(params) => ResponseResult::AgentPrompted {
                agent: serde_json::from_value(json!({
                    "terminal_id": params.expected_terminal_id,
                    "pane_id": params.expected_pane_id,
                    "agent": params.expected_agent,
                    "agent_status": "idle",
                    "interactive_ready": true,
                    "workspace_id": "workspace", "tab_id": "tab",
                    "focused": false, "revision": 1,
                    "agent_session": {
                        "source": "bus", "agent": params.expected_agent,
                        "kind": "id", "value": params.expected_session_id,
                    },
                }))
                .unwrap(),
            },
            Method::AgentObserve(params) => open_native_question(&params.target),
            Method::AgentGet(params) => open_native_info(&params.target),
            _ => ResponseResult::Ok {},
        };
        self.0.lock().unwrap().push(method);
        Ok(result)
    }
}

/// A coherent generic menu in the blocked worker's owned pane.
fn open_native_question(pane: &str) -> ResponseResult {
    let mut grid = schema::driver::Grid::blank(60, 5);
    for (row, line) in [
        "Which option?",
        "❯ 1. First",
        "  2. Second",
        "Enter to select · Esc to cancel",
    ]
    .iter()
    .enumerate()
    {
        for (col, ch) in line.chars().enumerate() {
            grid.cells[row][col].symbol = ch.to_string();
        }
    }
    ResponseResult::TerminalObserved {
        observation: schema::TerminalObservation {
            terminal_id: "terminal".into(),
            pane_id: pane.into(),
            content_revision: 2,
            text: grid.text(),
            focus: Some((1, "First".into())),
            focus_occurrence: 0,
            grid,
        },
    }
}

fn open_native_info(pane: &str) -> ResponseResult {
    ResponseResult::AgentInfo {
        agent: serde_json::from_value(json!({
            "terminal_id":"terminal", "pane_id":pane, "agent":"codex",
            "workspace_id":"workspace", "tab_id":"tab", "focused":false,
            "revision":1, "agent_status":"blocked", "interactive_ready":false,
        }))
        .unwrap(),
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
    fn bind_dialog_pane(&mut self, worker: AgentId) {
        let mut state = self.worker.state.clone();
        state
            .set_agent_runtime_identity(
                worker,
                AgentRuntimeIdentity {
                    launch_id: Some("worker-launch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("worker-pane".into()),
                    session_id: None,
                },
            )
            .unwrap();
        self.worker.save(state).unwrap();
    }

    /// Mirror tick's boundary: queued cleanup, durable callbacks, then expiry.
    fn expire_after_reconciliation(&mut self, now_ms: u64) {
        self.worker.expire_queued_requests(now_ms).unwrap();
        self.consume();
        self.worker.expire_unconfirmed_submissions(now_ms).unwrap();
    }

    fn new(provider: Provider) -> Self {
        Self::in_room(provider, false)
    }

    fn in_room(provider: Provider, master: bool) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bus-steering-{}-{}-{}",
            std::process::id(),
            crate::messaging::storage::io::now_ns(),
            NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let native = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::open(dir.clone(), Box::new(Native(native.clone()))).unwrap();
        let mut state = worker.state.clone();
        let room = if master {
            state.master_room().unwrap().id
        } else {
            state.create_room("room").unwrap()
        };
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
                Author::Developer,
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

    fn typed_content(&self) -> Vec<(String, bool)> {
        self.typed()
            .into_iter()
            .map(|(text, steer)| (test_prompt_content(&text), steer))
            .collect()
    }

    fn started(&self, turn: &str, prompt: &str) {
        let value = match self.provider {
            Provider::Kimi => {
                json!({"hook_event_name":"TurnStarted","session_id":"session","turn_id":turn.parse::<u64>().unwrap(),"origin_kind":"user","prompt":prompt})
            }
            Provider::Grok => {
                json!({"hookEventName":"user_prompt_submit","sessionId":"session","promptId":turn,"prompt":prompt})
            }
            Provider::ClaudeCode => {
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":turn,"prompt":prompt})
            }
            Provider::Codex => {
                json!({"hook_event_name":"UserPromptSubmit","session_id":"session","turn_id":turn,"prompt":prompt,"transcript_path":"/tmp/rollout.jsonl"})
            }
            Provider::Cursor => {
                json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session","generation_id":turn,"prompt":prompt})
            }
            #[cfg(feature = "dev-tools")]
            Provider::Fake => {
                json!({"event":"started","session":"session","turn":turn,"prompt":prompt})
            }
        };
        self.record(value);
    }

    fn stopped(&self, turn: &str, text: &str) {
        match self.provider {
            crate::agents::providers::ProviderKind::Grok | crate::agents::providers::ProviderKind::Kimi => panic!("This fixture covers the original providers; use dedicated Grok provider tests"),
            Provider::ClaudeCode => self.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,"last_assistant_message":text,"background_tasks":[],"session_crons":[]})),
            Provider::Codex => self.record(json!({"hook_event_name":"Stop","session_id":"session","turn_id":turn,"last_assistant_message":text,"transcript_path":"/tmp/rollout.jsonl"})),
            Provider::Cursor => {
                self.record(json!({"hook_event_name":"afterAgentResponse","conversation_id":"session","generation_id":turn,"text":text}));
                self.record(json!({"hook_event_name":"stop","conversation_id":"session","generation_id":turn,"status":"completed"}));
            }
            #[cfg(feature = "dev-tools")]
            Provider::Fake => self.record(json!({"event":"final","session":"session","turn":turn,"text":text})),
        }
    }

    fn record(&self, mut value: Value) {
        if let Some(prompt) = value.get("prompt").and_then(Value::as_str) {
            value["prompt"] = json!(delivered_test_prompt(&self.worker.state, prompt));
        }
        callbacks::append(
            &self.dir.join("callbacks/launch"),
            "launch",
            self.provider,
            value,
            &[],
            false,
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
        request.reply().map(|reply| reply.text.clone())
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
    assert!(
        fixture.request(lead).trusted_start_bound,
        "request={:?}; agent={:?}; typed={:?}",
        fixture.request(lead),
        fixture.worker.state.agent(fixture.agent),
        fixture.typed()
    );
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
            fixture.typed_content(),
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
        // The matching Stop settles the extended turn independently of native Idle.
        assert_eq!(
            fixture.request(lead).phase,
            RequestPhase::Completed,
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
            .control_message(fixture.request(correction).prompt.id)
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
fn typed_input_answered_in_its_own_turn_keeps_both_answers() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    // The first turn ends and the agent looks idle for a moment...
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    // ...then runs the typed input as the next turn.
    fixture.started("turn-2", "also add tests");
    fixture.stopped("turn-2", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("parser written"));
    assert_eq!(
        fixture.reply(correction).as_deref(),
        Some("parser and tests written")
    );
    assert_eq!(fixture.request(correction).group, None);
    assert_eq!(fixture.error(), None);
}

#[test]
fn typed_input_without_turn_proof_surfaces_an_uncertain_outcome() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    fixture.consume();
    assert!(fixture.reply(correction).is_none());
    assert_eq!(fixture.request(correction).phase, RequestPhase::Abandoned);
    assert_eq!(
        fixture.request(correction).failure_reason.as_deref(),
        Some("submission_unconfirmed")
    );
    assert!(fixture.request(correction).submission_notice().is_some());
}

/// Async followers need a terminal outcome. Unclaimed steering exposes its
/// uncertainty immediately and cannot borrow the lead's completed answer.
#[test]
fn an_unclaimed_joined_message_exposes_uncertainty_until_reconciled() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    let message = fixture.request(correction).prompt.id;
    let status = fixture.worker.control_message(message).unwrap();
    assert_eq!(status["complete"], false);
    assert_eq!(status["requests"][0]["stage"], "submitting");
    assert_eq!(status["requests"][0]["turn_ended"], false);
    assert_eq!(status["requests"][0]["uncertain_outcome"], true);
    fixture.consume();
    let status = fixture.worker.control_message(message).unwrap();
    assert_eq!(status["complete"], true);
    assert_eq!(status["requests"][0]["stage"], "abandoned");
    assert_eq!(status["requests"][0]["reason"], "submission_unconfirmed");
    assert!(status["requests"][0]["reply"].is_null());
}

#[test]
fn uncertain_steering_without_turn_proof_cannot_take_a_group_reply() {
    struct UncertainNative;
    impl Transport for UncertainNative {
        fn request(&mut self, _: Method) -> Result<ResponseResult, TransportError> {
            Ok(ResponseResult::Ok {})
        }
    }
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    fixture.worker.transport = Box::new(UncertainNative);
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    assert!(fixture.request(correction).uncertain_outcome);
    assert!(!fixture.request(correction).trusted_start_bound);
    fixture.stopped("turn-1", "parser and tests written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(correction).phase, RequestPhase::Abandoned);
    assert!(fixture.reply(correction).is_none());
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("parser and tests written")
    );
    let status = fixture
        .worker
        .control_message(fixture.request(correction).prompt.id)
        .unwrap();
    assert_eq!(status["complete"], true);
    assert_eq!(status["requests"][0]["reason"], "submission_unconfirmed");
    assert_eq!(status["requests"][0]["turn_ended"], false);
    assert_eq!(status["requests"][0]["uncertain_outcome"], true);
    assert_eq!(status["requests"][0]["start_bound"], false);
    assert!(status["requests"][0]["group"].is_null());
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
    state
        .observe_dialog(
            fixture.agent,
            true,
            state.agent(fixture.agent).unwrap().observed_at_ms,
        )
        .unwrap();
    fixture.worker.save(state).unwrap();
    fixture.deliver();
    assert_eq!(fixture.typed().len(), 1, "only the task itself was typed");
    assert_eq!(
        fixture.worker.state.queued_requests(fixture.agent),
        &[waiting, correction]
    );

    // The --queue message gets its own turn once the agent is idle again.
    let mut state = fixture.worker.state.clone();
    state
        .observe_dialog(
            fixture.agent,
            false,
            state.agent(fixture.agent).unwrap().observed_at_ms,
        )
        .unwrap();
    fixture.worker.save(state).unwrap();
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("parser written"));
    fixture.deliver();
    assert_eq!(
        fixture.typed_content()[1],
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
    let typed = fixture.typed_content();
    assert_eq!(typed.len(), 1);
    let text = &typed[0].0;
    assert!(text.starts_with("[1/2 from the developer at "), "{text}");
    assert!(
        text.contains("]\nfirst\n\n[2/2 from the developer at "),
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
    assert_eq!(fixture.typed_content()[1], ("own turn".to_owned(), false));
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
            crate::utils::test_temp::unique_temp_path("bus-steering-swap"),
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

/// `send --async` returns once every recipient's `turn_ended` is true, so a
/// captured Stop answer must read as ended even while its shells still run.
#[test]
fn a_captured_claude_stop_reports_the_turn_ended_despite_running_shells() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}),
    );
    fixture.consume();
    let lead = running(&mut fixture, "run the tests", "turn-1");
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"Waiting on the background shells.","session_crons":[],
        "background_tasks":[{"id":"b1","type":"shell","status":"running","description":"Run","command":"sleep 45"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
    let status = fixture
        .worker
        .control_message(fixture.request(lead).prompt.id)
        .unwrap();
    assert_eq!(status["requests"][0]["turn_ended"], true, "{status}");
}

/// A delivered Stop answer stays complete while its background shells run.
#[test]
fn a_delivered_claude_stop_answer_stays_completed() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}),
    );
    fixture.consume();
    let lead = running(&mut fixture, "run the tests", "turn-1");
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"Waiting on the background shells.","session_crons":[],
        "background_tasks":[{"id":"b1","type":"shell","status":"running","description":"Run","command":"cargo test"}]}));
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.now += crate::messaging::model::DELIVERED_STALL_MS + 1;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.request(lead).phase, RequestPhase::Completed);
}

/// A paste the agent keeps refusing (Bus retries every
/// `PROMPT_NOT_SHOWN_RETRY`) never reaches the agent, so queued recovery
/// still closes the message `QUEUED_STALL_MS` after it was sent; the retries
/// themselves are not progress.
#[test]
fn a_message_whose_paste_is_refused_on_every_retry_still_expires_as_queued() {
    use crate::messaging::model::{SubmissionOutcome, QUEUED_STALL_MS};
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let sent_at = fixture.now;
    let message = fixture.send("look at the screenshot", false);
    let retry_ms =
        crate::messaging::coordinator::delivery::PROMPT_NOT_SHOWN_RETRY.as_millis() as u64;
    let mut at = sent_at;
    while at < sent_at + QUEUED_STALL_MS + retry_ms {
        let mut state = fixture.worker.state.clone();
        // Each status poll still sees the agent Idle; each attempt is refused.
        state
            .observe_status(fixture.agent, RuntimeStatus::Idle, at)
            .unwrap();
        state.begin_submission(message, "launch", 0).unwrap();
        state
            .record_submission(
                message,
                SubmissionOutcome::DefinitelyRejected {
                    message: crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE.into(),
                },
            )
            .unwrap();
        fixture.worker.save(state).unwrap();
        at += retry_ms;
    }
    fixture.worker.expire_queued_requests(at).unwrap();
    assert_eq!(fixture.request(message).phase, RequestPhase::Abandoned);
}

fn claude_stop_holding(fixture: &Fixture, turn: &str, text: &str, shells: &[&str]) {
    let tasks: Vec<_> = shells
        .iter()
        .map(|id| json!({"id":id,"type":"shell","status":"running","description":"Run","command":"sleep 45"}))
        .collect();
    fixture.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":turn,"last_assistant_message":text,"background_tasks":tasks,"session_crons":[]}));
}

/// Cursor's later generation preserves an answer already captured for the
/// first generation instead of replacing it with the follow-up's answer.
#[test]
fn a_cursor_follow_up_after_a_captured_answer_keeps_both_answers() {
    let mut fixture = Fixture::new(Provider::Cursor);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("use the streaming API instead", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.started("turn-2", "use the streaming API instead");
    fixture.stopped("turn-2", "parser written with the streaming API");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    assert_eq!(fixture.reply(lead).as_deref(), Some("parser written"));
    assert_eq!(
        fixture.reply(correction).as_deref(),
        Some("parser written with the streaming API")
    );
    assert_eq!(fixture.request(correction).group, None);
}

/// Typed messages answered in separate turns keep their own turn's reply,
/// while a message Claude folds into one of those turns shares it.
#[test]
fn typed_messages_keep_their_own_turn_reply_and_folded_ones_share_it() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let second = fixture.send("also add tests", false);
    let folded = fixture.send("and docs", false);
    let fourth = fixture.send("then lint", false);
    fixture.deliver();
    fixture.deliver();
    fixture.deliver();
    fixture.stopped("turn-1", "parser written");
    fixture.started("turn-2", "also add tests");
    fixture.started("turn-2", "and docs");
    fixture.stopped("turn-2", "tests and docs written");
    fixture.started("turn-3", "then lint");
    fixture.stopped("turn-3", "lint clean");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("parser written"));
    for id in [second, folded] {
        assert_eq!(fixture.reply(id).as_deref(), Some("tests and docs written"));
    }
    assert_eq!(fixture.reply(fourth).as_deref(), Some("lint clean"));
    assert_eq!(fixture.error(), None);
}

/// Two messages with the same text, each answered in its own turn, keep their
/// own replies.
#[test]
fn identical_typed_messages_answered_in_separate_turns_keep_their_own_replies() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let first = fixture.send("yes", false);
    let second = fixture.send("yes", false);
    fixture.deliver();
    fixture.deliver();
    fixture.stopped("turn-1", "parser written");
    let first_prompt = fixture.request(first).submitted_payload.clone().unwrap();
    let second_prompt = fixture.request(second).submitted_payload.clone().unwrap();
    fixture.started("turn-2", &first_prompt);
    fixture.stopped("turn-2", "first yes done");
    fixture.started("turn-3", &second_prompt);
    fixture.stopped("turn-3", "second yes done");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(fixture.reply(lead).as_deref(), Some("parser written"));
    assert_eq!(fixture.reply(first).as_deref(), Some("first yes done"));
    assert_eq!(fixture.reply(second).as_deref(), Some("second yes done"));
}

/// While shells hold the replies, a message folded into a later turn shares
/// that turn's held reply and the next independent turn keeps its own.
#[test]
fn shell_held_turns_keep_their_replies_and_folded_messages_share_them() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    fixture.record(
        json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}),
    );
    fixture.consume();
    let lead = running(&mut fixture, "build it", "turn-1");
    claude_stop_holding(&fixture, "turn-1", "Waiting on the build.", &["b1"]);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let second = fixture.send("is it ready?", false);
    fixture.deliver();
    fixture.started("turn-2", "is it ready?");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    let folded = fixture.send("and the docs?", false);
    fixture.deliver();
    fixture.started("turn-2", "and the docs?");
    fixture.consume();
    claude_stop_holding(&fixture, "turn-2", "Not yet; docs pending.", &["b1", "b2"]);
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let third = fixture.send("ping", false);
    fixture.deliver();
    fixture.started("turn-3", "ping");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.stopped("turn-3", "pong");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    assert_eq!(
        fixture.reply(lead).as_deref(),
        Some("Waiting on the build.")
    );
    for id in [second, folded] {
        assert_eq!(fixture.reply(id).as_deref(), Some("Not yet; docs pending."));
    }
    assert_eq!(fixture.reply(third).as_deref(), Some("pong"));
    assert_eq!(fixture.error(), None);
}

#[path = "codex_unbound_reply_recovery_test.rs"]
mod codex_unbound_reply_recovery_tests;

#[path = "master_replies_test.rs"]
mod master_replies_tests;

/// Whether `send --async` / `wait --async` exit 0 on this `message.status`:
/// the message is complete and no request closed other than as
/// `reply_not_captured`.
fn async_waiters_exit_zero(status: &Value) -> bool {
    status["complete"] == true
        && status["requests"].as_array().unwrap().iter().all(|r| {
            r["stage"] == "replied"
                || (r["stage"] == "abandoned" && r["reason"] == "reply_not_captured")
        })
}

/// The goal of the async-wait fix: an interrupted message never exits 0. A
/// message typed into a turn whose reply arrived, then run by the agent and
/// interrupted before its ownership hook arrives, must not read replied.
#[test]
fn an_unclaimed_steered_message_interrupted_before_turn_proof_does_not_read_replied() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    running(&mut fixture, "write the parser", "turn-1");
    let correction = fixture.send("also add tests", false);
    fixture.deliver();
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    // The agent runs the typed input; its submit hook has not been seen.
    fixture.status(RuntimeStatus::Working);
    let response = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "probe-interrupt-unclaimed-input".into(),
            method: "agent.interrupt".into(),
            params: json!({"agent": "builder"}),
        },
        None,
    );
    assert!(response.ok, "{response:?}");
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    fixture.now = crate::messaging::storage::io::now_ms() + 5_000;
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    let status = fixture
        .worker
        .control_message(fixture.request(correction).prompt.id)
        .unwrap();
    assert!(
        !async_waiters_exit_zero(&status),
        "an interrupted typed message lets async waiters exit 0: {status}"
    );
}

/// #128 scratch e2e: a message queued before `agent interrupt` was typed
/// into the stopped turn while Bus still saw it Working; the agent ran it as a
/// turn of its own, whose reply the interrupted lead then took. A pressed stop
/// key ends steering: the message stays queued and closes with the turn.
#[test]
fn messages_are_not_typed_into_a_turn_after_its_stop_key() {
    let mut fixture = Fixture::new(Provider::Codex);
    let lead = running(&mut fixture, "write the parser", "turn-1");
    let mut state = fixture.worker.state.clone();
    state.begin_interrupt(fixture.agent).unwrap();
    fixture.worker.save(state).unwrap();
    let queued = fixture.send("also add tests", false);
    fixture.deliver();
    assert_eq!(fixture.typed().len(), 1, "{:?}", fixture.typed());
    fixture.status(RuntimeStatus::Idle);
    fixture.consume();
    for id in [lead, queued] {
        assert_eq!(fixture.request(id).phase, RequestPhase::Abandoned);
        assert_eq!(
            fixture.request(id).failure_reason.as_deref(),
            Some(crate::messaging::model::INTERRUPTED)
        );
    }
}

/// Deleting one recipient of a multi-recipient message must not make the
/// message read complete for the recipients that remain, so async waiters
/// do not exit 0 on work that never ran.
#[test]
fn deleting_an_unfinished_recipient_does_not_complete_a_multi_recipient_message() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let mut state = fixture.worker.state.clone();
    let other = state
        .create_agent(
            fixture.room,
            "reviewer",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    let ids = state
        .submit_message_with(
            fixture.room,
            Draft {
                text: "write the parser".into(),
                files: Vec::new(),
                recipient_ids: [fixture.agent, other].into(),
            },
            Author::Developer,
            fixture.now,
            false,
        )
        .unwrap();
    fixture.worker.save(state).unwrap();
    fixture.deliver();
    fixture.started("turn-1", "write the parser");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    let message = fixture.request(ids[0]).prompt.id;
    let before = fixture.worker.control_message(message).unwrap();
    assert_eq!(before["complete"], false, "{before}");
    let mut state = fixture.worker.state.clone();
    state.prepare_delete_agent(other).unwrap();
    state.delete_agent(other, fixture.now).unwrap();
    fixture.worker.save(state).unwrap();
    let after = fixture.worker.control_message(message);
    assert!(
        after.as_ref().map_or(true, |s| !async_waiters_exit_zero(s)),
        "a deleted recipient's message reads complete: {after:?}"
    );
}

/// One completed recipient and one still-open recipient share a message.
fn a_shared_message_with_one_completed_recipient() -> (Fixture, AgentId, PromptId) {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let mut state = fixture.worker.state.clone();
    let other = state
        .create_agent(
            fixture.room,
            "reviewer",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    let ids = state
        .submit_message_with(
            fixture.room,
            Draft {
                text: "write the parser".into(),
                files: Vec::new(),
                recipient_ids: [fixture.agent, other].into(),
            },
            Author::Developer,
            fixture.now,
            false,
        )
        .unwrap();
    fixture.worker.save(state).unwrap();
    fixture.deliver();
    fixture.started("turn-1", "write the parser");
    fixture.consume();
    fixture.status(RuntimeStatus::Working);
    fixture.stopped("turn-1", "parser written");
    fixture.consume();
    fixture.status(RuntimeStatus::Idle);
    let message = fixture.request(ids[0]).prompt.id;
    let before = fixture.worker.control_message(message).unwrap();
    assert_eq!(before["complete"], false, "{before}");
    (fixture, other, message)
}

#[test]
fn deleting_a_recipient_preserves_failure_when_the_other_reply_is_archived() {
    let (mut fixture, other, message) = a_shared_message_with_one_completed_recipient();
    let later =
        crate::messaging::storage::io::now_ms() + crate::messaging::model::ARCHIVE_AFTER_MS + 1;
    fixture.worker.archive_history(later).unwrap();
    assert_eq!(
        fixture.worker.control_message(message).unwrap()["complete"],
        false
    );
    let mut state = fixture.worker.state.clone();
    state.prepare_delete_agent(other).unwrap();
    state.delete_agent(other, fixture.now).unwrap();
    fixture.worker.save(state).unwrap();
    let status = fixture.worker.control_message(message).unwrap();
    assert!(
        !async_waiters_exit_zero(&status),
        "deleting an unfinished recipient after its peer reply was archived reads success: {status}"
    );
}

#[test]
fn a_deleted_recipient_failure_survives_history_archive_reload() {
    let (mut fixture, other, message) = a_shared_message_with_one_completed_recipient();
    let mut state = fixture.worker.state.clone();
    state.prepare_delete_agent(other).unwrap();
    state.delete_agent(other, fixture.now).unwrap();
    fixture.worker.save(state).unwrap();
    let before = fixture.worker.control_message(message).unwrap();
    assert!(!async_waiters_exit_zero(&before), "{before}");
    let later =
        crate::messaging::storage::io::now_ms() + crate::messaging::model::ARCHIVE_AFTER_MS + 1;
    fixture.worker.archive_history(later).unwrap();
    let reloaded = JsonStore::new(fixture.dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    fixture.worker.state = reloaded;
    let status = fixture.worker.control_message(message).unwrap();
    assert!(
        !async_waiters_exit_zero(&status),
        "reloading archived history erased the deleted recipient failure: {status}"
    );
}

#[test]
fn a_deleted_recipients_closed_request_survives_archive_reload_and_history() {
    let mut fixture = Fixture::new(Provider::ClaudeCode);
    let mut state = fixture.worker.state.clone();
    let other = state
        .create_agent(
            fixture.room,
            "reviewer",
            Provider::ClaudeCode,
            fixture.dir.clone(),
            None,
        )
        .unwrap();
    let ids = state
        .submit_message_with(
            fixture.room,
            Draft {
                text: "review the parser".into(),
                files: Vec::new(),
                recipient_ids: [fixture.agent, other].into(),
            },
            Author::Developer,
            fixture.now,
            false,
        )
        .unwrap();
    state.prepare_delete_agent(other).unwrap();
    state.delete_agent(other, fixture.now).unwrap();
    let saved = serde_json::to_value(&state).unwrap();
    let mut reloaded: crate::messaging::model::BusState = serde_json::from_value(saved).unwrap();
    reloaded.archive(fixture.now + crate::messaging::model::ARCHIVE_AFTER_MS + 1);
    fixture.worker.save(reloaded).unwrap();
    let message = fixture.request(ids[0]).prompt.id;
    let status = fixture.worker.control_message(message).unwrap();
    assert_eq!(status["requests"].as_array().unwrap().len(), 2, "{status}");
    let history = fixture.worker.control_response_with_events(
        &crate::messaging::control::Request {
            id: "history".into(),
            method: "room.history".into(),
            params: json!({"room": fixture.room.0.to_string()}),
        },
        None,
    );
    assert!(history.ok, "{history:?}");
}

/// A MASTER orchestrator running `lead`, bound to a work room whose Codex
/// worker has just posted a real blocked-dialog notice to it.
fn orchestrator_with_blocked_worker_notice() -> (Fixture, AgentId, RequestId, RequestId) {
    use crate::messaging::model::DialogNoticeTo;
    let mut f = Fixture::in_room(Provider::ClaudeCode, true);
    let mut state = f.worker.state.clone();
    let work = state.create_room("work").unwrap();
    state.bind_orchestrator(f.agent, work).unwrap();
    let codex = state
        .create_agent(work, "feishu-pull", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    f.worker.save(state).unwrap();
    f.bind_dialog_pane(codex);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let lead = running(&mut f, "keep working", "turn-1");
    for _ in 0..2 {
        f.worker
            .notify_dialogs(vec![(
                codex,
                Some(crate::messaging::coordinator::dialogs::BLOCKED.into()),
            )])
            .unwrap();
    }
    let Some(DialogNoticeTo::Orchestrator(notice)) =
        f.worker.state.agent(codex).unwrap().dialog_notice_to
    else {
        panic!("blocked question must queue a notice");
    };
    (f, codex, lead, notice)
}

#[test]
fn an_uncertain_steered_dialog_notice_reaches_the_developer_even_if_its_turn_settles_first() {
    use crate::messaging::model::{DialogNoticeTo, QUEUED_STALL_MS};
    struct UncertainNative;
    impl Transport for UncertainNative {
        fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
            Ok(match method {
                Method::AgentObserve(params) => open_native_question(&params.target),
                Method::AgentGet(params) => open_native_info(&params.target),
                _ => ResponseResult::Ok {},
            })
        }
    }
    let (mut f, codex, lead, notice) = orchestrator_with_blocked_worker_notice();
    f.worker.transport = Box::new(UncertainNative);
    f.deliver();
    assert!(f.request(notice).uncertain_outcome);
    assert!(!f.request(notice).trusted_start_bound);
    assert_eq!(f.request(notice).group, Some(lead));
    // The lead's turn ends without any hook showing the notice was seen.
    f.stopped("turn-1", "MASTER status done");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now = crate::messaging::storage::io::now_ms() + 5_000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    let Some(crate::messaging::model::DialogNoticeDelivery::Unconfirmed { since_ms: at }) =
        f.request(notice).dialog_delivery
    else {
        panic!("the real native write must persist its start before I/O");
    };
    f.expire_after_reconciliation(at + QUEUED_STALL_MS);
    assert_eq!(
        f.worker.state.agent(codex).unwrap().dialog_notice_to,
        Some(DialogNoticeTo::Developer),
        "an unconfirmed blocked notice must reach the developer: notice={:?}",
        f.request(notice)
    );
}

#[test]
fn an_interrupt_that_closes_a_queued_dialog_notice_reaches_the_developer() {
    use crate::messaging::model::{DialogNoticeTo, QUEUED_STALL_MS};
    let (mut f, codex, _lead, notice) = orchestrator_with_blocked_worker_notice();
    // The developer pressed stop before Bus could type the notice.
    let mut state = f.worker.state.clone();
    state.begin_interrupt(f.agent).unwrap();
    f.worker.save(state).unwrap();
    f.deliver();
    assert_eq!(f.request(notice).phase, RequestPhase::Queued);
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(notice).phase, RequestPhase::Abandoned);
    let at = f.request(notice).prompt.submitted_at_ms;
    f.expire_after_reconciliation(at + QUEUED_STALL_MS);
    f.deliver();
    let room = f.request(notice).room_id;
    let reached_developer =
        f.worker.state.agent(codex).unwrap().dialog_notice_to == Some(DialogNoticeTo::Developer);
    let retried = f
        .worker
        .state
        .queued_requests(f.agent)
        .iter()
        .any(|id| f.request(*id).prompt.text.contains("Blocked, needs help"));
    assert!(
        reached_developer || retried,
        "an interrupted blocked notice is lost: dialog_notice_to={:?}, notices={:?}",
        f.worker.state.agent(codex).unwrap().dialog_notice_to,
        f.worker.state.room(room).unwrap().notices
    );
    assert!(reached_developer);
    assert!(!retried, "interrupted notices must not be replayed");
    f.expire_after_reconciliation(at + QUEUED_STALL_MS + 60_000);
    assert_eq!(f.worker.state.room(room).unwrap().notices.len(), 1);
}
