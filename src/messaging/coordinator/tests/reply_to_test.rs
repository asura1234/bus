use super::*;
use crate::messaging::control::{Request as ControlRequest, Response};
use crate::messaging::model::RecordedTurnEnd;

fn answer(f: &mut Fixture, message: PromptId, author: AgentId, text: &str) -> Response {
    f.worker.control_response_with_events(
        &ControlRequest {
            id: format!("answer-{}-{}-{}", message.0, author.0, text),
            method: "message.send".into(),
            params: json!({"reply_to":message.0.to_string(),"as":author.0.to_string(),"text":text}),
        },
        None,
    )
}

fn start(f: &mut Fixture, text: &str) -> RequestId {
    let id = f.send(text, false);
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    f.started("infinite-turn", &payload);
    f.consume();
    f.status(RuntimeStatus::Working);
    assert!(f.request(id).trusted_start_bound);
    id
}

fn status(f: &mut Fixture, id: RequestId) -> Value {
    let message = f.request(id).prompt.id;
    f.worker.control_message_at(message, f.now).unwrap()
}

#[test]
fn reply_to_completes_the_lead_without_ending_its_turn_or_stopping_later_steering() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "standing watcher");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    let sent = answer(&mut f, message, agent, "watcher ready");
    assert!(sent.ok, "{sent:?}");
    let result = status(&mut f, lead);
    assert_eq!(result["complete"], true);
    assert_eq!(result["requests"][0]["reply"]["text"], "watcher ready");
    assert_eq!(result["requests"][0]["turn_ended"], false);
    assert_eq!(
        f.worker.state.agent(agent).unwrap().current_request,
        Some(lead)
    );
    let next = start(&mut f, "one task");
    assert_eq!(f.request(next).group, Some(lead));
    assert!(f.typed().last().unwrap().1);
    assert_eq!(status(&mut f, next)["complete"], false);
    assert_eq!(
        f.worker.state.requests().count(),
        2,
        "reply creates no reverse request"
    );

    f.stopped("infinite-turn", "outstanding work finished");
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.worker.settle_requests().unwrap();
    assert_eq!(
        status(&mut f, lead)["requests"][0]["reply"]["text"],
        "watcher ready"
    );
    assert_eq!(
        status(&mut f, next)["requests"][0]["reply"]["text"],
        "outstanding work finished"
    );
    assert_eq!(f.worker.state.agent(agent).unwrap().current_request, None);
    assert_ne!(
        f.request(lead).reply_key(),
        f.request(next).reply_key(),
        "reading the direct answer must not suppress the later group final"
    );
}

#[test]
fn reply_to_validates_message_and_recipient_and_deduplicates_the_answer() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "watch");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    let stranger = f
        .worker
        .state
        .create_agent(f.room, "stranger", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    assert!(!answer(&mut f, PromptId(999999), agent, "wrong ID").ok);
    assert!(!answer(&mut f, message, stranger, "not my work").ok);
    assert_eq!(status(&mut f, lead)["complete"], false);
    assert!(answer(&mut f, message, agent, "done").ok);
    let before = f.worker.state.clone();
    // A different control request ID must not create a second publication.
    f.worker.control_receipts.clear();
    assert!(answer(&mut f, message, agent, "done").ok);
    assert_eq!(f.worker.state, before);
    assert!(!answer(&mut f, message, agent, "conflicting answer").ok);
    assert_eq!(f.worker.state, before);
}

