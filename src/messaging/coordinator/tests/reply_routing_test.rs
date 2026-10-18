use super::*;
use crate::messaging::{control::Request as ControlRequest, native::TransportError};
use serde_json::json;

static NEXT_FIXTURE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct Accepting(Arc<Mutex<Vec<String>>>);
impl Transport for Accepting {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentPromptIfIdle(p) => self.0.lock().unwrap().push(p.text),
            other => panic!("unexpected native call: {other:?}"),
        }
        Ok(ResponseResult::AgentPrompted {
            agent: serde_json::from_value(json!({
                "terminal_id":"terminal", "pane_id":"pane", "agent":"claude", "agent_status":"idle",
                "workspace_id":"workspace", "tab_id":"tab", "focused":false, "revision":1
            }))
            .unwrap(),
        })
    }
}

struct Fixture {
    worker: Worker,
    dir: PathBuf,
    room: RoomId,
    orch: AgentId,
    agent: AgentId,
    calls: Arc<Mutex<Vec<String>>>,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bus-reply-route-{}-{}-{}",
            std::process::id(),
            io::now_ns(),
            NEXT_FIXTURE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut worker = Worker::open(dir.clone(), Box::new(Accepting(calls.clone()))).unwrap();
        let mut state = worker.state.clone();
        let room = state.create_room("work").unwrap();
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
        let agent = state
            .create_agent(room, "builder", Provider::ClaudeCode, dir.clone(), None)
            .unwrap();
        state
            .set_agent_runtime_identity(
                orch,
                AgentRuntimeIdentity {
                    launch_id: Some("orch".into()),
                    terminal_id: Some("terminal".into()),
                    pane_id: Some("pane".into()),
                    session_id: Some("session".into()),
                },
            )
            .unwrap();
        state.confirm_hook_setup(orch).unwrap();
        state
            .observe_status(orch, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        worker.save(state).unwrap();
        crate::agents::providers::spool::initialize(
            &dir.join("callbacks/orch"),
            &crate::agents::providers::spool::Manifest {
                routing_key: crate::agents::providers::spool::RoutingKey(orch.0),
                provider: Provider::ClaudeCode,
                launch_id: "orch".into(),
            },
        )
        .unwrap();
        Self {
            worker,
            dir,
            room,
            orch,
            agent,
            calls,
        }
    }

    fn send(&mut self, author: Author, text: &str) -> RequestId {
        let mut state = self.worker.state.clone();
        let id = state
            .submit_message_from(
                self.room,
                Draft {
                    text: text.into(),
                    files: vec![],
                    recipient_ids: [self.agent].into(),
                },
                author,
                io::now_ms() - 4_000,
            )
            .unwrap()[0];
        self.worker.save(state).unwrap();
        id
    }

    fn finish(&mut self, id: RequestId, text: &str) {
        let mut state = self.worker.state.clone();
        state.finish_for_test(id, text, io::now_ms() - 3_000);
        self.worker.save(state).unwrap();
    }

    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::messaging::control::Response {
        self.worker.control_response_with_events(
            &ControlRequest {
                id: format!("test-{}", io::now_ns()),
                method: method.into(),
                params,
            },
            None,
        )
    }

    fn notices(&self) -> Vec<&Request> {
        self.worker
            .state
            .requests()
            .filter(|r| r.agent_id == self.orch && r.prompt.author == Author::Agent(self.agent))
            .collect()
    }

    fn reopen(&mut self) {
        // Drop the lease before opening the same durable directory again.
        let dummy = Worker::open(
            self.dir.join("unused"),
            Box::new(Accepting(self.calls.clone())),
        )
        .unwrap();
        drop(std::mem::replace(&mut self.worker, dummy));
        self.worker =
            Worker::open(self.dir.clone(), Box::new(Accepting(self.calls.clone()))).unwrap();
        let mut state = self.worker.state.clone();
        state
            .observe_status(self.orch, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        self.worker.save(state).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn reply_routing_delivers_to_the_orchestrator_without_any_waiter() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Built and tested.");
    f.worker.submit_ready().unwrap();
    let calls = f.calls.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "a captured reply must wake the terminal without a waiter"
    );
    assert!(calls[0].contains("builder"));
    assert!(calls[0].contains("Built and tested."));
    assert!(calls[0].contains(&format!(
        "message {}",
        f.worker.state.request(source).unwrap().prompt.id.0
    )));
    assert_eq!(f.notices().len(), 1);
}

#[test]
fn reply_routing_recovers_pending_and_queued_replies_once_after_restart() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Finished while waiter was gone.");
    f.reopen();
    f.worker.submit_ready().unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[test]
fn reply_routing_acknowledged_print_does_not_wake_again_after_restart() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Printed by the author waiter.");
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn reply_routing_developer_replies_are_unchanged() {
    let mut f = Fixture::new();
    let source = f.send(Author::Developer, "build this");
    f.finish(source, "Developer reply");
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
}

#[test]
fn reply_routing_waiter_lease_holds_early_replies_until_it_expires() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Early reply");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert!(
        f.notices().is_empty(),
        "a live author waiter has not printed its multi-worker result yet"
    );
}

#[test]
fn reply_routing_a_plain_status_reader_cannot_suppress_delivery() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Still needs delivery");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    assert!(
        f.call("message.status", json!({"message":message.0.to_string()}))
            .ok
    );
    assert!(
        !f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":f.agent.0.to_string()})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
}

