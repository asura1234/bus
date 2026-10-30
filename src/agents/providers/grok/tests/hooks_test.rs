use super::*;
use serde_json::json;

fn event(name: &str) -> Value {
    json!({"hookEventName":name, "sessionId":"synthetic-session",
        "promptId":"opaque/turn:1", "prompt":"  Synthetic task\n",
        "reason":"end_turn", "lastAssistantMessage":"not yet a final"})
}

#[test]
fn grok_offline_session_start_preserves_native_identity() {
    let value = serde_json::from_str(include_str!("fixtures/session-start.json")).unwrap();
    assert_eq!(
        parse(&value).unwrap(),
        Parsed::Session {
            session: "01a162bf-5091-7c42-9b5c-47930d378acd".into(),
            source: Some("new".into())
        }
    );
}

#[test]
fn grok_offline_shutdown_and_session_end_are_ignored() {
    let value = serde_json::from_str(include_str!("fixtures/shutdown-stop.json")).unwrap();
    assert_eq!(parse(&value).unwrap(), Parsed::Ignore);
    for reason in ["shutdown", "channel_closed"] {
        let mut value = event("stop");
        value["reason"] = json!(reason);
        assert_eq!(parse(&value).unwrap(), Parsed::Ignore);
    }
    assert_eq!(parse(&event("session_end")).unwrap(), Parsed::Ignore);
}

#[test]
fn grok_prompt_and_stop_gate_are_scoped_facts() {
    assert_eq!(
        parse(&event("user_prompt_submit")).unwrap(),
        Parsed::Started {
            session: "synthetic-session".into(),
            turn: "opaque/turn:1".into(),
            prompt: "  Synthetic task\n".into()
        }
    );
    for active in [false, true] {
        let mut value = event("stop");
        value["stopHookActive"] = json!(active);
        assert_eq!(
            parse(&value).unwrap(),
            Parsed::CompletionCandidate {
                session: "synthetic-session".into(),
                turn: "opaque/turn:1".into()
            }
        );
    }
}

#[test]
fn grok_missing_or_wrong_required_identity_fails() {
    for name in [
        "session_start",
        "user_prompt_submit",
        "stop",
        "stop_failure",
        "stop_cancelled",
    ] {
        for bad in [Value::Null, json!(""), json!(7), json!(" ")] {
            let mut value = event(name);
            value["sessionId"] = bad;
            assert!(parse(&value).is_err(), "{value}");
        }
        if name != "session_start" {
            let mut value = event(name);
            value.as_object_mut().unwrap().remove("promptId");
            assert!(parse(&value).is_err(), "{value}");
        }
    }
    let mut value = event("user_prompt_submit");
    value.as_object_mut().unwrap().remove("prompt");
    assert!(parse(&value).is_err());
    assert!(parse(&json!({})).is_err());
}

#[test]
fn grok_children_and_unattributed_notifications_never_become_finals() {
    for name in [
        "session_start",
        "user_prompt_submit",
        "stop",
        "stop_failure",
        "stop_cancelled",
    ] {
        let mut value = event(name);
        value["subagentType"] = json!("explore");
        assert_eq!(parse(&value).unwrap(), Parsed::Ignore);
    }
    assert_eq!(
        parse(&json!({"hookEventName":"subagent_stop"})).unwrap(),
        Parsed::Ignore
    );
    for kind in ["idle_prompt", "task_complete", "permission_prompt"] {
        assert_eq!(
            parse(
                &json!({"hookEventName":"notification", "notificationType":kind,
            "sessionId":"synthetic-session"})
            )
            .unwrap(),
            if kind == "idle_prompt" {
                Parsed::ReconcileSession {
                    session: "synthetic-session".into(),
                }
            } else {
                Parsed::Ignore
            }
        );
    }
    assert!(parse(&event("future_unknown_event")).is_err());
}

