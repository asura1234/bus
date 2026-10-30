use super::*;
use crate::messaging::{control::Request as ControlRequest, native::TransportError};
use serde_json::json;

#[path = "history_upgrade_test.rs"]
mod history_upgrade;

#[path = "worker_events_test.rs"]
mod worker_events;

#[path = "exit_snapshot_test.rs"]
mod exit_snapshot;

#[path = "rejected_notice_test.rs"]
mod rejected_notice;

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
        Self::with_provider(Provider::ClaudeCode)
    }

    fn with_provider(provider: Provider) -> Self {
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
            .create_agent(room, "builder", provider, dir.clone(), None)
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
fn reply_waiter_lease_keeps_the_reply_until_exactly_three_seconds() {
    let mut f = Fixture::new();
    let now = std::time::Instant::now();
    f.worker.test_reply_waiter_now = Some(now);
    let source = f.send(Author::Agent(f.orch), "build this");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.finish(source, "One reply after the waiter expires");
    f.worker.test_reply_waiter_now = Some(now + Duration::from_millis(2_999));
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty(), "the waiter still owns the reply");
    f.worker.test_reply_waiter_now = Some(now + Duration::from_secs(3));
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert!(f.notices()[0]
        .prompt
        .text
        .contains("One reply after the waiter expires"));
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

#[test]
fn reply_routing_seen_reply_is_not_rerouted_after_archive_and_restart() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Printed and acknowledged");
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.worker
        .archive_history(io::now_ms() + crate::messaging::model::ARCHIVE_AFTER_MS)
        .unwrap();
    f.reopen();
    assert!(
        f.worker.state.request(source).is_some(),
        "the archived source must still be retained"
    );
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn reply_routing_developer_lead_group_routes_its_orchestrator_member_once() {
    for acknowledge in [false, true] {
        let mut f = Fixture::new();
        let lead = f.send(Author::Developer, "developer asks");
        let member = f.send(Author::Agent(f.orch), "orchestrator asks");
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
        f.worker.submit_ready().unwrap();
        assert_eq!(f.notices().len(), usize::from(!acknowledge));
        if !acknowledge {
            let message = f.worker.state.request(member).unwrap().prompt.id;
            assert!(f.notices()[0]
                .prompt
                .text
                .contains(&format!("Reply to message {} ", message.0)));
        }
    }
}

#[test]
fn reply_routing_preserves_an_old_recovered_reply_before_first_routing() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "build during downtime".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 9 * 24 * 60 * 60_000,
        )
        .unwrap()[0];
    // Final callbacks carry their original event time. The tick archives
    // recovered callbacks before its delivery reconciliation runs.
    state.finish_for_test(
        source,
        "Recovered from the spool",
        io::now_ms() - 8 * 24 * 60 * 60_000,
    );
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.notices().len(),
        1,
        "an old spooled reply still owes a wake"
    );
    assert!(f.calls.lock().unwrap()[0].contains("Recovered from the spool"));
    assert!(f.worker.state.request(source).is_some());
}

struct NeverShown(Arc<Mutex<Vec<String>>>);
impl Transport for NeverShown {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        match method {
            Method::AgentPromptIfIdle(p) => self.0.lock().unwrap().push(p.text),
            other => panic!("unexpected native call: {other:?}"),
        }
        Err(TransportError {
            code: Some(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN.into()),
            message: crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE.into(),
            definitely_rejected: true,
        })
    }
}

#[test]
fn reply_routing_notice_stops_pasting_after_the_queued_stall_window() {
    let mut f = Fixture::new();
    let pasted = Arc::new(Mutex::new(Vec::new()));
    f.worker.transport = Box::new(NeverShown(pasted.clone()));
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Reply the composer never shows");
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(pasted.lock().unwrap().len(), 1, "first paste attempt");
    // Every later tick past the queued stall window, after each 20-second
    // prompt-not-shown pause, would paste and clear the composer again.
    for minutes in [3, 30, 300] {
        f.worker
            .expire_queued_requests(io::now_ms() + minutes * 60_000)
            .unwrap();
        f.worker.withdrawn_at.clear();
        f.worker.submit_ready().unwrap();
    }
    assert_eq!(
        pasted.lock().unwrap().len(),
        1,
        "an ordinary queued request closes after QUEUED_STALL_MS; the exempt notice keeps pasting into and clearing the orchestrator composer"
    );
    assert_eq!(f.notices()[0].phase, RequestPhase::Queued);
}