#[test]
fn reply_to_finishes_only_its_recipient_of_a_multi_recipient_message() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let other = f
        .worker
        .state
        .create_agent(f.room, "other", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    let ids = f
        .worker
        .state
        .submit_message_from(
            f.room,
            Draft {
                text: "both review".into(),
                files: vec![],
                recipient_ids: [f.agent, other].into(),
            },
            Author::Developer,
            f.now,
        )
        .unwrap();
    let message = f.request(ids[0]).prompt.id;
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    f.started("infinite-turn", &payload);
    f.consume();
    f.status(RuntimeStatus::Working);
    f.worker
        .state
        .set_agent_runtime_identity(
            other,
            AgentRuntimeIdentity {
                launch_id: Some("other-launch".into()),
                ..Default::default()
            },
        )
        .unwrap();
    f.worker
        .state
        .begin_submission(ids[1], "other-launch", 0)
        .unwrap();
    let agent = f.agent;
    assert!(answer(&mut f, message, agent, "first result").ok);
    let partial = status(&mut f, ids[0]);
    assert_eq!(partial["complete"], false);
    assert_eq!(partial["requests"][0]["stage"], "replied");
    assert_ne!(partial["requests"][1]["stage"], "replied");
    assert!(answer(&mut f, message, other, "second result").ok);
    let complete = status(&mut f, ids[0]);
    assert_eq!(complete["complete"], true);
    assert_eq!(complete["requests"][0]["reply"]["text"], "first result");
    assert_eq!(complete["requests"][1]["reply"]["text"], "second result");
}

#[test]
fn reply_to_is_durable_and_deletion_preserves_answered_success() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "watch forever");
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    assert!(answer(&mut f, message, agent, "done before delete").ok);
    let saved = JsonStore::new(f.dir.join("state.json"))
        .load()
        .unwrap()
        .unwrap();
    f.worker.state = saved;
    f.worker.control_receipts.clear();
    assert_eq!(
        status(&mut f, lead)["requests"][0]["reply"]["text"],
        "done before delete"
    );
    assert_eq!(
        f.worker.state.agent(agent).unwrap().current_request,
        Some(lead)
    );
    assert!(answer(&mut f, message, agent, "done before delete").ok);
    let mut deleted = f.worker.state.clone();
    deleted.delete_agent(agent, f.now + 100).unwrap();
    f.worker.save(deleted).unwrap();
    let after = status(&mut f, lead);
    assert_eq!(after["complete"], true);
    assert_eq!(after["requests"][0]["stage"], "replied");
    assert_eq!(after["requests"][0]["reason"], Value::Null);
    assert_eq!(after["requests"][0]["reply"]["text"], "done before delete");
}

#[test]
fn reply_to_delivery_includes_each_batched_message_id_and_exact_recipient_command() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.status(RuntimeStatus::Launching);
    let first = f.send("first", false);
    let second = f.send("second", false);
    f.deliver();
    f.status(RuntimeStatus::Idle);
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    for id in [first, second] {
        let message = f.request(id).prompt.id;
        assert!(
            payload.contains(&format!(
                "bus send --reply-to {} --as {}",
                message.0, f.agent.0
            )),
            "{payload}"
        );
    }
}

