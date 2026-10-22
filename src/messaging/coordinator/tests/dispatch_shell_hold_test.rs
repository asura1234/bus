use super::*;

#[test]
fn same_turn_shell_hold_refresh_delivers_the_newest_answer_at_the_original_cap() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let lead = running(&mut f, "build it", "turn-1");
    claude_stop_holding(&f, "turn-1", "build pending", &["b1"]);
    f.consume();
    let since_ms = f.request(lead).shell_hold.as_ref().unwrap().since_ms;

    f.now += 5 * 60 * 1000;
    f.status(RuntimeStatus::Working);
    claude_stop_holding(&f, "turn-1", "build passed; server pending", &["b1"]);
    f.consume();
    assert!(f.request(lead).has_held_background_reply());
    let hold = f.request(lead).shell_hold.as_ref().unwrap();
    assert_eq!(hold.since_ms, since_ms);
    assert_eq!(hold.interim.text, "build passed; server pending");

    let cap_ms = 15 * 60 * 1000;
    f.now = since_ms + cap_ms - 2;
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Active);
    assert!(f.reply(lead).is_none());
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(f.request(lead).phase, RequestPhase::Completed);
    assert_eq!(
        f.reply(lead).as_deref(),
        Some("build passed; server pending")
    );
    assert_eq!(
        f.request(lead).failure_reason.as_deref(),
        Some("background_shells_timeout")
    );
}

#[test]
fn rejected_shell_final_preserves_the_answer_for_the_whole_joined_group() {
    let mut f = Fixture::new(Provider::ClaudeCode);
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let lead = running(&mut f, "question", "turn-1");
    let mut group = vec![lead];
    for text in ["authorization", "retraction"] {
        let joined = f.send(text, false);
        f.deliver();
        f.started("turn-1", text);
        f.consume();
        assert_eq!(f.request(joined).group, Some(lead));
        group.push(joined);
    }
    claude_stop_holding(&f, "turn-1", "first answer", &["b1"]);
    f.consume();
    assert!(f.request(lead).has_held_background_reply());

    // No new submit hook: this Stop is rejected as WrongTurn, just as in
    // the incident, but still carries captured text and the running shell.
    claude_stop_holding(&f, "turn-2", "retraction acknowledged", &["b1"]);
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.now += crate::messaging::model::DELIVERED_STALL_MS;
    for &id in &group {
        let message = f.request(id).prompt.id;
        let status = f.worker.control_message_at(message, f.now).unwrap();
        assert_ne!(status["requests"][0]["stage"], "stalled", "{status}");
        assert!(status["requests"][0]["reason"].is_null(), "{status}");
    }
    let hold = f.request(lead).shell_hold.as_ref().unwrap();
    assert_eq!(hold.interim.text, "first answer");
    assert_eq!(hold.interim.provider_turn_id.as_deref(), Some("turn-1"));

    // A valid hold can still release the captured answer at its cap.
    f.now += 15 * 60 * 1000;
    f.status(RuntimeStatus::Idle);
    f.consume();
    for id in group {
        assert_eq!(f.request(id).phase, RequestPhase::Completed);
        assert_eq!(f.reply(id).as_deref(), Some("first answer"));
        assert_eq!(
            f.request(id).failure_reason.as_deref(),
            Some("background_shells_timeout")
        );
    }
    let persisted = f.worker.store.load().unwrap().unwrap();
    assert_eq!(
        persisted.request(lead).unwrap().phase,
        RequestPhase::Completed
    );
}