#[test]
fn reply_routing_stalled_delivery_resumes_on_fresh_evidence_or_restart() {
    for recovery in ["status", "dialog", "session", "restart"] {
        let mut f = Fixture::new();
        let pasted = Arc::new(Mutex::new(Vec::new()));
        f.worker.transport = Box::new(NeverShown(pasted.clone()));
        let source = f.send(Author::Agent(f.orch), "build this");
        f.finish(source, "A retained reply");
        f.worker.submit_ready().unwrap();
        let notice = f.notices()[0].id;
        f.worker
            .expire_queued_requests(io::now_ms() + 3 * 60_000)
            .unwrap();
        f.worker.withdrawn_at.clear();
        f.worker.submit_ready().unwrap();
        assert_eq!(pasted.lock().unwrap().len(), 1, "paused: {recovery}");
        if recovery == "restart" {
            f.reopen();
        } else {
            let mut state = f.worker.state.clone();
            if recovery == "status" {
                state
                    .observe_status(f.orch, RuntimeStatus::Working, io::now_ms())
                    .unwrap();
            } else if recovery == "dialog" {
                state
                    .observe_dialog(f.orch, true, state.agent(f.orch).unwrap().observed_at_ms)
                    .unwrap();
            } else {
                let mut identity = state.agent(f.orch).unwrap().runtime_identity.clone();
                identity.session_id = Some("new-session".into());
                state.set_agent_runtime_identity(f.orch, identity).unwrap();
            }
            f.worker.save(state).unwrap();
            f.worker.expire_queued_now().unwrap();
            let mut state = f.worker.state.clone();
            state
                .observe_status(f.orch, RuntimeStatus::Idle, io::now_ms())
                .unwrap();
            state
                .observe_dialog(f.orch, false, state.agent(f.orch).unwrap().observed_at_ms)
                .unwrap();
            f.worker.save(state).unwrap();
        }
        f.worker.transport = Box::new(Accepting(f.calls.clone()));
        f.worker.submit_ready().unwrap();
        f.worker.submit_ready().unwrap();
        assert_eq!(f.calls.lock().unwrap().len(), 1, "resumed: {recovery}");
        assert_eq!(f.notices().len(), 1);
        assert_eq!(
            f.worker.state.request(notice).unwrap().phase,
            RequestPhase::Active
        );
    }
}

#[test]
fn reply_routing_ordinary_agent_message_stops_pasting_after_the_queued_stall_window() {
    let mut f = Fixture::new();
    let pasted = Arc::new(Mutex::new(Vec::new()));
    f.worker.transport = Box::new(NeverShown(pasted.clone()));
    let mut state = f.worker.state.clone();
    let id = state
        .submit_message_from(
            f.room,
            Draft {
                text: "builder (agent 3). Blocked-style message".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            io::now_ms(),
        )
        .unwrap()[0];
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(pasted.lock().unwrap().len(), 1, "first paste attempt");
    for minutes in [3, 30, 300] {
        f.worker
            .expire_queued_requests(io::now_ms() + minutes * 60_000)
            .unwrap();
        f.worker.withdrawn_at.clear();
        f.worker.submit_ready().unwrap();
    }
    assert_eq!(pasted.lock().unwrap().len(), 1);
    assert_eq!(
        f.worker.state.request(id).unwrap().phase,
        RequestPhase::Abandoned
    );
}

#[test]
fn reply_routing_paused_notice_does_not_hold_later_messages_to_the_orchestrator() {
    let mut f = Fixture::new();
    let pasted = Arc::new(Mutex::new(Vec::new()));
    f.worker.transport = Box::new(NeverShown(pasted.clone()));
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "A reply the composer rejected");
    f.worker.submit_ready().unwrap();
    f.worker
        .expire_queued_requests(io::now_ms() + 3 * 60_000)
        .unwrap();
    f.worker.withdrawn_at.clear();
    f.worker.submit_ready().unwrap();
    assert_eq!(
        pasted.lock().unwrap().len(),
        1,
        "the reply notice is paused"
    );
    // The composer accepts again without any status change, and a worker's
    // Blocked notice arrives for the orchestrator.
    f.worker.transport = Box::new(Accepting(f.calls.clone()));
    let mut state = f.worker.state.clone();
    let blocked = state
        .submit_message_from(
            f.room,
            Draft {
                text: "builder (agent 3). Blocked: approve a command".into(),
                files: vec![],
                recipient_ids: [f.orch].into(),
            },
            Author::Agent(f.agent),
            io::now_ms(),
        )
        .unwrap()[0];
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    f.worker.submit_ready().unwrap();
    f.worker
        .expire_queued_requests(io::now_ms() + 3 * 60_000)
        .unwrap();
    assert!(
        !f.calls.lock().unwrap().is_empty(),
        "a later message to the orchestrator was never attempted; it ended {:?}",
        f.worker.state.request(blocked).unwrap().phase
    );
}

