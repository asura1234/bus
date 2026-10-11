use super::{callback, callbacks, order_records, AgentId, CallbackEventKind, ProviderKind};
use serde_json::{json, Value};
use std::path::PathBuf;

fn record(sequence: u64, provider: ProviderKind, value: Value) -> callbacks::Record {
    callbacks::Record {
        id: format!("record-{sequence}"),
        sequence,
        at_ms: 20 + sequence,
        manifest: callbacks::Manifest {
            routing_key: callbacks::RoutingKey(7),
            provider,
            launch_id: "launch".into(),
        },
        value,
        reporter: Vec::new(),
        nested_provider: false,
    }
}

#[test]
fn neutral_record_mapping_keeps_validated_owner_and_original_callback_metadata() {
    let record = record(3, ProviderKind::ClaudeCode, json!({}));
    let mapped = callback(
        &record,
        AgentId(7),
        "session".into(),
        "prompt".into(),
        Some("payload".into()),
        CallbackEventKind::PromptStarted,
    );
    assert_eq!(mapped.agent_id, AgentId(7));
    assert_eq!(mapped.callback_id, "record-3");
    assert_eq!(mapped.sequence, 3);
    assert_eq!(mapped.occurred_at_ms, 23);
    assert_eq!(mapped.launch_id, "launch");
    assert_eq!(mapped.provider_session_id.as_deref(), Some("session"));
    assert_eq!(mapped.provider_turn_id.as_deref(), Some("prompt"));
    assert_eq!(mapped.provider_prompt_id.as_deref(), Some("prompt"));
    assert_eq!(mapped.prompt_payload.as_deref(), Some("payload"));
}

#[test]
fn non_claude_record_mapping_does_not_invent_a_provider_prompt_id() {
    for provider in [ProviderKind::Codex, ProviderKind::Cursor] {
        let mapped = callback(
            &record(1, provider, json!({})),
            AgentId(7),
            "session".into(),
            "turn".into(),
            None,
            CallbackEventKind::Final {
                text: "reply".into(),
            },
        );
        assert_eq!(mapped.provider_turn_id.as_deref(), Some("turn"));
        assert_eq!(mapped.provider_prompt_id, None);
        assert_eq!(mapped.prompt_payload, None);
    }
}

#[test]
fn neutral_turn_order_keeps_session_then_submit_before_companion_finals() {
    let events = [
        (
            1,
            json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"first","last_assistant_message":"first reply"}),
        ),
        (
            2,
            json!({"hook_event_name":"Stop","session_id":"s","prompt_id":"second","last_assistant_message":"second reply"}),
        ),
        (
            3,
            json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt_id":"first","prompt":"first"}),
        ),
        (
            4,
            json!({"hook_event_name":"SessionStart","session_id":"s"}),
        ),
        (
            5,
            json!({"hook_event_name":"UserPromptSubmit","session_id":"s","prompt_id":"second","prompt":"second"}),
        ),
    ];
    let mut records = events
        .into_iter()
        .map(|(sequence, value)| {
            (
                PathBuf::from(format!("event-{sequence}.json")),
                record(sequence, ProviderKind::ClaudeCode, value),
            )
        })
        .collect();
    order_records(&mut records);
    assert_eq!(
        records
            .iter()
            .map(|(_, record)| record.sequence)
            .collect::<Vec<_>>(),
        [4, 3, 1, 5, 2]
    );
}
