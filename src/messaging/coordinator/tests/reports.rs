use super::super::*;
use crate::bus::transport::TransportError;
use serde_json::json;

/// Accepts every native call, as a server that types prompts does.
struct Accepting;
impl Transport for Accepting {
    fn request(&mut self, _method: Method) -> Result<ResponseResult, TransportError> {
        Ok(ResponseResult::Ok {})
    }
}

struct Fixture {
    worker: Worker,
    master: RoomId,
    room: RoomId,
    orchestrator: AgentId,
    builder: AgentId,
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A Bus with an orchestrator in MASTER for room "work", and a worker there.
/// Each agent has its own launch (`orch` and `builder`) and session.
fn fixture(provider: Provider) -> Fixture {
    let dir = std::env::temp_dir().join(format!(
        "bus-reports-{}-{}",
        std::process::id(),
        crate::bus::io::now_ns()
    ));
    let mut worker = Worker::open(dir.clone(), Box::new(Accepting)).unwrap();
    let mut state = worker.state.clone();
    let master = state.master_room().unwrap().id;
    let room = state.create_room("work").unwrap();
    let orchestrator = state
        .create_agent(master, "orch", provider, dir.clone(), None)
        .unwrap();
    state.bind_orchestrator(orchestrator, room).unwrap();
    let builder = state
        .create_agent(room, "builder", provider, dir.clone(), None)
        .unwrap();
    for (agent, launch) in [(orchestrator, "orch"), (builder, "builder")] {
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some(launch.into()),
                    terminal_id: Some(format!("terminal-{launch}")),
                    pane_id: Some(format!("pane-{launch}")),
                    session_id: Some(format!("session-{launch}")),
                },
            )
            .unwrap();
        state.confirm_hook_setup(agent).unwrap();
        state.observe_status(agent, RuntimeStatus::Idle, 1).unwrap();
        callbacks::initialize(
            &dir.join("callbacks").join(launch),
            &callbacks::Manifest {
                agent_id: agent,
                provider,
                launch_id: launch.into(),
            },
        )
        .unwrap();
    }
    worker.save(state).unwrap();
    Fixture {
        worker,
        master,
        room,
        orchestrator,
        builder,
        dir,
    }
}

impl Fixture {
    fn launch(&self, agent: AgentId) -> &'static str {
        if agent == self.orchestrator {
            "orch"
        } else {
            "builder"
        }
    }

    /// One Claude Code turn: its submit hook carrying `prompt`, then its Stop.
    fn claude_turn(&mut self, agent: AgentId, turn: &str, prompt: &str, reply: &str) {
        let launch = self.launch(agent);
        let session = format!("session-{launch}");
        let spool = self.dir.join("callbacks").join(launch);
        for value in [
            json!({"hook_event_name":"UserPromptSubmit","session_id":session,"prompt_id":turn,"prompt":prompt}),
            json!({"hook_event_name":"Stop","session_id":session,"prompt_id":turn,"last_assistant_message":reply}),
        ] {
            callbacks::append(&spool, launch, Provider::ClaudeCode, value).unwrap();
        }
        self.worker.consume_callbacks(agent, &spool).unwrap();
    }

    /// Queues `text` from `author` in `room` to `to`, and types it.
    fn deliver(&mut self, room: RoomId, author: Author, to: AgentId, text: &str) -> String {
        let mut state = self.worker.state.clone();
        let request = state
            .submit_message_from(
                room,
                Draft {
                    text: text.into(),
                    files: Vec::new(),
                    recipient_ids: [to].into(),
                },
                author,
                2,
            )
            .unwrap()[0];
        self.worker.save(state).unwrap();
        self.worker.submit_ready().unwrap();
        assert_eq!(
            self.worker.state.agent(to).unwrap().current_request,
            Some(request),
            "the message was typed"
        );
        let request = self.worker.state.request(request).unwrap();
        request
            .submitted_payload
            .clone()
            .unwrap_or_else(|| request.prompt.rendered_payload())
    }

    /// The orchestrator's messages to the Human in MASTER.
    fn reports(&self) -> Vec<String> {
        self.worker
            .state
            .room(self.master)
            .unwrap()
            .notices
            .iter()
            .filter(|notice| notice.author == Author::Agent(self.orchestrator))
            .map(|notice| notice.text.clone())
            .collect()
    }
}