#[test]
fn reply_routing_old_recovered_reply_is_released_once_delivered() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "build during downtime".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 9 * 24 * 60 * 60_000,
        )
        .unwrap()[0];
    state.finish_for_test(source, "Recovered", io::now_ms() - 8 * 24 * 60 * 60_000);
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.worker.submit_ready().unwrap();
    let notice = f.notices()[0].id;
    let mut state = f.worker.state.clone();
    state.finish_for_test(notice, "Read it", io::now_ms());
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert!(
        f.worker.state.request(source).is_some(),
        "a delivered reply stays in the permanent audit archive"
    );
    assert_eq!(f.calls.lock().unwrap().len(), 1, "no second wake");
}

#[test]
fn reply_routing_expired_seen_reply_does_not_reappear_from_append_only_history() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "build during downtime".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 9 * 24 * 60 * 60_000,
        )
        .unwrap()[0];
    state.finish_for_test(
        source,
        "Already printed",
        io::now_ms() - 8 * 24 * 60 * 60_000,
    );
    f.worker.save(state).unwrap();
    // First archive appends this source. Expiring routing metadata later
    // must not make that permanent source become new work on restart.
    f.worker.archive_history(io::now_ms()).unwrap();
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.worker.archive_history(io::now_ms()).unwrap();
    assert!(f.worker.state.request(source).is_some());
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "expired seen history must never become a pending reply again"
    );
    assert!(f.worker.state.request(source).is_some());
}

#[test]
fn reply_routing_pending_group_survives_after_its_developer_lead_expires() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let draft = Draft {
        text: "shared old turn".into(),
        files: vec![],
        recipient_ids: [f.agent].into(),
    };
    let at = io::now_ms() - 9 * 24 * 60 * 60_000;
    let lead = state
        .submit_message_from(f.room, draft.clone(), Author::Developer, at)
        .unwrap()[0];
    let member = state
        .submit_message_from(f.room, draft, Author::Agent(f.orch), at + 1)
        .unwrap()[0];
    state.coalesce_queue(lead).unwrap();
    for id in [lead, member] {
        state.finish_for_test(
            id,
            "Shared reply recovered",
            io::now_ms() - 8 * 24 * 60 * 60_000,
        );
    }
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    assert!(f.worker.state.request(lead).is_some());
    assert!(f.worker.state.request(member).is_some());
    f.reopen();
    f.worker.submit_ready().unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("Shared reply recovered"));
    assert_eq!(f.notices().len(), 1);
}

