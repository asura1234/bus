use super::*;

fn master_reply(author_is_agent: bool, command: &str) {
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
    let work_room = state.create_room("desktop").unwrap();
    state.bind_orchestrator(sender, work_room).unwrap();
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
        format!("desktop master (room {}):\nstatus please", f.room.0)
    } else {
        "status please".into()
    };
    assert_eq!(f.typed_content(), [(payload.clone(), false)]);
    f.started("turn-1", &payload);
    f.consume();
    f.status(RuntimeStatus::Working);
    f.record(json!({"hook_event_name":"Stop","session_id":"session","prompt_id":"turn-1",
        "last_assistant_message":"Dispatched; available for your next message.",
        "background_tasks":[{"id":"dispatch-1","type":"shell","status":"running","command":command}],"session_crons":[]}));
    f.consume();
    f.status(RuntimeStatus::Idle);
    f.consume();
    assert_eq!(
        f.reply(id).as_deref(),
        Some("Dispatched; available for your next message."),
        "agent sender={author_is_agent}, command={command}"
    );
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
        master_reply(author_is_agent, command);
    }
}

#[test]
fn developer_and_agent_master_messages_deliver_answers_despite_work_and_wrappers() {
    for author_is_agent in [false, true] {
        for command in [
            "cargo test && bus send --to worker --text ready",
            "./rev_send.sh worker brief.md",
            "cd /tmp && until bus state; do sleep 3; done; ./rev_send.sh worker brief.md",
        ] {
            master_reply(author_is_agent, command);
        }
    }
}
