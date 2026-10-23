use super::*;
use serde_json::json;

#[test]
fn reply_capture_diagnostics_bound_tokens_and_never_log_unknown_events_or_errors() {
    let capture = crate::utils::logging::test_capture::Capture::default();
    capture.run(|| {
        for value in [
            json!({"hook_event_name":"PRIVATE_EVENT", "session_id":"PRIVATE ID",
                "turn_id":"x".repeat(129), "last_assistant_message":true}),
            json!({}),
        ] {
            observe_payload(ProviderKind::Codex, &value, &Err("PRIVATE_ERROR".into()));
        }
    });
    let log = capture.text();
    for field in [
        "hook_event=\"other\"",
        "hook_event=\"missing_or_invalid\"",
        "session_id=None",
        "turn_id=None",
        "final_state=\"invalid_type\"",
        "final_state=\"missing\"",
        "parse_outcome=\"invalid\"",
    ] {
        assert!(log.contains(field), "missing {field}: {log}");
    }
    assert!(
        !log.contains("PRIVATE") && !log.contains(&"x".repeat(129)),
        "{log}"
    );
}

#[test]
fn reply_capture_diagnostics_use_claude_and_cursor_correlation_fields() {
    for (provider, value) in [
        (
            ProviderKind::ClaudeCode,
            json!({"hook_event_name":"Stop", "session_id":"s",
            "prompt_id":"p", "last_assistant_message":"PRIVATE_REPLY"}),
        ),
        (
            ProviderKind::Cursor,
            json!({"hook_event_name":"afterAgentResponse", "conversation_id":"s",
            "generation_id":"p", "text":"PRIVATE_REPLY"}),
        ),
    ] {
        let capture = crate::utils::logging::test_capture::Capture::default();
        capture.run(|| observe_payload(provider, &value, &Ok(spool::Parsed::Ignore)));
        let log = capture.text();
        for field in [
            "session_id=Some(\"s\")",
            "turn_id=Some(\"p\")",
            "final_bytes=Some(13)",
        ] {
            assert!(log.contains(field), "missing {field}: {log}");
        }
        assert!(!log.contains("PRIVATE"), "{log}");
    }
}