#[test]
fn an_unprompted_orchestrator_turn_is_reported_in_master_once() {
    let mut bus = fixture(Provider::ClaudeCode);
    let orchestrator = bus.orchestrator;
    // A background command exited and woke the orchestrator.
    bus.claude_turn(
        orchestrator,
        "own",
        "<task-notification>",
        "Gate passed on 1a2b3c.",
    );
    assert_eq!(bus.reports(), ["Gate passed on 1a2b3c."]);
    let master = bus.worker.state.room(bus.master).unwrap();
    // Like a reply: it rings and counts as unread while MASTER is not open.
    assert_eq!(
        master
            .latest_prompt
            .as_ref()
            .map(|prompt| prompt.text.as_str()),
        Some("Gate passed on 1a2b3c.")
    );
    assert_eq!(master.unread_count, 1);
    assert!(
        master.notices[0].recipient_ids.is_empty(),
        "orchestrator -> You"
    );
    // The same hooks read again (a restart replaying the spool) post nothing.
    let spool = bus.dir.join("callbacks/orch");
    bus.worker.consume_callbacks(orchestrator, &spool).unwrap();
    assert_eq!(bus.reports().len(), 1);
}

#[test]
fn a_reply_to_a_human_message_in_master_is_not_reported_again() {
    let mut bus = fixture(Provider::ClaudeCode);
    let (master, orchestrator) = (bus.master, bus.orchestrator);
    let typed = bus.deliver(master, Author::Human, orchestrator, "How is the PR?");
    bus.claude_turn(
        orchestrator,
        "asked",
        &typed,
        "Review is done; merging next.",
    );
    // The final was taken as the reply, which already shows in MASTER under
    // the human's message.
    let request = bus
        .worker
        .state
        .agent(orchestrator)
        .unwrap()
        .current_request;
    let reply = request
        .and_then(|request| bus.worker.state.request(request))
        .and_then(|request| request.pending_final.as_ref())
        .map(|reply| reply.text.clone());
    assert_eq!(reply.as_deref(), Some("Review is done; merging next."));
    assert!(bus.reports().is_empty(), "{:?}", bus.reports());
}

#[test]
fn a_turn_that_answers_a_worker_is_reported_in_master() {
    let mut bus = fixture(Provider::ClaudeCode);
    let (room, orchestrator, builder) = (bus.room, bus.orchestrator, bus.builder);
    let typed = bus.deliver(
        room,
        Author::Agent(builder),
        orchestrator,
        "Done: tests pass.",
    );
    bus.claude_turn(
        orchestrator,
        "worker",
        &typed,
        "builder finished; tests pass.",
    );
    assert_eq!(bus.reports(), ["builder finished; tests pass."]);
}

#[test]
fn a_report_the_orchestrator_already_sent_to_the_human_is_not_posted_twice() {
    let mut bus = fixture(Provider::ClaudeCode);
    let (master, orchestrator) = (bus.master, bus.orchestrator);
    let mut state = bus.worker.state.clone();
    // What `bus send --to human` does mid-turn.
    state
        .post_to_human(master, orchestrator, "Merged PR 12.".into(), Vec::new(), 3)
        .unwrap();
    bus.worker.save(state).unwrap();
    bus.claude_turn(orchestrator, "own", "<task-notification>", "Merged PR 12.");
    assert_eq!(bus.reports(), ["Merged PR 12."]);
}

#[test]
fn an_empty_or_wordless_final_is_not_a_report() {
    let mut bus = fixture(Provider::ClaudeCode);
    let orchestrator = bus.orchestrator;
    bus.claude_turn(orchestrator, "one", "<task-notification>", "  ");
    bus.claude_turn(orchestrator, "two", "<task-notification>", "…");
    assert!(bus.reports().is_empty(), "{:?}", bus.reports());
}

#[test]
fn worker_turns_never_post_to_master() {
    let mut bus = fixture(Provider::ClaudeCode);
    let builder = bus.builder;
    bus.claude_turn(
        builder,
        "own",
        "<task-notification>",
        "Background build finished.",
    );
    let master = bus.worker.state.room(bus.master).unwrap();
    assert!(master.notices.is_empty(), "{:?}", master.notices);
    assert!(master.latest_prompt.is_none());
    let room = bus.worker.state.room(bus.room).unwrap();
    assert!(room.notices.is_empty(), "{:?}", room.notices);
}

#[test]
fn an_unprompted_cursor_orchestrator_turn_is_reported_in_master() {
    let mut bus = fixture(Provider::Cursor);
    let orchestrator = bus.orchestrator;
    let spool = bus.dir.join("callbacks/orch");
    // Without a transcript path, Cursor's response hook carries the reply.
    for value in [
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"session-orch","generation_id":"own","prompt":"Background shell finished"}),
        json!({"hook_event_name":"afterAgentResponse","conversation_id":"session-orch","generation_id":"own","text":"Gate passed."}),
        json!({"hook_event_name":"stop","conversation_id":"session-orch","generation_id":"own","status":"completed"}),
    ] {
        callbacks::append(&spool, "orch", Provider::Cursor, value).unwrap();
    }
    bus.worker.consume_callbacks(orchestrator, &spool).unwrap();
    assert_eq!(bus.reports(), ["Gate passed."]);
}