fn master_reply(author_is_agent: bool, command: &str, held: bool) {
    let mut f = Fixture::in_room(Provider::ClaudeCode, true);
    let mut state = f.worker.state.clone();
    let sender = state
        .create_agent(
            f.room,
            "desktop master",
            Provider::ClaudeCode,
            f.dir.clone(),
            None,
        )
        .unwrap();
    f.worker.save(state).unwrap();
    f.record(json!({"hook_event_name":"SessionStart","session_id":"session","source":"startup"}));
    f.consume();
    let mut params = json!({"room":"master","to":["builder"],"text":"status please"});
    if author_is_agent {
        params["as"] = json!(sender.0.to_string());
    }
    let sent = f.worker.control_send(&params).unwrap();
    let message = PromptId(sent["message_id"].as_u64().unwrap());
    let id = f
        .worker
        .state
        .requests()
        .find(|r| r.prompt.id == message)
        .unwrap()
        .id;
    f.deliver();
    let payload = if author_is_agent {
        format!(
            "desktop master (agent {}, room {}):\nstatus please",
            sender.0, f.room.0
        )
    } else {
        "status please".into()
    };
    assert_eq!(f.typed(), [(payload.clone(), false)]);
    f.started("turn-1", &payload);
    f.consume();
    f.status(RuntimeStatus::Working);
    f.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"Dispatched; available for your next message.",
        "background_tasks":[{"id":"dispatch-1","type":"shell","status":"running","command":command}],"session_crons":[]}));
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    if held {
        assert_eq!(f.request(id).phase, RequestPhase::Active);
        assert!(f.request(id).shell_hold.is_some());
        assert!(f.reply(id).is_none());
    } else {
        assert_eq!(
            f.reply(id).as_deref(),
            Some("Dispatched; available for your next message."),
            "agent sender={author_is_agent}, command={command}"
        );
        assert!(f.request(id).shell_hold.is_none());
        let response = f.worker.control_response_with_events(
            &crate::messaging::control::Request {
                id: "history-proof".into(),
                method: "room.history".into(),
                params: json!({"room":"master"}),
            },
            None,
        );
        assert!(response.ok, "{response:?}");
        let history = response.result;
        assert_eq!(
            history["messages"][0]["delivery"]["requests"][0]["reply"]["text"],
            "Dispatched; available for your next message."
        );
        assert_eq!(history["room_id"], 0);
    }
}

#[test]
fn developer_master_messages_publish_direct_dispatch_replies() {
    direct_controls(false);
}

#[test]
fn agent_master_messages_publish_direct_dispatch_replies() {
    direct_controls(true);
}

fn direct_controls(author_is_agent: bool) {
    for command in [
        "bus send --room work --as orch --to worker --async --text go",
        "bus history --room 0",
        "bus agent read worker --source recent --lines 20",
    ] {
        master_reply(author_is_agent, command, false);
    }
}

#[test]
fn developer_and_agent_master_messages_keep_real_work_and_wrappers_held() {
    for author_is_agent in [false, true] {
        for command in [
            "cargo test && bus send --to worker --text ready",
            "./rev_send.sh worker brief.md",
            "cd /tmp && until bus state; do sleep 3; done; ./rev_send.sh worker brief.md",
        ] {
            master_reply(author_is_agent, command, true);
        }
    }
}

#[test]
fn shell_hold_diagnostics_identify_both_sender_paths_without_payloads() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        master_reply(
            false,
            "./private-script-name.sh PRIVATE_COMMAND_SECRET",
            true,
        );
        master_reply(
            true,
            "./private-script-name.sh PRIVATE_COMMAND_SECRET",
            true,
        );
        master_reply(true, "bus history --room 0", false);
    });
    let log = capture
        .text()
        .lines()
        .filter(|line| {
            line.contains("bus.callback.shell_classified")
                || line.contains("bus.callback.background_shells")
        })
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "bus.callback.shell_classified",
        "work_or_opaque_command",
        "bus_control",
        "bus.callback.background_shells",
        "request_id=",
        "prompt_id=",
        "author=\"developer\"",
        "author=\"agent\"",
        "captured_reply_bytes=",
    ] {
        assert!(log.contains(expected), "missing {expected}: {log}");
    }
    for private in [
        "PRIVATE_COMMAND_SECRET",
        "private-script-name",
        "Dispatched; available",
        "status please",
    ] {
        assert!(!log.contains(private), "payload leaked: {log}");
    }
}
