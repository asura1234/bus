//! Replies use the existing durable wake queue for every requester.
use super::*;

fn requester(provider: Provider, cross_room: bool) -> Fixture {
    let mut f = Fixture::new();
    let mut state = f.worker.state.clone();
    let home = if cross_room {
        state.create_room("requester-home").unwrap()
    } else {
        f.room
    };
    let author = state
        .create_agent(home, "requester", provider, f.dir.clone(), None)
        .unwrap();
    state
        .set_agent_runtime_identity(
            author,
            AgentRuntimeIdentity {
                launch_id: Some("requester".into()),
                terminal_id: Some("terminal".into()),
                pane_id: Some("pane".into()),
                session_id: Some("session".into()),
            },
        )
        .unwrap();
    state.confirm_hook_setup(author).unwrap();
    state
        .observe_status(author, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    f.worker.save(state).unwrap();
    crate::agents::providers::spool::initialize(
        &f.dir.join("callbacks/requester"),
        &crate::agents::providers::spool::Manifest {
            routing_key: crate::agents::providers::spool::RoutingKey(author.0),
            provider,
            launch_id: "requester".into(),
        },
    )
    .unwrap();
    // Reuse the fixture's destination/notice helpers, without binding this author.
    f.orch = author;
    f
}

fn explicit(f: &mut Fixture, source: RequestId, text: &str) {
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
    crate::agents::providers::spool::initialize(
        &f.dir.join("callbacks/worker"),
        &crate::agents::providers::spool::Manifest {
            routing_key: crate::agents::providers::spool::RoutingKey(f.agent.0),
            provider: Provider::ClaudeCode,
            launch_id: "worker".into(),
        },
    )
    .unwrap();
    let message = f.worker.state.request(source).unwrap().prompt.id;
    let response = f.call(
        "message.send",
        json!({"reply_to":message.0.to_string(),"as":f.agent.0.to_string(),"text":text}),
    );
    assert!(response.ok, "{response:?}");
    assert_eq!(response.result["stage"], "replied");
    // Advance the batching clock without sleeps or changing the reply.
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 3_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
}

fn assert_requester_delivery(provider: Provider, captured: bool) {
    for cross_room in [false, true] {
        let mut f = requester(provider, cross_room);
        let source = f.send(Author::Agent(f.orch), "request");
        if captured {
            f.finish(source, "GO requester");
        } else {
            explicit(&mut f, source, "GO requester");
        }
        f.worker.submit_ready().unwrap();
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "{provider:?}, captured={captured}");
        assert!(calls[0].contains("GO requester"));
        drop(calls);
        let history = f.call("room.history", json!({"room":f.room.0.to_string()}));
        assert!(history.ok, "{history:?}");
        let rows: Vec<_> = history.result["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["prompt"]["author"] == json!({"agent":f.agent.0})
                    && row["prompt"]["recipient_ids"] == json!([f.orch.0])
            })
            .collect();
        assert_eq!(rows.len(), 1, "{history:?}");
        assert_eq!(rows[0]["recipient_names"], json!(["requester"]));
    }
}

#[test]
fn claude_requester_receives_explicit_reply_in_terminal_and_history() {
    assert_requester_delivery(Provider::ClaudeCode, false);
}

#[test]
fn claude_requester_receives_captured_final_in_terminal_and_history() {
    assert_requester_delivery(Provider::ClaudeCode, true);
}

#[test]
fn codex_requester_receives_explicit_reply_in_terminal_and_history() {
    assert_requester_delivery(Provider::Codex, false);
}

#[test]
fn codex_requester_receives_captured_final_in_terminal_and_history() {
    assert_requester_delivery(Provider::Codex, true);
}

#[test]
fn cursor_requester_receives_explicit_reply_in_terminal_and_history() {
    assert_requester_delivery(Provider::Cursor, false);
}

#[test]
fn cursor_requester_receives_captured_final_in_terminal_and_history() {
    assert_requester_delivery(Provider::Cursor, true);
}

#[test]
fn grok_requester_receives_explicit_reply_in_terminal_and_history() {
    assert_requester_delivery(Provider::Grok, false);
}

#[test]
fn grok_requester_receives_captured_final_in_terminal_and_history() {
    assert_requester_delivery(Provider::Grok, true);
}

#[test]
fn kimi_requester_receives_explicit_reply_in_terminal_and_history() {
    assert_requester_delivery(Provider::Kimi, false);
}

