use super::*;

fn join(f: &mut Fixture, text: &str) -> RequestId {
    let id = f.send(text, false);
    f.deliver();
    f.started("turn-1", text);
    f.consume();
    assert!(f.request(id).steered && f.request(id).trusted_start_bound);
    id
}

fn fail(f: &mut Fixture) -> String {
    let reason =
        match f.provider {
            Provider::ClaudeCode => {
                f.record(
                    json!({"hook_event_name":"StopFailure","session_id":"session",
                "prompt_id":"turn-1","error":"server_error",
                "last_assistant_message":"API Error: Overloaded"}),
                );
                "API Error: Overloaded (server_error)"
            }
            Provider::Cursor => {
                f.record(json!({"hook_event_name":"stop","conversation_id":"session",
                "generation_id":"turn-1","status":"error"}));
                "Cursor turn error; inspect its terminal."
            }
            Provider::Codex => {
                let rollout = f.dir.join("rollout-steer-error.jsonl");
                std::fs::write(&rollout, json!({"type":"event_msg","payload":{
                "type":"task_complete","turn_id":"turn-1",
                "error":{"message":"Model at capacity","codex_error_info":"server_overloaded"}
            }}).to_string()).unwrap();
                // Keep the real Codex no-Stop path: settlement reads the rollout.
                f.record(
                    json!({"hook_event_name":"UserPromptSubmit","session_id":"session",
                "turn_id":"turn-1","prompt":"original work","transcript_path":rollout}),
                );
                f.consume();
                f.status(RuntimeStatus::Idle);
                f.now += crate::messaging::model::TURN_SETTLE_MS;
                f.status(RuntimeStatus::Idle);
                "Model at capacity (server_overloaded)"
            }
        };
    f.consume();
    reason.into()
}

fn message_status(f: &Fixture, id: RequestId) -> Value {
    f.worker
        .control_message_at(f.request(id).prompt.id, f.now)
        .unwrap()
}

fn expected(f: &Fixture, lead: RequestId, joined: RequestId, reason: &str) -> String {
    format!(
        "Message {}: Delivered into builder's running turn (started by message {}); that turn ended on a provider error: {reason}. The recipient saw your text but may not have acted on it; resend if needed.",
        f.request(joined).prompt.id.0, f.request(lead).prompt.id.0, reason = reason.trim_end_matches('.')
    )
}

fn reopen(f: &mut Fixture) {
    let placeholder = Worker::open(
        f.dir.join("restart-placeholder"),
        Box::new(Native(f.native.clone())),
    )
    .unwrap();
    drop(std::mem::replace(&mut f.worker, placeholder));
    f.worker = Worker::open(f.dir.clone(), Box::new(Native(f.native.clone()))).unwrap();
}

