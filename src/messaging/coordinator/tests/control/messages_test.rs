use super::*;

#[test]
fn room_orchestrator_core_cli_send_durably_queues_and_returns_message_request_identity() {
    let (mut worker, room, agent, dir) = fixture();
    let second = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker
        .state
        .set_draft_text(room, "my unfinished draft")
        .unwrap();
    worker.state.set_draft_recipients(room, [second]).unwrap();
    let before = worker.state.room(room).unwrap().draft.clone();
    let result = call(
        &mut worker,
        "send-1",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"round trip"}),
    );
    assert!(result.ok, "{result:?}");
    assert_eq!(result.result["stage"], "queued");
    assert_eq!(worker.state.room(room).unwrap().draft, before);
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, agent);
    assert_eq!(requests[0].phase, RequestPhase::Queued);
    assert_eq!(requests[0].prompt.text, "round trip");
    let result2 = call(
        &mut worker,
        "send-1",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"round trip"}),
    );
    assert_eq!(result2.result, result.result);
    assert_eq!(worker.state.requests().count(), 1);
    let conflict = call(
        &mut worker,
        "send-1",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"different"}),
    );
    assert_eq!(conflict.error.unwrap().code, "id_conflict");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_and_history_preserve_explicit_recipient_order() {
    let (mut worker, room, codex, dir) = fixture();
    let claude = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let cursor = worker
        .state
        .create_agent(room, "cursor1", Provider::Cursor, dir.clone(), None)
        .unwrap();

    let sent = call(
        &mut worker,
        "ordered-send",
        "message.send",
        json!({"room":"test","to":["cursor1","codex1","claude1","cursor1"],"text":"review"}),
    );
    assert!(sent.ok, "{sent:?}");
    let message = sent.result["message_id"].clone();
    let request_agents = sent.result["request_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            worker
                .state
                .request(RequestId(id.as_u64().unwrap()))
                .unwrap()
                .agent_id
        })
        .collect::<Vec<_>>();
    assert_eq!(request_agents, [cursor, codex, claude]);

    let history = call(
        &mut worker,
        "ordered-history",
        "room.history",
        json!({"room":"test"}),
    );
    assert!(history.ok, "{history:?}");
    assert_eq!(history.result["messages"][0]["prompt"]["id"], message);
    let history_agents = history.result["messages"][0]["delivery"]["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["agent_id"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(history_agents, [cursor.0, codex.0, claude.0]);

    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_history_retains_twenty_ordered_messages_and_their_own_final_replies() {
    let (mut worker, _room, agent, dir) = fixture();
    worker
        .state
        .set_agent_runtime_identity(
            agent,
            AgentRuntimeIdentity {
                launch_id: Some("launch".into()),
                session_id: Some("session".into()),
                ..Default::default()
            },
        )
        .unwrap();
    let mut messages = Vec::new();
    for i in 0..20 {
        let receipt = call(
            &mut worker,
            &format!("send-{i}"),
            "message.send",
            json!({"room":"test","to":["codex1"],"text":format!("message-{i}")}),
        );
        assert!(receipt.ok, "{receipt:?}");
        messages.push(receipt.result["message_id"].clone());
        let request = RequestId(receipt.result["request_ids"][0].as_u64().unwrap());
        let seq = (i + 1) * 10;
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, seq)
            .unwrap();
        worker
            .state
            .begin_submission(request, "launch", seq)
            .unwrap();
        worker
            .state
            .record_submission(
                request,
                SubmissionOutcome::Confirmed {
                    provider_session_id: Some("session".into()),
                    provider_turn_id: Some(format!("turn-{i}")),
                },
            )
            .unwrap();
        assert_eq!(
            worker.state.accept_callback(ProviderCallback {
                callback_id: format!("start-{i}"),
                sequence: seq + 1,
                occurred_at_ms: seq + 1,
                agent_id: agent,
                launch_id: "launch".into(),
                provider_session_id: Some("session".into()),
                provider_turn_id: Some(format!("turn-{i}")),
                provider_prompt_id: None,
                prompt_payload: Some(format!("message-{i}")),
                kind: CallbackEventKind::PromptStarted
            }),
            CallbackDisposition::AcceptedBinding
        );
        worker.state.accept_callback(ProviderCallback::final_event(
            &format!("final-{i}"),
            seq + 2,
            agent,
            "launch",
            "session",
            &format!("turn-{i}"),
            &format!("message-{i}"),
            &format!("reply-{i}"),
        ));
        worker
            .state
            .observe_status(agent, RuntimeStatus::Idle, seq + 3)
            .unwrap();
    }
    worker.save(worker.state.clone()).unwrap();
    let result = call(
        &mut worker,
        "history",
        "room.history",
        json!({"room":"test"}),
    );
    let history = result.result["messages"].as_array().unwrap();
    assert_eq!(history.len(), 20);
    for (i, message) in history.iter().enumerate() {
        assert_eq!(message["prompt"]["id"], messages[i]);
        assert_eq!(message["delivery"]["complete"], true);
        assert_eq!(
            message["delivery"]["requests"][0]["reply"]["text"],
            format!("reply-{i}")
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_recipient_sets_and_failed_sends_never_modify_draft_or_broadcast_implicitly() {
    let (mut worker, room, first, dir) = fixture();
    let mut ids = vec![first];
    for name in ["second", "third", "fourth"] {
        ids.push(
            worker
                .state
                .create_agent(room, name, Provider::Codex, dir.clone(), None)
                .unwrap(),
        );
    }
    worker.state.set_draft_text(room, "keep me").unwrap();
    let draft = worker.state.room(room).unwrap().draft.clone();
    for size in 1..=4 {
        let to = if size == 4 {
            json!(["all"])
        } else {
            json!(ids[..size]
                .iter()
                .map(|id| id.0.to_string())
                .collect::<Vec<_>>())
        };
        let sent = call(
            &mut worker,
            &format!("set-{size}"),
            "message.send",
            json!({"room":"test","to":to,"text":"check"}),
        );
        assert!(sent.ok, "{sent:?}");
        let actual = sent.result["request_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                worker
                    .state
                    .request(RequestId(v.as_u64().unwrap()))
                    .unwrap()
                    .agent_id
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, ids[..size].iter().copied().collect());
        assert_eq!(worker.state.room(room).unwrap().draft, draft);
    }
    let count = worker.state.requests().count();
    for params in [
        json!({"room":"test","to":[],"text":"no"}),
        json!({"room":"test","text":"no"}),
        json!({"room":"test","to":["codex1"],"text":"no","files":["/does-not-exist/bus-file"]}),
    ] {
        assert!(
            !call(
                &mut worker,
                &format!("invalid-{params}"),
                "message.send",
                params
            )
            .ok
        );
        assert_eq!(worker.state.room(room).unwrap().draft, draft);
        assert_eq!(worker.state.requests().count(), count);
    }
    worker.storage_failed = true;
    assert_eq!(
        call(
            &mut worker,
            "storage",
            "message.send",
            json!({"room":"test","to":["all"],"text":"no"})
        )
        .error
        .unwrap()
        .code,
        "storage_unavailable"
    );
    assert!(call(&mut worker, "diag", "diagnostics", json!({})).ok);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_records_agent_author_and_skips_it_for_all() {
    let (mut worker, room, codex, dir) = fixture();
    let claude = worker
        .state
        .create_agent(room, "claude1", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    let other_room = worker.state.create_room("other").unwrap();
    worker
        .state
        .create_agent(other_room, "outsider", Provider::Codex, dir.clone(), None)
        .unwrap();

    let sent = call(
        &mut worker,
        "as-all",
        "message.send",
        json!({"room":"test","to":["all"],"text":"done","as":"codex1"}),
    );
    assert!(sent.ok, "{sent:?}");
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, claude);
    assert_eq!(requests[0].prompt.author, Author::Agent(codex));
    let status = call(
        &mut worker,
        "as-history",
        "room.history",
        json!({"room":"test"}),
    );
    assert_eq!(
        status.result["messages"][0]["prompt"]["author"],
        json!({"agent": codex})
    );

    let human = call(
        &mut worker,
        "as-default",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"hi"}),
    );
    assert!(human.ok, "{human:?}");
    let message = PromptId(human.result["message_id"].as_u64().unwrap());
    assert!(worker
        .state
        .requests()
        .filter(|r| r.prompt.id == message)
        .all(|r| r.prompt.author == Author::Human));

    for (id, to, author, expected) in [
        (
            "as-self",
            json!(["codex1"]),
            "codex1",
            "cannot send a message to itself",
        ),
        (
            "as-self-mixed",
            json!(["claude1", "codex1"]),
            "codex1",
            "cannot send a message to itself",
        ),
        (
            "as-outsider",
            json!(["claude1"]),
            "outsider",
            "No matching room or agent",
        ),
    ] {
        let failed = call(
            &mut worker,
            id,
            "message.send",
            json!({"room":"test","to":to,"text":"x","as":author}),
        );
        let message = failed.error.unwrap().message;
        assert!(message.contains(expected), "{id}: {message}");
    }

    worker.state.prepare_delete_agent(codex).unwrap();
    let deleting = call(
        &mut worker,
        "as-deleting",
        "message.send",
        json!({"room":"test","to":["claude1"],"text":"x","as":"codex1"}),
    );
    assert!(deleting.error.unwrap().message.contains("being deleted"));
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_the_only_room_agent_to_all_has_no_recipients() {
    let (mut worker, _room, _agent, dir) = fixture();
    let sent = call(
        &mut worker,
        "as-alone",
        "message.send",
        json!({"room":"test","to":["all"],"text":"x","as":"codex1"}),
    );
    assert!(
        error_message(&sent).starts_with("No recipients besides the author"),
        "{sent:?}"
    );
    assert_eq!(worker.state.requests().count(), 0);
    let empty = worker.state.create_room("empty").unwrap();
    let nobody = call(
        &mut worker,
        "all-empty",
        "message.send",
        json!({"room":"empty","to":["all"],"text":"x"}),
    );
    assert_eq!(
        error_message(&nobody),
        "No recipients: the room has no agents"
    );

    // Deleting the visible room moves the view where the UI goes: MASTER.
    worker.state.select_room(empty).unwrap();
    let deleted = call(
        &mut worker,
        "delete-visible",
        "room.delete",
        json!({"room":"empty","confirm":true}),
    );
    assert!(deleted.ok, "{deleted:?}");
    let state = call(&mut worker, "state-after-delete", "state", json!({}));
    assert_eq!(state.result["visible_room"], state.result["master_room"]);
    assert!(!state.result["visible_room"].is_null());
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_allows_only_the_rooms_own_master_orchestrator_from_outside() {
    let (mut worker, room, codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let other = worker.state.create_room("other").unwrap();
    let orchestrator = worker
        .state
        .create_agent(
            master,
            "claude-orch",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let elsewhere = worker
        .state
        .create_agent(master, "codex-orch", Provider::Codex, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(elsewhere, other).unwrap();
    worker
        .state
        .create_agent(master, "idle-orch", Provider::Codex, dir.clone(), None)
        .unwrap();

    let sent = call(
        &mut worker,
        "orch-as",
        "message.send",
        json!({"room":"test","to":["all"],"text":"plan","as":"claude-orch"}),
    );
    assert!(sent.ok, "{sent:?}");
    let requests = worker.state.requests().collect::<Vec<_>>();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].agent_id, codex);
    assert_eq!(requests[0].prompt.author, Author::Agent(orchestrator));

    // By ID too, and an orchestrator can address the MASTER room it lives in.
    let by_id = call(
        &mut worker,
        "orch-as-id",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"x","as":orchestrator.0.to_string()}),
    );
    assert!(by_id.ok, "{by_id:?}");
    let in_master = call(
        &mut worker,
        "orch-in-master",
        "message.send",
        json!({"room":"master","to":["idle-orch"],"text":"x","as":"claude-orch"}),
    );
    assert!(in_master.ok, "{in_master:?}");

    for (id, author) in [("orch-other", "codex-orch"), ("orch-none", "idle-orch")] {
        let rejected = call(
            &mut worker,
            id,
            "message.send",
            json!({"room":"test","to":["codex1"],"text":"x","as":author}),
        );
        assert!(
            error_message(&rejected).contains("in this room or its orchestrator"),
            "{id}: {rejected:?}"
        );
    }
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_as_an_ambiguous_room_agent_name_never_falls_back_to_the_orchestrator() {
    let (mut worker, room, _codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    for _ in 0..2 {
        worker
            .state
            .create_agent(room, "dev", Provider::Codex, dir.clone(), None)
            .unwrap();
    }
    let orchestrator = worker
        .state
        .create_agent(master, "dev", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    let sent = call(
        &mut worker,
        "as-ambiguous",
        "message.send",
        json!({"room":"test","to":["codex1"],"text":"x","as":"dev"}),
    );
    assert!(
        error_message(&sent).contains("Ambiguous"),
        "an ambiguous author must not resolve to the orchestrator: {sent:?}"
    );
    assert_eq!(worker.state.requests().count(), 0);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn dev_send_to_human_posts_an_orchestrator_report_in_master_history() {
    let (mut worker, room, _codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let orchestrator = worker
        .state
        .create_agent(
            master,
            "claude-orch",
            Provider::ClaudeCode,
            dir.clone(),
            None,
        )
        .unwrap();
    worker.state.bind_orchestrator(orchestrator, room).unwrap();
    assert_ne!(worker.state.visible_room(), Some(master));

    let sent = call(
        &mut worker,
        "report",
        "message.send",
        json!({"room":"master","to":["human"],"text":"gate passed; please restart","as":"claude-orch"}),
    );
    assert!(sent.ok, "{sent:?}");
    let result = &sent.result;
    assert_eq!(result["stage"], "posted");
    assert_eq!(result["request_ids"], json!([]));
    // No agent receives it, so it creates no request.
    assert_eq!(worker.state.requests().count(), 0);
    let master_room = worker.state.room(master).unwrap();
    let report = master_room.notices.last().unwrap();
    assert_eq!(report.author, Author::Agent(orchestrator));
    assert_eq!(report.text, "gate passed; please restart");
    assert!(report.recipient_ids.is_empty());
    assert_eq!(result["message_id"], json!(report.id));
    // The latest prompt is what rings and counts as unread, like a reply.
    assert_eq!(master_room.latest_prompt.as_ref(), Some(report));
    assert_eq!(master_room.unread_count, 1);

    for (id, params, expected) in [
        (
            "no-author",
            json!({"room":"master","to":["human"],"text":"x"}),
            "requires --as",
        ),
        (
            "work-room",
            json!({"room":"test","to":["human"],"text":"x","as":"claude-orch"}),
            "only in the MASTER room",
        ),
        (
            "mixed",
            json!({"room":"master","to":["human","claude-orch"],"text":"x","as":"claude-orch"}),
            "cannot be combined",
        ),
        (
            "queued",
            json!({"room":"master","to":["human"],"text":"x","as":"claude-orch","queue":true}),
            "--queue",
        ),
        (
            "empty",
            json!({"room":"master","to":["human"],"text":"  ","as":"claude-orch"}),
            "",
        ),
    ] {
        let rejected = call(&mut worker, id, "message.send", params);
        assert!(!rejected.ok, "{id}: {rejected:?}");
        assert!(
            error_message(&rejected).contains(expected),
            "{id}: {rejected:?}"
        );
    }
    assert_eq!(worker.state.room(master).unwrap().notices.len(), 1);
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn notices_older_versions_saved_as_bus_are_dropped_and_reports_to_the_human_kept() {
    let (mut worker, _room, _codex, dir) = fixture();
    let master = worker.state.master_room().unwrap().id;
    let orchestrator = worker
        .state
        .create_agent(master, "orch", Provider::ClaudeCode, dir.clone(), None)
        .unwrap();
    worker
        .state
        .post_to_human(master, orchestrator, "first report".into(), Vec::new(), 1)
        .unwrap();
    let mut saved = serde_json::to_value(&worker.state).unwrap();
    let notices = &mut saved["rooms"][master.0.to_string()]["notices"];
    let mut legacy = notices[0].clone();
    legacy["author"] = json!("bus");
    legacy["text"] = json!("orch never started message 9");
    notices.as_array_mut().unwrap().push(legacy);
    let loaded: BusState = serde_json::from_value(saved).unwrap();
    let notices = &loaded.room(master).unwrap().notices;
    assert_eq!(notices.len(), 1, "Bus is not an agent and authors nothing");
    assert_eq!(notices[0].text, "first report");
    drop(worker);
    std::fs::remove_dir_all(dir).unwrap();
}