#[test]
fn reply_routing_group_members_share_one_notice_and_one_acknowledgement() {
    for acknowledge in [false, true] {
        let mut f = Fixture::new();
        let lead = f.send(Author::Agent(f.orch), "first");
        let member = f.send(Author::Agent(f.orch), "second");
        let mut state = f.worker.state.clone();
        state.coalesce_queue(lead).unwrap();
        f.worker.save(state).unwrap();
        f.finish(lead, "One shared reply");
        f.finish(member, "One shared reply");
        if acknowledge {
            assert!(
                f.call(
                    "message.seen",
                    json!({"request":member.0.to_string(),"as":f.orch.0.to_string()})
                )
                .ok
            );
        }
        f.worker.submit_ready().unwrap();
        assert_eq!(f.notices().len(), usize::from(!acknowledge));
    }
}

#[test]
fn reply_routing_keeps_undelivered_notices_across_expiry_and_restart() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Never lose this reply");
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.orch, RuntimeStatus::Unavailable, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let notice = f.notices()[0].id;
    f.worker
        .expire_queued_requests(io::now_ms() + 3_600_000)
        .unwrap();
    f.worker
        .archive_history(io::now_ms() + 8 * 24 * 3_600_000)
        .unwrap();
    assert!(
        f.worker.state.request(source).is_some(),
        "pending delivery pins its full reply beyond ordinary retention"
    );
    assert_eq!(
        f.worker.state.request(notice).unwrap().phase,
        RequestPhase::Queued
    );
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[test]
fn reply_routing_an_expired_waiter_lease_releases_the_reply() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "No live waiter");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    f.worker
        .reply_waiters
        .insert(message, std::time::Instant::now() - Duration::from_secs(4));
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
}

#[test]
fn reply_routing_save_failure_does_not_record_a_delivery_or_contact_the_terminal() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Durable reply");
    let invalid = f.dir.join("not-a-state-file");
    std::fs::create_dir(&invalid).unwrap();
    f.worker.store = JsonStore::new(invalid);
    assert!(f.worker.submit_ready().is_err());
    assert!(f.notices().is_empty());
    assert!(f.calls.lock().unwrap().is_empty());
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
}

#[test]
fn reply_routing_multi_worker_author_waiter_acknowledges_early_and_late_replies() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let second = state
        .create_agent(f.room, "second", Provider::ClaudeCode, f.dir.clone(), None)
        .unwrap();
    let ids = state
        .submit_message_from(
            f.room,
            Draft {
                text: "both build".into(),
                files: vec![],
                recipient_ids: [f.agent, second].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 4_000,
        )
        .unwrap();
    let message = state.request(ids[0]).unwrap().prompt.id;
    f.worker.save(state).unwrap();
    f.finish(ids[0], "early");
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":"orch"})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
    f.finish(ids[1], "late");
    for id in ids {
        assert!(
            f.call(
                "message.seen",
                json!({"request":id.0.to_string(),"as":"orch"})
            )
            .ok
        );
    }
    f.worker.submit_ready().unwrap();
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn reply_routing_consumes_a_final_spooled_while_bus_was_down() {
    use crate::agents::providers::spool;
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    let mut state = f.worker.state.clone();
    state
        .set_agent_runtime_identity(
            f.agent,
            AgentRuntimeIdentity {
                launch_id: Some("worker".into()),
                terminal_id: Some("worker-terminal".into()),
                pane_id: Some("worker-pane".into()),
                session_id: Some("worker-session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(f.agent).unwrap();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    state.begin_submission(source, "worker", 0).unwrap();
    state
        .record_submission(
            source,
            SubmissionOutcome::Confirmed {
                provider_session_id: Some("worker-session".into()),
                provider_turn_id: None,
            },
        )
        .unwrap();
    f.worker.save(state).unwrap();
    let dir = f.dir.join("callbacks/worker");
    spool::initialize(
        &dir,
        &spool::Manifest {
            routing_key: spool::RoutingKey(f.agent.0),
            provider: Provider::ClaudeCode,
            launch_id: "worker".into(),
        },
    )
    .unwrap();
    f.reopen();
    for value in [
        json!({"hook_event_name":"UserPromptSubmit","session_id":"worker-session","prompt_id":"turn","prompt":"build this"}),
        json!({"hook_event_name":"Stop","session_id":"worker-session","prompt_id":"turn","last_assistant_message":"Recovered from disk spool"}),
    ] {
        spool::append(&dir, "worker", Provider::ClaudeCode, value, &[], false).unwrap();
    }
    f.reopen();
    f.worker.consume_callbacks(f.agent, &dir).unwrap();
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() + 3_000)
        .unwrap();
    assert_eq!(
        state.request(source).unwrap().phase,
        RequestPhase::Completed
    );
    assert!(state
        .route_worker_replies(io::now_ms() + 6_000, &BTreeSet::new())
        .unwrap());
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Recovered from disk spool"));
}

#[test]
fn reply_routing_restart_keeps_an_old_full_reply_behind_its_pending_pointer() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    let mut state = f.worker.state.clone();
    state.finish_for_test(
        source,
        &"full reply🚌".repeat(1_000),
        io::now_ms() - 8 * 24 * 3_600_000,
    );
    state
        .observe_status(f.orch, RuntimeStatus::Unavailable, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.reopen();
    assert!(
        f.worker.state.request(source).is_some(),
        "loading must preserve the full reply while its notification is pending"
    );
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
}

#[test]
fn reply_routing_ack_cancels_an_untyped_notification() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Printed just after queuing");
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.orch, RuntimeStatus::Unavailable, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices()[0].phase, RequestPhase::Abandoned);
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn reply_routing_archived_replies_and_unicode_pointers_are_recovered_once() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, &"回复🚌".repeat(3_000));
    f.worker
        .archive_history(io::now_ms() + crate::messaging::model::ARCHIVE_AFTER_MS)
        .unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let text = &f.notices()[0].prompt.text;
    assert!(text.contains("回复🚌"));
    assert!(text.contains("bus message status"));
    assert!(text.chars().count() < 4_500);
}