#[test]
fn reply_routing_coalesced_rejection_does_not_rearm_without_a_new_arrival() {
    let mut f = Fixture::new();
    let pasted = Arc::new(Mutex::new(Vec::new()));
    f.worker.transport = Box::new(NeverShown(pasted.clone()));
    let source = f.send(Author::Agent(f.orch), "first reply");
    f.finish(source, "first reply");
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.orch, RuntimeStatus::Unavailable, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    // Two collection windows still exercise native queue coalescing; a
    // same-window pair now intentionally becomes one worker-event batch.
    let source = f.send(Author::Agent(f.orch), "second reply");
    f.finish(source, "second reply");
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 3_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(f.notices().len(), 2);
    let mut state = f.worker.state.clone();
    state
        .observe_status(f.orch, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(pasted.lock().unwrap().len(), 1);
    for minutes in [3, 30, 300] {
        f.worker
            .expire_queued_requests(io::now_ms() + minutes * 60_000)
            .unwrap();
        f.worker.withdrawn_at.clear();
        f.worker.submit_ready().unwrap();
    }
    assert_eq!(
        pasted.lock().unwrap().len(),
        1,
        "restoring a rejected group is not a new arrival"
    );
    assert!(f.notices().iter().all(|r| r.phase == RequestPhase::Queued));
}

#[test]
fn late_final_reaches_orchestrator_member_of_a_developer_led_background_group() {
    let mut missing = Vec::new();
    for provider in [Provider::ClaudeCode, Provider::Codex] {
        let mut f = Fixture::with_provider(provider);
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
            .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms() - 10_000)
            .unwrap();
        f.worker.save(state).unwrap();
        let lead = f.send(Author::Developer, "developer asks");
        let member = f.send(Author::Agent(f.orch), "orchestrator asks");
        let mut state = f.worker.state.clone();
        let payload = state.coalesce_queue(lead).unwrap();
        state.begin_submission(lead, "worker", 0).unwrap();
        state
            .record_submission(
                lead,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("worker-session".into()),
                    provider_turn_id: None,
                },
            )
            .unwrap();
        let callback = |sequence, kind, prompt_payload| ProviderCallback {
            callback_id: format!("group-{sequence}"),
            sequence,
            occurred_at_ms: io::now_ms() - 4_000,
            agent_id: f.agent,
            launch_id: "worker".into(),
            provider_session_id: Some("worker-session".into()),
            provider_turn_id: Some("shared-turn".into()),
            provider_prompt_id: (provider == Provider::ClaudeCode).then_some("shared-turn".into()),
            prompt_payload,
            kind,
        };
        assert_eq!(
            state.accept_callback(callback(1, CallbackEventKind::PromptStarted, Some(payload))),
            CallbackDisposition::AcceptedBinding
        );

        state
            .observe_status(f.agent, RuntimeStatus::Idle, io::now_ms())
            .unwrap();
        assert_eq!(
            state.accept_callback(callback(
                2,
                CallbackEventKind::Final {
                    text: "Shared work still running.".into(),
                },
                None,
            )),
            CallbackDisposition::AcceptedCompleted
        );
        assert_eq!(
            state.request(member).unwrap().phase,
            RequestPhase::Completed
        );
        state
            .see_worker_reply(member, f.orch, io::now_ms())
            .unwrap();
        let original = state.request(member).unwrap().reply().cloned();
        for (index, mut invalid) in [
            callback(
                0,
                CallbackEventKind::Final {
                    text: "Stale result".into(),
                },
                None,
            ),
            callback(
                3,
                CallbackEventKind::Final {
                    text: "Foreign launch".into(),
                },
                None,
            ),
            callback(
                3,
                CallbackEventKind::Final {
                    text: "Foreign session".into(),
                },
                None,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            invalid.callback_id = format!("invalid-group-{index}");
            if index == 1 {
                invalid.launch_id = "another-launch".into();
            } else if index == 2 {
                invalid.provider_session_id = Some("another-session".into());
            }
            state.accept_callback(invalid);
        }
        state.accept_callback(callback(
            3,
            CallbackEventKind::Final {
                text: "Shared work actually finished.".into(),
            },
            None,
        ));
        state.accept_callback(callback(
            4,
            CallbackEventKind::Final {
                text: "Shared work actually finished.".into(),
            },
            None,
        ));
        assert_eq!(state.request(member).unwrap().reply().cloned(), original);
        assert_eq!(state.request(lead).unwrap().reply().cloned(), original);
        state
            .route_worker_replies(io::now_ms() + 10_000, &BTreeSet::new())
            .unwrap();
        f.worker.save(state).unwrap();
        f.worker.submit_ready().unwrap();
        let calls = f.calls.lock().unwrap();
        assert!(!calls
            .iter()
            .any(|text| text.contains("Stale result") || text.contains("Foreign")));
        if !calls.iter().any(|text| {
            text.contains("Late reply") && text.contains("Shared work actually finished.")
        }) {
            missing.push(provider);
        } else {
            assert_eq!(
                calls.len(),
                1,
                "late result must wake the sender exactly once"
            );
        }
    }
    assert!(
        missing.is_empty(),
        "an orchestrator's coalesced message still owes the changed late final: {missing:?}"
    );
}

