use super::*;

#[test]
fn deletion_retry_reconciles_launch_attested_initial_session_without_callbacks_or_delivery() {
    for provider in [Provider::ClaudeCode, Provider::Cursor, Provider::Codex] {
        let (mut worker, agent, room, dir, _) = fixture(provider, vec![]);
        let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
        identity.session_id = None;
        worker
            .state
            .set_agent_runtime_identity(agent, identity)
            .unwrap();
        queue(&mut worker, room, agent, "never deliver");
        let calls = Arc::new(Mutex::new(Vec::new()));
        worker.transport = Box::new(NativeBoundClose {
            session: "native-session".into(),
            calls: Arc::clone(&calls),
            state_path: dir.join("state.json"),
        });
        let (events, _) = mpsc::channel();
        assert!(worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .is_err());
        assert!(worker.state.agent(agent).unwrap().deletion_pending);
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"sessionStart"} else {"SessionStart"}, "session_id":"native-session", "conversation_id":"native-session"}),
        );
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"beforeSubmitPrompt"} else {"UserPromptSubmit"}, "session_id":"native-session", "conversation_id":"native-session", "prompt_id":"p", "turn_id":"p", "generation_id":"p", "prompt":"never deliver"}),
        );
        record(
            &dir,
            provider,
            json!({"hook_event_name":if provider == Provider::Cursor {"afterAgentResponse"} else {"Stop"}, "session_id":"native-session", "conversation_id":"native-session", "prompt_id":"p", "turn_id":"p", "generation_id":"p", "last_assistant_message":"never publish", "text":"never publish"}),
        );
        worker
            .command(BusCommand::DeleteAgent(agent), &events)
            .unwrap();
        assert!(worker.state.agent(agent).is_none());
        assert!(worker.state.room(room).unwrap().latest_replies.is_empty());
        assert_eq!(worker.state.requests().count(), 0);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![None, None, Some("native-session".into())]
        );
        assert_eq!(
            callbacks::records(&dir.join("callbacks/launch"))
                .unwrap()
                .len(),
            3
        );
        drop(worker);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn deletion_initial_session_reconciliation_rejects_conflicting_launch_sessions() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let mut identity = worker.state.agent(agent).unwrap().runtime_identity.clone();
    identity.session_id = None;
    worker
        .state
        .set_agent_runtime_identity(agent, identity)
        .unwrap();
    worker.state.prepare_delete_agent(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    for session in ["first", "second"] {
        record(
            &dir,
            Provider::ClaudeCode,
            json!({"hook_event_name":"SessionStart", "session_id":session}),
        );
    }
    assert!(worker
        .reconcile_deleting_initial_session(agent)
        .unwrap_err()
        .contains("Conflicting"));
    assert!(worker
        .state
        .agent(agent)
        .unwrap()
        .runtime_identity
        .session_id
        .is_none());
    assert!(worker.state.agent(agent).unwrap().deletion_pending);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deletion_never_rebinds_a_known_session_from_a_later_session_start() {
    let (mut worker, agent, _, dir, _) = fixture(Provider::ClaudeCode, vec![]);
    let calls = Arc::new(Mutex::new(Vec::new()));
    worker.transport = Box::new(NativeBoundClose {
        session: "rebound".into(),
        calls: Arc::clone(&calls),
        state_path: dir.join("state.json"),
    });
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart", "session_id":"rebound"}),
    );
    let (events, received) = mpsc::channel();
    // The agent is deleted, but its terminal is never closed under the
    // rebound session: that session may not be Bus's to end.
    worker
        .command(BusCommand::DeleteAgent(agent), &events)
        .unwrap();
    assert!(worker.state.agent(agent).is_none());
    assert_eq!(*calls.lock().unwrap(), vec![Some("session".into())]);
    assert!(received
        .try_iter()
        .any(|event| matches!(event, BusEvent::TerminalsLeftOpen(_))));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_claude_rebinds_to_its_fresh_session_start() {
    let (mut worker, agent, _room, dir, calls) = fixture(Provider::ClaudeCode, vec![]);
    worker.state.begin_session_reset(agent).unwrap();
    worker.state.record_compaction(agent, 5).unwrap();
    worker.save(worker.state.clone()).unwrap();
    record(
        &dir,
        Provider::ClaudeCode,
        json!({"hook_event_name":"SessionStart","session_id":"fresh","source":"clear"}),
    );
    // The first pass rebinds; the retained SessionStart is consumed on the next.
    for _ in 0..2 {
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    let cleared = agent_of(&worker, agent);
    assert_eq!(
        cleared.runtime_identity.session_id.as_deref(),
        Some("fresh")
    );
    assert!(!cleared.session_binding_invalidated);
    assert!(!cleared.session_reset_pending);
    assert_eq!(cleared.compactions.count, 0);
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    assert!(calls.lock().unwrap().contains(&"pane.report_agent_session"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_codex_answers_a_message_sent_into_its_fresh_session() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Codex, vec![]);
    worker.state.begin_session_reset(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "fresh task");
    worker.submit_ready().unwrap();
    // Codex starts the fresh session with the first turn after /clear.
    for value in [
        json!({"hook_event_name":"SessionStart","session_id":"fresh","source":"startup"}),
        json!({"hook_event_name":"UserPromptSubmit","session_id":"fresh","turn_id":"t","prompt":"fresh task"}),
        json!({"hook_event_name":"Stop","session_id":"fresh","turn_id":"t","last_assistant_message":"FRESH"}),
    ] {
        record(&dir, Provider::Codex, value);
    }
    for _ in 0..2 {
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    worker
        .state
        .observe_status(agent, RuntimeStatus::Idle, 100)
        .unwrap();
    assert_eq!(
        worker.state.request(request).unwrap().phase,
        RequestPhase::Completed
    );
    assert_eq!(
        worker.state.room(room).unwrap().latest_replies[&agent].text,
        "FRESH"
    );
    assert_eq!(
        agent_of(&worker, agent)
            .runtime_identity
            .session_id
            .as_deref(),
        Some("fresh")
    );
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_cursor_rebinds_from_the_first_turn_of_its_new_chat() {
    let (mut worker, agent, room, dir, _) = fixture(Provider::Cursor, vec![]);
    worker.state.begin_session_reset(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    let request = queue(&mut worker, room, agent, "fresh task");
    worker.submit_ready().unwrap();
    // Cursor sends no sessionStart for /new-chat; the turn names the new chat.
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"fresh","generation_id":"t","prompt":"fresh task"}),
    );
    for _ in 0..2 {
        worker
            .consume_callbacks(agent, &dir.join("callbacks/launch"))
            .unwrap();
    }
    let cleared = agent_of(&worker, agent);
    assert_eq!(
        cleared.runtime_identity.session_id.as_deref(),
        Some("fresh")
    );
    assert!(!cleared.session_binding_invalidated);
    let request = worker.state.request(request).unwrap();
    assert!(request.trusted_start_bound);
    assert_eq!(request.provider_session_id.as_deref(), Some("fresh"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleared_cursor_keeps_its_session_when_the_server_refuses_the_new_chat() {
    // A server older than the Cursor new-chat rule answers the report with ok
    // but still attributes the old session to the pane.
    let (mut worker, agent, _room, dir, _) = fixture(
        Provider::Cursor,
        vec![
            Ok(ResponseResult::Ok {}),
            Ok(ResponseResult::AgentInfo {
                agent: native_info("pane", "cursor", "session"),
            }),
        ],
    );
    worker.state.begin_session_reset(agent).unwrap();
    worker.save(worker.state.clone()).unwrap();
    record(
        &dir,
        Provider::Cursor,
        json!({"hook_event_name":"beforeSubmitPrompt","conversation_id":"fresh","generation_id":"t","prompt":"fresh task"}),
    );
    worker
        .consume_callbacks(agent, &dir.join("callbacks/launch"))
        .unwrap();
    let refused = agent_of(&worker, agent);
    assert_eq!(
        refused.runtime_identity.session_id.as_deref(),
        Some("session")
    );
    assert!(refused.session_binding_invalidated);
    assert_eq!(refused.status, RuntimeStatus::Unavailable);
    assert!(refused
        .actionable_error
        .as_deref()
        .is_some_and(|error| error.contains("kept the previous provider session")));
    assert!(callbacks::records(&dir.join("callbacks/launch"))
        .unwrap()
        .is_empty());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
