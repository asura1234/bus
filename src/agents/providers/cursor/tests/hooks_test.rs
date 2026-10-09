use super::hooks::parse;
use crate::agents::providers::{
    hook_json::{merge_hooks, HookContract},
    spool::Parsed,
    ProviderKind,
};
use serde_json::{json, Value};
use std::path::Path;

fn hook(event: &str) -> Value {
    json!({"hook_event_name":event,"conversation_id":"s","generation_id":"g","prompt":"prompt","text":"observed","status":"completed"})
}

#[test]
fn cursor_hooks_separate_response_completion_and_start_observations() {
    assert_eq!(
        parse(&hook("sessionStart")).unwrap(),
        Parsed::Session {
            session: "s".into(),
            source: None
        }
    );
    assert_eq!(
        parse(&hook("beforeSubmitPrompt")).unwrap(),
        Parsed::Started {
            session: "s".into(),
            turn: "g".into(),
            prompt: "prompt".into()
        }
    );
    assert_eq!(
        parse(&hook("afterAgentResponse")).unwrap(),
        Parsed::Response {
            session: "s".into(),
            turn: "g".into(),
            text: "observed".into()
        }
    );
    assert_eq!(
        parse(&hook("stop")).unwrap(),
        Parsed::Completed {
            session: "s".into(),
            turn: "g".into()
        }
    );
    let mut failure = hook("stop");
    failure["status"] = json!("aborted");
    assert_eq!(
        parse(&failure).unwrap(),
        Parsed::Failure {
            session: "s".into(),
            turn: "g".into(),
            message: "Cursor turn aborted; inspect its terminal.".into()
        }
    );
    // Cursor callbacks do not require a Codex transcript path.
    let mut response = hook("afterAgentResponse");
    response["transcript_path"] = Value::Null;
    assert!(matches!(parse(&response).unwrap(), Parsed::Response { .. }));
}

#[test]
fn cursor_missing_generation_and_unsupported_events_preserve_field_order() {
    let mut missing = hook("stop");
    missing.as_object_mut().unwrap().remove("generation_id");
    assert_eq!(parse(&missing).unwrap_err(), "Hook missing generation_id; update the CLI and verify Bus hooks. No prompt will be retried.");
    missing["generation_id"] = json!("g");
    missing.as_object_mut().unwrap().remove("status");
    assert!(parse(&missing)
        .unwrap_err()
        .starts_with("Hook missing status;"));
    assert_eq!(
        parse(&hook("Stop")).unwrap_err(),
        "Unsupported Bus hook event Stop"
    );
    missing["hook_event_name"] = json!("unknown");
    assert_eq!(
        parse(&missing).unwrap_err(),
        "Unsupported Bus hook event unknown"
    );
}

#[test]
fn cursor_subagent_exclusion_is_independent_of_missing_session_fields() {
    assert_eq!(
        parse(&json!({"hook_event_name":"stop","agent_id":"child"})).unwrap(),
        Parsed::Ignore
    );
    assert_eq!(
        parse(&json!({"hook_event_name":"SubagentStop"})).unwrap(),
        Parsed::Ignore
    );
    assert!(parse(&json!({"hook_event_name":"stop","agent_id":null}))
        .unwrap_err()
        .starts_with("Hook missing conversation_id;"));
}

#[test]
fn cursor_hook_merge_retains_user_version_entries_and_stop_shape() {
    let contract = HookContract::for_provider(ProviderKind::Cursor);
    assert_eq!(
        contract.events,
        [
            "sessionStart",
            "beforeSubmitPrompt",
            "afterAgentResponse",
            "stop"
        ]
    );
    let binary = Path::new("/tmp/bus fixture");
    let original = json!({"version":7,"custom":true,"hooks":{"stop":[{"command":"user-hook"}]}});
    let merged = merge_hooks(original.clone(), contract, binary).unwrap();
    assert_eq!(merged["version"], 7);
    assert_eq!(merged["custom"], true);
    assert_eq!(merged["hooks"]["stop"][0], original["hooks"]["stop"][0]);
    assert_eq!(
        merged["hooks"]["stop"][1],
        contract.entry(&contract.command(binary))
    );
    assert!(contract
        .command(binary)
        .ends_with(" --bus-callback cursor-hook"));
    assert_eq!(
        merge_hooks(merged.clone(), contract, binary).unwrap(),
        merged
    );
    assert_eq!(
        merge_hooks(json!({}), contract, binary).unwrap()["version"],
        1
    );
    assert!(merge_hooks(json!({"hooks":false}), contract, binary).is_err());
}