#[test]
fn grok_failures_and_late_cancels_are_never_successes() {
    let mut value = event("stop_failure");
    value["error"] = json!("server_error");
    value["errorDetails"] = json!("Synthetic API failure");
    assert_eq!(
        parse(&value).unwrap(),
        Parsed::Failure {
            session: "synthetic-session".into(),
            turn: "opaque/turn:1".into(),
            message: "Grok turn failed; inspect its terminal.".into(),
            error: Some("Synthetic API failure (server_error)".into())
        }
    );
    value = event("stop_cancelled");
    value["reason"] = json!("user_interrupt");
    value["timestamp"] = json!("2099-01-01T00:00:00Z");
    match parse(&value).unwrap() {
        Parsed::Failure {
            session,
            turn,
            message,
            error,
        } => {
            assert_eq!(session, "synthetic-session");
            assert_eq!(turn, "opaque/turn:1");
            assert!(message.contains("cancelled"));
            assert_eq!(error.as_deref(), Some("user_interrupt"));
        }
        other => panic!("cancel became {other:?}"),
    }
}

#[test]
fn grok_imported_callbacks_filtered_without_suppressing_native_providers() {
    let grok = event("stop");
    let claude = json!({"hook_event_name":"Stop", "session_id":"claude", "prompt_id":"c"});
    let cursor = json!({"hook_event_name":"stop", "conversation_id":"cursor", "generation_id":"g"});
    for adapter in [
        ProviderKind::ClaudeCode,
        ProviderKind::Cursor,
        ProviderKind::Codex,
    ] {
        assert!(is_foreign_callback(adapter, &grok));
    }
    assert!(!is_foreign_callback(ProviderKind::Grok, &grok));
    assert!(is_foreign_callback(ProviderKind::Grok, &claude));
    assert!(is_foreign_callback(ProviderKind::Grok, &cursor));
    assert!(!is_foreign_callback(ProviderKind::ClaudeCode, &claude));
    assert!(!is_foreign_callback(ProviderKind::Cursor, &cursor));
    assert!(!is_foreign_callback(
        ProviderKind::Codex,
        &json!({"hook_event_name":"Stop", "session_id":"codex"})
    ));
    assert_eq!(parse(&claude).unwrap(), Parsed::Ignore);
}

#[test]
fn grok_1070_dual_spelling_is_native_on_grok_and_foreign_on_imported_adapters() {
    // Grok 1.0.70 emits Claude-compatible aliases beside its native fields.
    let value = json!({"hookEventName":"session_start", "hook_event_name":"SessionStart",
        "sessionId":"grok-1070", "session_id":"grok-1070", "source":"new"});
    assert!(!is_foreign_callback(ProviderKind::Grok, &value));
    assert_eq!(
        parse(&value).unwrap(),
        Parsed::Session {
            session: "grok-1070".into(),
            source: Some("new".into())
        }
    );
    for adapter in [
        ProviderKind::ClaudeCode,
        ProviderKind::Codex,
        ProviderKind::Cursor,
    ] {
        assert!(is_foreign_callback(adapter, &value));
    }
}

#[test]
fn grok_synthetic_gate_sequence_keeps_candidate_and_cancel_distinct() {
    let rows: Vec<Value> = include_str!("fixtures/turn-events.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(matches!(parse(&rows[0]).unwrap(), Parsed::Started { .. }));
    for row in &rows[1..3] {
        assert!(matches!(
            parse(row).unwrap(),
            Parsed::CompletionCandidate { .. }
        ));
    }
    assert!(matches!(parse(&rows[3]).unwrap(), Parsed::Failure { .. }));
}

#[test]
fn grok_native_provider_hooks_remain_decodable_after_foreign_filter() {
    for (adapter, value) in [
        (
            ProviderKind::ClaudeCode,
            json!({"hook_event_name":"SessionStart", "session_id":"native-claude"}),
        ),
        (
            ProviderKind::Cursor,
            json!({"hook_event_name":"sessionStart", "conversation_id":"native-cursor"}),
        ),
    ] {
        assert!(!is_foreign_callback(adapter, &value));
        assert!(matches!(
            crate::agents::providers::parse(adapter, &value).unwrap(),
            Parsed::Session { .. }
        ));
    }
}

#[test]
fn grok_session_scoped_or_unsupported_stop_reason_is_not_a_turn_candidate() {
    for reason in [Value::Null, json!(""), json!("future_reason")] {
        let mut value = event("stop");
        value["reason"] = reason;
        assert!(parse(&value).is_err());
    }
}