#[test]
fn kimi_requester_receives_captured_final_in_terminal_and_history() {
    assert_requester_delivery(Provider::Kimi, true);
}
#[test]
fn requester_live_waiter_defers_then_flushed_ack_suppresses_terminal_copy() {
    let mut f = requester(Provider::Codex, false);
    let source = f.send(Author::Agent(f.orch), "request");
    f.finish(source, "Consumed on stdout");
    f.worker.test_reply_waiter_now = Some(std::time::Instant::now());
    let message = f.worker.state.request(source).unwrap().prompt.id;
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
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
fn requester_waiter_still_owns_the_message_when_its_first_recipient_is_deleted() {
    let mut f = requester(Provider::Codex, false);
    let mut state = f.worker.state.clone();
    let second = state
        .create_agent(
            f.room,
            "surviving-peer",
            Provider::ClaudeCode,
            f.dir.clone(),
            None,
        )
        .unwrap();
    let ids = state
        .submit_message_from(
            f.room,
            Draft {
                text: "both reply".into(),
                files: vec![],
                recipient_ids: [f.agent, second].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 4_000,
        )
        .unwrap();
    let message = state.request(ids[0]).unwrap().prompt.id;
    state.delete_agent(f.agent, io::now_ms() - 3_500).unwrap();
    state.finish_for_test(ids[1], "Surviving answer", io::now_ms() - 3_000);
    f.worker.save(state).unwrap();
    f.worker.test_reply_waiter_now = Some(std::time::Instant::now());
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert!(
        f.calls.lock().unwrap().is_empty(),
        "ownership belongs to the author, even if the first recipient has gone"
    );
    assert!(
        f.call(
            "message.seen",
            json!({"request":ids[1].0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.worker
            .state
            .requests()
            .filter(|r| r.bus_generated)
            .count(),
        0
    );
}

#[test]
fn requester_ack_cancels_queued_wake_without_a_false_outcome_to_the_orchestrator() {
    let mut f = requester(Provider::Codex, false);
    let source = f.send(Author::Agent(f.orch), "request");
    f.finish(source, "Consumed just after queueing");
    f.worker
        .state
        .observe_status(f.orch, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert!(
        f.call(
            "message.seen",
            json!({"request":source.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    assert_eq!(f.notices()[0].phase, RequestPhase::Abandoned);
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 3_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.worker.state.requests().count(),
        2,
        "a consumed reply must not become new work for the room orchestrator"
    );
    assert!(f.calls.lock().unwrap().is_empty());
}

#[test]
fn an_orchestrators_reply_notification_does_not_open_another_orchestrator_task() {
    let mut f = requester(Provider::Codex, false);
    f.agent = f.worker.state.orchestrator_of(f.room).unwrap().id;
    let source = f.send(Author::Agent(f.orch), "question for orchestrator");
    f.finish(source, "GO from orchestrator");
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let mut state = f.worker.state.clone();
    state.record_worker_exit(f.orch, io::now_ms() + 3_000);
    state
        .route_worker_replies(io::now_ms() + 6_000, &BTreeSet::new())
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.worker.state.requests().count(),
        2,
        "a reply notification must not generate a new open-task exit outcome"
    );
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[test]
fn requester_exited_async_waiter_releases_reply_at_lease_expiry() {
    let mut f = requester(Provider::Codex, false);
    let now = std::time::Instant::now();
    f.worker.test_reply_waiter_now = Some(now);
    let source = f.send(Author::Agent(f.orch), "request");
    let message = f.worker.state.request(source).unwrap().prompt.id;
    assert!(
        f.call(
            "message.status",
            json!({"message":message.0.to_string(),"as":f.orch.0.to_string()})
        )
        .ok
    );
    f.finish(source, "RELEASE requester");
    f.worker.test_reply_waiter_now = Some(now + Duration::from_millis(2_999));
    f.worker.submit_ready().unwrap();
    assert!(f.notices().is_empty());
    f.worker.test_reply_waiter_now = Some(now + Duration::from_secs(3));
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert!(f.calls.lock().unwrap()[0].contains("RELEASE requester"));
}

#[test]
fn requester_replies_survive_restart_between_request_and_answer_once() {
    let mut f = requester(Provider::Codex, true);
    let source = f.send(Author::Agent(f.orch), "request");
    f.reopen();
    explicit(&mut f, source, "Changed build spec");
    f.reopen();
    f.worker.submit_ready().unwrap();
    f.reopen();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[test]
fn requester_rename_uses_stable_id_and_deletion_does_not_retarget_replacement() {
    for deleted in [false, true] {
        let mut f = requester(Provider::Codex, false);
        let source = f.send(Author::Agent(f.orch), "request");
        let mut state = f.worker.state.clone();
        if deleted {
            state.delete_agent(f.orch, io::now_ms()).unwrap();
            state
                .create_agent(f.room, "requester", Provider::Codex, f.dir.clone(), None)
                .unwrap();
        } else {
            state.rename_agent(f.orch, "renamed-requester").unwrap();
        }
        f.worker.save(state).unwrap();
        f.finish(source, "Stable recipient");
        f.worker.submit_ready().unwrap();
        assert_eq!(f.notices().len(), usize::from(!deleted));
        assert_eq!(f.calls.lock().unwrap().len(), usize::from(!deleted));
        assert_eq!(
            f.worker
                .state
                .request(source)
                .unwrap()
                .reply()
                .unwrap()
                .text,
            "Stable recipient"
        );
    }
}

#[test]
fn several_recipients_reply_to_requester_without_waking_room_orchestrator() {
    let mut f = requester(Provider::Codex, false);
    let mut state = f.worker.state.clone();
    let second = state
        .create_agent(f.room, "second", Provider::ClaudeCode, f.dir.clone(), None)
        .unwrap();
    let ids = state
        .submit_message_from(
            f.room,
            Draft {
                text: "both reply".into(),
                files: vec![],
                recipient_ids: [f.agent, second].into(),
            },
            Author::Agent(f.orch),
            io::now_ms() - 4_000,
        )
        .unwrap();
    state.finish_for_test(ids[0], "first answer", io::now_ms() - 3_000);
    state.finish_for_test(ids[1], "second answer", io::now_ms() - 3_000);
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    let notices: Vec<_> = f
        .worker
        .state
        .requests()
        .filter(|r| r.bus_generated)
        .collect();
    assert_eq!(notices.len(), 2);
    assert!(notices.iter().all(|r| r.agent_id == f.orch));
    // A fresh idle recipient takes the first prompt alone; the other answer
    // follows through the same queue after that turn finishes.
    let first_delivery = f
        .worker
        .state
        .agent(f.orch)
        .unwrap()
        .current_request
        .unwrap();
    f.finish(first_delivery, "First reply consumed");
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.worker
            .state
            .requests()
            .filter(|r| r.bus_generated)
            .count(),
        2
    );
    let calls = f.calls.lock().unwrap().join("\n");
    assert!(calls.contains("first answer"));
    assert!(calls.contains("second answer"));
}

struct BusyDraft(Arc<Mutex<Vec<String>>>);
impl Transport for BusyDraft {
    fn request(&mut self, method: Method) -> Result<ResponseResult, TransportError> {
        let Method::AgentPromptIfIdle(prompt) = method else {
            panic!("busy draft must use idle guard: {method:?}")
        };
        self.0.lock().unwrap().push(prompt.text);
        Err(TransportError {
            message: "developer draft".into(),
            code: Some(crate::protocol::api::AGENT_COMPOSER_BUSY.into()),
            definitely_rejected: true,
        })
    }
}

#[test]
fn requester_busy_and_draft_replies_wait_in_normal_queue_until_idle() {
    let mut f = requester(Provider::Codex, false);
    let source = f.send(Author::Agent(f.orch), "request");
    f.finish(source, "Queued reply");
    f.worker
        .state
        .observe_status(f.orch, RuntimeStatus::Working, io::now_ms())
        .unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.notices()[0].phase, RequestPhase::Queued);
    assert!(f.calls.lock().unwrap().is_empty());
    f.worker
        .state
        .observe_status(f.orch, RuntimeStatus::Idle, io::now_ms())
        .unwrap();
    f.worker.transport = Box::new(BusyDraft(f.calls.clone()));
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices()[0].phase, RequestPhase::Queued);
    f.worker.transport = Box::new(Accepting(f.calls.clone()));
    f.worker.submit_ready().unwrap();
    assert_eq!(f.calls.lock().unwrap().len(), 2);
    assert_eq!(f.notices().len(), 1);
    assert_eq!(f.notices()[0].phase, RequestPhase::Active);
}

#[test]
fn reply_wake_turn_final_does_not_start_a_reply_ping_pong() {
    let mut f = requester(Provider::Codex, false);
    let source = f.send(Author::Agent(f.orch), "request");
    f.finish(source, "GO");
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let notice = f.notices()[0].id;
    f.finish(notice, "Used the GO");
    f.worker.submit_ready().unwrap();
    assert_eq!(f.worker.state.requests().count(), 2);
}

#[test]
fn intentional_reply_to_a_reply_notification_still_reaches_its_author() {
    let mut f = requester(Provider::Codex, false);
    let source = f.send(Author::Agent(f.orch), "request");
    f.finish(source, "GO");
    f.worker.submit_ready().unwrap();
    assert_eq!(f.notices().len(), 1);
    let message = f.notices()[0].prompt.id;
    let reply = f.call("message.send", json!({"reply_to":message.0.to_string(),"as":f.orch.0.to_string(),"text":"Need a revised GO"}));
    assert!(reply.ok, "{reply:?}");
    let mut state = f.worker.state.clone();
    state
        .route_worker_replies(io::now_ms() + 3_000, &BTreeSet::new())
        .unwrap();
    assert_eq!(
        state
            .requests()
            .filter(|r| r.agent_id == f.agent
                && r.prompt.author == Author::Agent(f.orch)
                && r.prompt.text.contains("Need a revised GO"))
            .count(),
        1
    );
}

#[test]
fn shared_captured_final_reaches_each_author_and_waiter_ack_is_author_scoped() {
    for acknowledge in [false, true] {
        let mut f = requester(Provider::Codex, false);
        let orchestrator = f.worker.state.orchestrator_of(f.room).unwrap().id;
        let first = f.send(Author::Agent(f.orch), "peer request");
        let second = f.send(Author::Agent(orchestrator), "orchestrator request");
        let mut state = f.worker.state.clone();
        state.coalesce_queue(first).unwrap();
        state.finish_for_test(first, "Shared final", io::now_ms() - 3_000);
        state.finish_for_test(second, "Shared final", io::now_ms() - 3_000);
        f.worker.save(state).unwrap();
        if acknowledge {
            assert!(
                f.call(
                    "message.seen",
                    json!({"request":first.0.to_string(),"as":f.orch.0.to_string()})
                )
                .ok
            );
        }
        f.worker.submit_ready().unwrap();
        assert_eq!(f.notices().len(), usize::from(!acknowledge));
        assert_eq!(
            f.worker
                .state
                .requests()
                .filter(|r| r.bus_generated && r.agent_id == orchestrator)
                .count(),
            1
        );
    }
}

#[test]
fn copy_sent_to_someone_else_does_not_consume_requesters_reply() {
    let mut f = requester(Provider::Codex, false);
    let source = f.send(Author::Agent(f.orch), "request");
    f.finish(source, "Same result");
    let orchestrator = f.worker.state.orchestrator_of(f.room).unwrap().id;
    let mut state = f.worker.state.clone();
    state
        .submit_message_from(
            f.room,
            Draft {
                text: "Same result".into(),
                files: vec![],
                recipient_ids: [orchestrator].into(),
            },
            Author::Agent(f.agent),
            io::now_ms() - 2_000,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.worker.submit_ready().unwrap();
    assert_eq!(
        f.notices().len(),
        1,
        "a copy addressed elsewhere cannot reach this requester"
    );
}

#[test]
fn legacy_orchestrator_group_routes_keep_their_seen_or_pending_state_on_upgrade() {
    for seen in [false, true] {
        let mut f = Fixture::new();
        let lead = f.send(Author::Developer, "developer request");
        let member = f.send(Author::Agent(f.orch), "orchestrator request");
        let mut state = f.worker.state.clone();
        state.coalesce_queue(lead).unwrap();
        state.finish_for_test(lead, "Legacy final", io::now_ms() - 3_000);
        state.finish_for_test(member, "Legacy final", io::now_ms() - 3_000);
        f.worker.save(state).unwrap();
        let mut document = serde_json::to_value(&f.worker.state).unwrap();
        document
            .as_object_mut()
            .unwrap()
            .remove("reply_routes_by_request");
        // Old versions keyed the orchestrator member's route by the group's lead.
        document["reply_routes"] = json!({lead.0.to_string(): if seen {"seen"} else {"pending"}});
        let state = serde_json::from_value::<BusState>(document).unwrap();
        f.worker.save(state).unwrap();
        f.reopen();
        f.worker.submit_ready().unwrap();
        f.reopen();
        f.worker.submit_ready().unwrap();
        assert_eq!(f.notices().len(), usize::from(!seen));
        assert_eq!(f.calls.lock().unwrap().len(), usize::from(!seen));
    }
}