#[test]
fn reply_to_posts_only_in_the_original_conversation_even_when_sender_and_recipient_live_elsewhere()
{
    let mut f = Fixture::new(Provider::ClaudeCode);
    let sender_room = f.worker.state.create_room("sender-home").unwrap();
    let conversation = f.worker.state.create_room("conversation").unwrap();
    let sender = f
        .worker
        .state
        .create_agent(sender_room, "sender", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    let id = f
        .worker
        .state
        .submit_message_from(
            conversation,
            Draft {
                text: "answer here".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(sender),
            f.now,
        )
        .unwrap()[0];
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    f.started("infinite-turn", &payload);
    f.consume();
    let message = f.request(id).prompt.id;
    let agent = f.agent;
    assert!(answer(&mut f, message, agent, "original conversation result").ok);
    for room in [f.room, sender_room] {
        assert!(f.worker.state.room(room).unwrap().latest_replies.is_empty());
    }
    let reply = &f.worker.state.room(conversation).unwrap().latest_replies[&agent];
    assert_eq!(reply.request_id, id);
    assert_eq!(reply.text, "original conversation result");
    assert_eq!(f.worker.state.requests().count(), 1);
}

#[test]
fn reply_to_survives_a_fresh_coordinator_and_is_still_idempotent() {
    use crate::messaging::coordinator::tests::support::{
        consume, fixture, queue, record, reopen_saved,
    };
    let (mut worker, agent, room, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let id = queue(&mut worker, room, agent, "watch");
    worker.submit_ready().unwrap();
    let payload = worker
        .state
        .request(id)
        .unwrap()
        .submitted_payload
        .clone()
        .unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"UserPromptSubmit","session_id":"session","prompt_id":"infinite-turn","prompt":payload}),
    );
    consume(&mut worker, agent, &dir);
    let message = worker.state.request(id).unwrap().prompt.id;
    let params = json!({"reply_to":message.0.to_string(),"as":agent.0.to_string(),"text":"persisted answer"});
    assert!(
        worker
            .control_response_with_events(
                &ControlRequest {
                    id: "before-restart".into(),
                    method: "message.send".into(),
                    params: params.clone(),
                },
                None
            )
            .ok
    );
    drop(worker);
    let mut reopened = reopen_saved(&dir);
    assert_eq!(
        reopened.state.agent(agent).unwrap().current_request,
        Some(id)
    );
    let status = reopened
        .control_message_at(message, crate::messaging::storage::io::now_ms())
        .unwrap();
    assert_eq!(status["complete"], true);
    assert_eq!(status["requests"][0]["reply"]["text"], "persisted answer");
    let before = reopened.state.clone();
    assert!(
        reopened
            .control_response_with_events(
                &ControlRequest {
                    id: "after-restart".into(),
                    method: "message.send".into(),
                    params,
                },
                None
            )
            .ok
    );
    assert_eq!(reopened.state, before);
    drop(reopened);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn reply_to_does_not_hide_a_steering_start_callback_that_arrives_after_the_answer() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let lead = start(&mut f, "watch");
    let follow = f.send("answer promptly", false);
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    let message = f.request(follow).prompt.id;
    let agent = f.agent;
    assert!(answer(&mut f, message, agent, "done before hook consumption").ok);
    f.started("infinite-turn", &payload);
    f.consume();
    assert!(f.request(follow).trusted_start_bound);
    assert_eq!(
        f.request(follow).provider_turn_id.as_deref(),
        Some("infinite-turn")
    );
    assert_eq!(
        f.worker.state.agent(agent).unwrap().current_request,
        Some(lead)
    );
    assert_eq!(
        status(&mut f, follow)["requests"][0]["reply"]["text"],
        "done before hook consumption"
    );
}

#[test]
fn reply_to_does_not_publish_a_late_final_after_the_answered_anchor_is_abandoned() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    let master = f.worker.state.ensure_master_room();
    let sender = f
        .worker
        .state
        .create_agent(master, "orchestrator", Provider::Codex, f.dir.clone(), None)
        .unwrap();
    f.worker.state.bind_orchestrator(sender, f.room).unwrap();
    let lead = f
        .worker
        .state
        .submit_message_from(
            f.room,
            Draft {
                text: "watch forever".into(),
                files: vec![],
                recipient_ids: [f.agent].into(),
            },
            Author::Agent(sender),
            f.now,
        )
        .unwrap()[0];
    f.deliver();
    let payload = f.typed().last().unwrap().0.clone();
    f.started("infinite-turn", &payload);
    f.consume();
    f.status(RuntimeStatus::Working);
    let message = f.request(lead).prompt.id;
    let agent = f.agent;
    assert!(answer(&mut f, message, agent, "already answered").ok);

    let mut state = f.worker.state.clone();
    state
        .observe_status(agent, RuntimeStatus::Idle, f.now)
        .unwrap();
    assert_eq!(
        state.settle_ended_request(
            agent,
            f.now + crate::messaging::model::TURN_SETTLE_MS + 1,
            |_| None,
        ),
        Some(lead)
    );
    f.worker.save(state).unwrap();
    assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
    let notices = f.worker.state.notices(f.room).count();
    let events = serde_json::to_value(&f.worker.state).unwrap()["worker_events"].clone();
    let requests = f.worker.state.requests().count();

    f.stopped("infinite-turn", "a different eventual final");
    f.consume();
    assert_eq!(f.worker.state.notices(f.room).count(), notices);
    assert_eq!(
        serde_json::to_value(&f.worker.state).unwrap()["worker_events"],
        events
    );
    assert_eq!(f.worker.state.requests().count(), requests);
    assert_eq!(
        status(&mut f, lead)["requests"][0]["reply"]["text"],
        "already answered"
    );
}