#[test]
fn reply_routing_format_2_upgrade_keeps_old_history_without_waking_old_replies() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "build during last month".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 30 * 24 * 60 * 60_000,
        )
        .unwrap()[0];
    state.finish_for_test(source, "Old verdict", io::now_ms() - 29 * 24 * 60 * 60_000);
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    // Rewrite state.json as a format 2 binary leaves it: no recovery floor, and
    // the reply route of the long-expired source already pruned.
    let path = f.dir.join("state.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["version"] = json!(2);
    saved["state"]
        .as_object_mut()
        .unwrap()
        .remove("routing_before_ms");
    saved["state"]
        .as_object_mut()
        .unwrap()
        .remove("reply_routes");
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let history = std::fs::read(f.dir.join("history.jsonl")).unwrap();
    f.reopen();
    assert!(!f.worker.store.compact_history().unwrap());
    assert_eq!(std::fs::read(f.dir.join("history.jsonl")).unwrap(), history);
    // Production tick order: archive_when_due runs before submit_ready_while.
    f.worker.archive_history(io::now_ms()).unwrap();
    f.worker.submit_ready().unwrap();
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "an upgraded month-old reply must not wake the orchestrator again"
    );
    assert_eq!(
        f.worker
            .state
            .request(source)
            .unwrap()
            .reply()
            .map(|r| r.text.as_str()),
        Some("Old verdict")
    );
}

#[test]
fn reply_routing_format_2_upgrade_does_not_backfill_month_old_peer_outcomes() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let peer = state
        .create_agent(f.room, "peer", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "peer review last month".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(peer),
            io::now_ms() - 30 * 24 * 60 * 60_000,
        )
        .unwrap()[0];
    state.finish_for_test(
        source,
        "Old peer verdict",
        io::now_ms() - 29 * 24 * 60 * 60_000,
    );
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    // A format 2 binary pruned the outcome event of this expired source but
    // left its line in history.jsonl until the next start.
    let path = f.dir.join("state.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved["version"] = json!(2);
    for key in ["routing_before_ms", "reply_routes", "worker_events"] {
        saved["state"].as_object_mut().unwrap().remove(key);
    }
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    f.reopen();
    // Production tick order: expire_queued_now, poll, archive_when_due, submit.
    f.worker.expire_queued_now().unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.worker.submit_ready().unwrap();
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "a month-old peer outcome must not wake the orchestrator after upgrade: {:?}",
        f.calls.lock().unwrap()
    );
}