fn check_failure(provider: Provider, count: usize, restart: bool, notice_only: bool) {
    let mut f = Fixture::new(provider);
    // Enable the real durable worker-outcome notice path.
    let mut state = f.worker.state.clone();
    let orch = state
        .create_agent(
            state.master_room().unwrap().id,
            "orch",
            Provider::ClaudeCode,
            f.dir.clone(),
            None,
        )
        .unwrap();
    state.bind_orchestrator(orch, f.room).unwrap();
    f.worker.save(state).unwrap();
    let lead = running(&mut f, "original work", "turn-1");
    let joined: Vec<_> = (0..count)
        .map(|i| join(&mut f, &format!("correction {i}")))
        .collect();
    let queued = f.send("independent next task", true);
    if restart {
        // Reopen from the persisted store, then reattach the agent to the same
        // provider conversation before its failing end arrives.
        reopen(&mut f);
        f.status(RuntimeStatus::Working);
        assert_eq!(f.request(joined[0]).group, Some(lead));
    }
    let reason = fail(&mut f);
    assert_eq!(f.request(lead).phase, RequestPhase::Abandoned);
    assert_eq!(f.request(queued).phase, RequestPhase::Queued);
    assert_eq!(f.request(queued).group, None);
    assert_eq!(
        message_status(&f, queued)["requests"][0]["detail"],
        Value::Null
    );
    for id in &joined {
        assert_eq!(f.request(*id).phase, RequestPhase::Abandoned);
        let status = message_status(&f, *id);
        assert_eq!(status["message_id"], f.request(*id).prompt.id.0);
        if !notice_only {
            assert_eq!(
                status["requests"][0]["detail"],
                expected(&f, lead, *id, &reason)
            );
        }
    }
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(f.now);
    f.worker.save(state).unwrap();
    let events = f.worker.state.worker_events_for(f.request(joined[0]));
    assert_eq!(events.len(), 1, "one group outcome, no duplicate notices");
    for id in &joined {
        assert!(
            events[0].1.contains(&expected(&f, lead, *id, &reason)),
            "{}",
            events[0].1
        );
    }
    let before = message_status(&f, joined[0]);
    reopen(&mut f);
    assert_eq!(
        message_status(&f, joined[0])["requests"][0]["detail"],
        before["requests"][0]["detail"]
    );
    let mut state = f.worker.state.clone();
    state.retain_worker_outcomes(f.now + 1);
    assert_eq!(
        state.worker_events_for(state.request(joined[0]).unwrap()),
        events
    );
}

#[test]
fn steer_failure_codex_names_the_joined_turn_and_own_message() {
    check_failure(Provider::Codex, 1, false, false);
}

#[test]
fn steer_failure_claude_names_the_joined_turn_and_own_message() {
    check_failure(Provider::ClaudeCode, 1, false, false);
}

#[test]
fn steer_failure_cursor_names_the_joined_turn_and_own_message() {
    check_failure(Provider::Cursor, 1, false, false);
}

#[test]
fn steer_failure_several_messages_keep_their_own_subject_after_restart_and_resume() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        check_failure(provider, 3, true, false);
    }
}

#[test]
fn steer_failure_notices_explain_each_steered_messages_outcome() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        check_failure(provider, 3, false, true);
    }
}

#[test]
fn steer_failure_lead_text_is_unchanged_and_queued_work_gets_its_own_turn() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original work", "turn-1");
        join(&mut f, "correction");
        let queued = f.send("independent next task", true);
        let reason = fail(&mut f);
        let status = message_status(&f, lead);
        assert_eq!(status["requests"][0]["detail"], Value::Null);
        if provider == Provider::Cursor {
            assert_eq!(status["requests"][0]["reason"], Value::Null);
            assert_eq!(f.error().as_deref(), Some(reason.as_str()));
        } else {
            assert_eq!(
                status["requests"][0]["reason"],
                format!("provider_error: {reason}")
            );
            assert_eq!(
                f.error(),
                Some(format!(
                    "Provider turn failed: {reason}; request closed. Resend it to retry."
                ))
            );
        }
        f.status(RuntimeStatus::Idle);
        f.deliver();
        assert_eq!(f.request(queued).group, None);
        assert!(!f.typed().last().unwrap().1);
        f.started("turn-2", "independent next task");
        f.stopped("turn-2", "next task done");
        f.consume();
        assert_eq!(f.reply(queued).as_deref(), Some("next task done"));
        assert_eq!(
            message_status(&f, queued)["requests"][0]["detail"],
            Value::Null
        );
    }
}

#[test]
fn steer_failure_successful_steers_still_share_the_answer_without_error_guidance() {
    for provider in [Provider::ClaudeCode, Provider::Codex, Provider::Cursor] {
        let mut f = Fixture::new(provider);
        let lead = running(&mut f, "original work", "turn-1");
        let joined = join(&mut f, "correction");
        f.stopped("turn-1", "done");
        f.consume();
        for id in [lead, joined] {
            assert_eq!(f.reply(id).as_deref(), Some("done"));
            assert_eq!(message_status(&f, id)["requests"][0]["detail"], Value::Null);
            assert_eq!(message_status(&f, id)["requests"][0]["reason"], Value::Null);
        }
    }
}