#[test]
fn reply_to_success_does_not_become_a_resend_error_when_the_turn_fails() {
    for also_answer_follower in [false, true] {
        let mut f = Fixture::new(Provider::ClaudeCode);
        let lead = start(&mut f, "watch forever");
        let follower = also_answer_follower.then(|| start(&mut f, "completed follow-up"));
        let agent = f.agent;
        for id in std::iter::once(lead).chain(follower) {
            let message = f.request(id).prompt.id;
            assert!(answer(&mut f, message, agent, "answered mid-turn").ok);
        }
        f.record(json!({"hook_event_name":"StopFailure","session_id":"session",
            "prompt_id":"infinite-turn","error":"server_error","error_details":"Connection lost mid-response"}));
        f.consume();
        assert_eq!(f.worker.state.agent(agent).unwrap().current_request, None);
        assert_eq!(
            f.error(),
            None,
            "successful work must not be presented as closed/retryable"
        );
        for id in std::iter::once(lead).chain(follower) {
            let outcome = status(&mut f, id);
            assert_eq!(outcome["complete"], true);
            assert_eq!(outcome["requests"][0]["stage"], "replied");
            assert_eq!(outcome["requests"][0]["reason"], Value::Null);
            assert_eq!(outcome["requests"][0]["reply"]["text"], "answered mid-turn");
        }
    }
}

#[test]
fn reply_to_success_closes_without_a_missing_reply_error_on_idle_or_unavailable() {
    for runtime in [RuntimeStatus::Idle, RuntimeStatus::Unavailable] {
        for recorded_error in [false, true] {
            let mut f = Fixture::new(Provider::ClaudeCode);
            let lead = start(&mut f, "watch forever");
            let message = f.request(lead).prompt.id;
            let agent = f.agent;
            assert!(answer(&mut f, message, agent, "answered mid-turn").ok);
            let mut state = f.worker.state.clone();
            // Unavailable settlement applies after an observed turn end.
            state
                .observe_status(agent, RuntimeStatus::Idle, f.now)
                .unwrap();
            state.observe_status(agent, runtime, f.now).unwrap();
            assert_eq!(
                state.settle_ended_request(
                    agent,
                    f.now + crate::messaging::model::TURN_SETTLE_MS + 1,
                    |_| recorded_error.then(|| RecordedTurnEnd::Error("Connection lost".into())),
                ),
                Some(lead)
            );
            f.worker.save(state).unwrap();
            assert_eq!(f.worker.state.agent(agent).unwrap().current_request, None);
            assert_eq!(f.error(), None);
            let outcome = status(&mut f, lead);
            assert_eq!(outcome["complete"], true);
            assert_eq!(outcome["requests"][0]["reason"], Value::Null);
            assert_eq!(outcome["requests"][0]["reply"]["text"], "answered mid-turn");
        }
    }
}

#[test]
fn reply_to_does_not_hide_a_turn_error_for_an_outstanding_group_member() {
    for answer_lead in [false, true] {
        let mut f = Fixture::new(Provider::ClaudeCode);
        let lead = start(&mut f, "watch forever");
        let follower = start(&mut f, "follow-up");
        let (answered, outstanding) = if answer_lead {
            (lead, follower)
        } else {
            (follower, lead)
        };
        let message = f.request(answered).prompt.id;
        let agent = f.agent;
        assert!(answer(&mut f, message, agent, "answered mid-turn").ok);
        f.record(
            json!({"hook_event_name":"StopFailure","session_id":"session",
            "prompt_id":"infinite-turn","error":"server_error","error_details":"Connection lost"}),
        );
        f.consume();
        assert_eq!(
            status(&mut f, answered)["requests"][0]["reply"]["text"],
            "answered mid-turn"
        );
        let failed = status(&mut f, outstanding);
        assert_eq!(failed["requests"][0]["stage"], "abandoned");
        assert!(failed["requests"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("provider_error:"));
        assert!(f.error().unwrap().contains("Resend it to retry"));
        assert_eq!(f.worker.state.agent(agent).unwrap().current_request, None);
    }
}