#[test]
fn reply_routing_persisted_floor_keeps_pruned_month_old_peer_outcomes_quiet() {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let peer = state
        .create_agent(f.room, "peer", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    let source = state
        .submit_message_from(
            f.room,
            Draft {
                text: "peer review last month".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(peer),
            io::now_ms() - 30 * 24 * 60 * 60_000,
        )
        .unwrap()[0];
    state.finish_for_test(
        source,
        "Old peer verdict",
        io::now_ms() - 29 * 24 * 60 * 60_000,
    );
    f.worker.save(state).unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    let path = f.dir.join("state.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(saved["state"]["routing_before_ms"].as_u64().unwrap() > 0);
    saved["state"]
        .as_object_mut()
        .unwrap()
        .remove("worker_events");
    std::fs::write(&path, serde_json::to_vec(&saved).unwrap()).unwrap();
    f.reopen();
    f.worker.expire_queued_now().unwrap();
    f.worker.archive_history(io::now_ms()).unwrap();
    f.worker.submit_ready().unwrap();
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "{:?}",
        f.calls.lock().unwrap()
    );
}

#[test]
fn identity_batched_worker_notification_has_exactly_one_sender_line() {
    let mut f = Fixture::new();
    let first = f.send(Author::Agent(f.orch), "task one");
    f.finish(first, "READY FOR VERIFY first");
    f.worker
        .state
        .route_worker_replies(io::now_ms(), &BTreeSet::new())
        .unwrap();
    let lead = f.notices()[0].id;
    // A second worker contributes another notification in the same queue.
    let peer = f
        .worker
        .state
        .create_agent(
            f.room,
            "rev-claude8",
            Provider::ClaudeCode,
            f.dir.clone(),
            None,
        )
        .unwrap();
    let id = f
        .worker
        .state
        .submit_message_from(
            f.room,
            Draft {
                text: "task two".into(),
                files: vec![],
                recipient_ids: [peer].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 4_000,
        )
        .unwrap()[0];
    f.worker
        .state
        .finish_for_test(id, "REVIEW READY second", io::now_ms() - 3_000);
    f.worker
        .state
        .route_worker_replies(io::now_ms(), &BTreeSet::new())
        .unwrap();
    let payload = f.worker.state.coalesce_queue(lead).unwrap();
    let builder_header = format!("builder (room {}):", f.room.0);
    let peer_header = format!("rev-claude8 (room {}):", f.room.0);
    assert_eq!(
        payload
            .lines()
            .filter(|line| *line == builder_header)
            .count(),
        1,
        "{payload}"
    );
    assert_eq!(
        payload.lines().filter(|line| *line == peer_header).count(),
        1,
        "{payload}"
    );
    assert_eq!(
        payload
            .lines()
            .filter(|line| line.starts_with("[Bus message ") && line.ends_with("from builder]"))
            .count(),
        1,
        "{payload}"
    );
    assert_eq!(
        payload
            .lines()
            .filter(|line| {
                line.starts_with("[Bus message ") && line.ends_with("from rev-claude8]")
            })
            .count(),
        1,
        "{payload}"
    );
    assert!(!payload.contains("(agent "), "{payload}");
    assert!(!payload.contains("answer with bus send"), "{payload}");
}

#[test]
fn reply_to_each_request_routes_once_and_reading_one_does_not_spend_the_other() {
    let mut f = Fixture::new();
    let first = f.send(Author::Agent(f.orch), "first watcher task");
    let second = f.send(Author::Agent(f.orch), "second watcher task");
    let mut document = serde_json::to_value(&f.worker.state).unwrap();
    document["requests"][first.0.to_string()]["phase"] = "active".into();
    document["requests"][second.0.to_string()]["phase"] = "active".into();
    document["requests"][second.0.to_string()]["group"] = first.0.into();
    let mut state: BusState = serde_json::from_value(document).unwrap();
    let first_message = state.request(first).unwrap().prompt.id;
    let second_message = state.request(second).unwrap().prompt.id;
    let now = io::now_ms() - 4000;
    state
        .answer_message(first_message, f.agent, "first answer".into(), now)
        .unwrap();
    state
        .answer_message(second_message, f.agent, "second answer".into(), now)
        .unwrap();
    state.see_worker_reply(first, f.orch, now).unwrap();
    state
        .route_worker_replies(io::now_ms(), &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    let notices = f.notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].prompt.text.contains("second answer"));
    assert!(!notices[0].prompt.text.contains("first answer"));
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 4000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    assert_eq!(
        f.notices().len(),
        1,
        "a repeated routing pass must not duplicate the answer"
    );
}

#[test]
fn reply_routing_keeps_worker_reply_through_orchestrator_startup_failure() {
    let mut f = Fixture::new();
    let source = f.send(Author::Agent(f.orch), "build this");
    f.finish(source, "Reply while orchestrator startup failed.");
    let mut state = f.worker.state.clone();
    state
        .set_agent_startup(
            f.orch,
            Some(crate::messaging::model::AgentStartup {
                command: "claude".into(),
                started_at_ms: 0,
                reported: true,
                native_terminal_id: None,
                recovery: None,
                failure: Some(crate::messaging::model::StartupFailure {
                    reason: "the native provider launch ended before startup was confirmed".into(),
                    recovery: "Inspect the agent's terminal.".into(),
                }),
            }),
        )
        .unwrap();
    state
        .observe_status(f.orch, RuntimeStatus::Unavailable, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    let notice = f.notices()[0].id;
    assert_eq!(
        f.worker.state.request(notice).unwrap().phase,
        RequestPhase::Queued
    );
    assert!(f.calls.lock().unwrap().is_empty());
    f.worker.expire_queued_requests(u64::MAX).unwrap();
    f.reopen();
    assert_eq!(
        f.worker.state.request(notice).unwrap().phase,
        RequestPhase::Queued
    );
    // The orchestrator is repaired and its startup confirmed.
    let mut state = f.worker.state.clone();
    state.set_agent_startup(f.orch, None).unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "deliver the retained reply exactly once");
    assert!(
        calls
            .iter()
            .any(|call| call.contains("Reply while orchestrator startup failed.")),
        "worker reply never reached the recovered orchestrator; notices: {:?}",
        f.notices()
            .iter()
            .map(|r| (r.phase, r.failure_reason.clone()))
            .collect::<Vec<_>>()
    );
}
