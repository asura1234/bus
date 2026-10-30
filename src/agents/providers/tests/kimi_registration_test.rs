use super::kimi::wire::committed_final;
use super::{launch, parse, spool::Parsed, ProviderKind};

#[test]
fn kimi_hooks_bind_session_and_started_turn_without_treating_stop_as_final() {
    let session = serde_json::json!({"hook_event_name":"SessionStart","session_id":"session_abc","source":"startup"});
    assert_eq!(
        parse(ProviderKind::Kimi, &session).unwrap(),
        Parsed::Session {
            session: "session_abc".into(),
            source: Some("startup".into())
        }
    );
    let start = serde_json::json!({"hook_event_name":"TurnStarted","session_id":"session_abc","turn_id":7,"origin_kind":"user","prompt":"Bus task"});
    assert_eq!(
        parse(ProviderKind::Kimi, &start).unwrap(),
        Parsed::Started {
            session: "session_abc".into(),
            turn: "7".into(),
            prompt: "Bus task".into()
        }
    );
    let stop = serde_json::json!({"hook_event_name":"Stop","session_id":"session_abc","stop_hook_active":false});
    assert_eq!(
        parse(ProviderKind::Kimi, &stop).unwrap(),
        Parsed::ReconcileSession {
            session: "session_abc".into()
        }
    );
}

#[test]
fn kimi_resume_options_retain_yolo_but_drop_new_session_file() {
    let options = vec![
        "--yolo".into(),
        "--agent-file".into(),
        "/private/agent.md".into(),
    ];
    assert_eq!(
        launch::resumable_options(ProviderKind::Kimi, &options),
        vec!["--yolo"]
    );
    assert_eq!(
        super::kimi::launch::resume_args("session_abc"),
        vec!["--session", "session_abc"]
    );
}

#[test]
fn completed_main_turn_publishes_only_its_final_text() {
    let wire = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":7,\"promptId\":\"prompt-7\",\"input\":[{\"type\":\"text\",\"text\":\"Bus task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"think\",\"think\":\"private\"},{\"type\":\"text\",\"text\":\"Final answer\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":7,\"outcome\":\"done\"}\n",
        "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":7,\"reason\":\"completed\"}\n",
        "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"prompt-7\",\"reason\":\"completed\"}\n",
    );
    assert_eq!(
        committed_final(wire, 7, "Bus task").as_deref(),
        Some("Final answer")
    );
    assert_eq!(committed_final(wire, 7, "Other task"), None);
}

#[test]
fn stop_or_partial_wire_never_claims_a_reply() {
    let partial = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":3,\"promptId\":\"prompt-3\",\"input\":[{\"type\":\"text\",\"text\":\"Task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Premature\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":3,\"outcome\":\"done\"}\n",
    );
    assert_eq!(committed_final(partial, 3, "Task"), None);
    assert_eq!(
        committed_final(&(partial.to_owned() + "{\"type\":\"turn.end"), 3, "Task"),
        None
    );
}

#[test]
fn interrupted_turn_never_claims_visible_assistant_text() {
    let wire = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":4,\"promptId\":\"prompt-4\",\"input\":[{\"type\":\"text\",\"text\":\"Task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Interrupted answer\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":4,\"outcome\":\"interrupted\"}\n",
        "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":4,\"reason\":\"interrupted\"}\n",
        "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"prompt-4\",\"reason\":\"interrupted\"}\n",
    );
    assert_eq!(committed_final(wire, 4, "Task"), None);
}

#[test]
fn assistant_text_after_turn_end_cannot_replace_the_final() {
    let wire = concat!(
        "{\"type\":\"turn.prompt\",\"agentId\":\"main\",\"turnId\":9,\"promptId\":\"prompt-9\",\"input\":[{\"type\":\"text\",\"text\":\"Task\"}]}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Correct\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"agent.turn.ended\",\"turnId\":9,\"outcome\":\"done\"}\n",
        "{\"type\":\"turn.ended\",\"agentId\":\"main\",\"turnId\":9,\"reason\":\"completed\"}\n",
        "{\"type\":\"agent.message.appended\",\"message\":{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"Injected\"}],\"toolCalls\":[]},\"meta\":{\"source\":\"llm\"}}}\n",
        "{\"type\":\"prompt.completed\",\"agentId\":\"main\",\"promptId\":\"prompt-9\",\"reason\":\"completed\"}\n",
    );
    assert_eq!(committed_final(wire, 9, "Task").as_deref(), Some("Correct"));
}
