use super::hooks::parse;
use crate::agents::providers::{
    hook_json::{merge_hooks, HookContract},
    spool::Parsed,
    ProviderKind,
};
use serde_json::{json, Value};
use std::path::Path;

#[test]
fn image_paste_parts_preserve_text_and_only_extract_exact_image_lines() {
    use super::hooks::image_paste_parts;
    let text =
        "worker:\nreply with bus send --reply-to 17\n\nbody \t\n\"/tmp/a.PNG\"\n\"/tmp/b.webp\"";
    assert_eq!(
        image_paste_parts(text),
        (
            vec!["\"/tmp/a.PNG\"", "\"/tmp/b.webp\""],
            "worker:\nreply with bus send --reply-to 17\n\nbody \t".to_owned()
        )
    );
    for text in [
        "",
        "body\n\n",
        "\"/tmp/a.diff\"",
        "\"/tmp/a.png\" \"/tmp/b.png\"",
        "body \"/tmp/a.png\"",
    ] {
        assert_eq!(image_paste_parts(text), (Vec::new(), text.to_owned()));
    }
    assert_eq!(
        image_paste_parts("\"/tmp/a.png\""),
        (vec!["\"/tmp/a.png\""], String::new())
    );
    assert_eq!(
        image_paste_parts("body\r\n\r\n\"/tmp/a.png\"\r\nlast\r\n"),
        (vec!["\"/tmp/a.png\""], "body\r\n\r\nlast\r\n".to_owned())
    );
}

fn hook(event: &str) -> Value {
    json!({"hook_event_name":event,"session_id":"s","turn_id":"t","transcript_path":"/tmp/interactive.jsonl","prompt":"prompt","last_assistant_message":"reply"})
}

#[test]
fn codex_empty_stop_keeps_the_turn_with_null_missing_or_empty_text() {
    for text in [None, Some(Value::Null), Some(json!(""))] {
        let mut value = hook("Stop");
        value
            .as_object_mut()
            .unwrap()
            .remove("last_assistant_message");
        if let Some(text) = text {
            value["last_assistant_message"] = text;
        }
        assert_eq!(
            parse(&value).unwrap(),
            Parsed::Final {
                session: "s".into(),
                turn: "t".into(),
                text: String::new(),
            }
        );
    }
    for text in [json!(false), json!(7), json!([]), json!({})] {
        let mut value = hook("Stop");
        value["last_assistant_message"] = text;
        assert!(parse(&value).is_err());
    }
}

#[test]
fn codex_hook_filter_runs_before_background_session_and_turn_decoding() {
    for event in ["SessionStart", "UserPromptSubmit", "Stop"] {
        for transcript in [Value::Null, json!(""), json!(false), json!(7)] {
            let value = json!({"hook_event_name":event,"transcript_path":transcript});
            assert_eq!(parse(&value).unwrap(), Parsed::Ignore);
        }
        assert_eq!(
            parse(&json!({"hook_event_name":event})).unwrap(),
            Parsed::Ignore
        );
    }
    assert_eq!(
        parse(&json!({"type":"agent-turn-complete","turn-id":"old"})).unwrap_err(),
        "Hook missing hook_event_name; update the CLI and verify Bus hooks. No prompt will be retried."
    );
}

#[test]
fn codex_interactive_hooks_keep_opaque_binding_and_verbatim_prompt() {
    let mut started = hook("UserPromptSubmit");
    started["prompt"] = json!("<pasted_content id=\"a\">\nbody\n</pasted_content id=\"a\">");
    assert_eq!(
        parse(&started).unwrap(),
        Parsed::Started {
            session: "s".into(),
            turn: "t".into(),
            prompt: started["prompt"].as_str().unwrap().into()
        }
    );
    let mut session = hook("SessionStart");
    session["source"] = json!("compact");
    assert_eq!(
        parse(&session).unwrap(),
        Parsed::Session {
            session: "s".into(),
            source: Some("compact".into())
        }
    );
    assert_eq!(
        parse(&hook("Stop")).unwrap(),
        Parsed::Final {
            session: "s".into(),
            turn: "t".into(),
            text: "reply".into()
        }
    );
    assert_eq!(
        parse(&hook("StopFailure")).unwrap(),
        Parsed::Failure {
            session: "s".into(),
            turn: "t".into(),
            message: "Provider turn failed; inspect its terminal.".into(),
            error: None,
        }
    );
    let mut malformed = hook("Stop");
    malformed.as_object_mut().unwrap().remove("turn_id");
    assert!(parse(&malformed)
        .unwrap_err()
        .starts_with("Hook missing turn_id;"));
    assert_eq!(
        parse(&hook("notify")).unwrap_err(),
        "Unsupported Bus hook event notify"
    );
}

#[test]
fn codex_subagent_hooks_never_become_terminal_observations() {
    let mut value = hook("Stop");
    value["agent_id"] = json!("subagent");
    assert_eq!(parse(&value).unwrap(), Parsed::Ignore);
    assert_eq!(parse(&hook("SubagentStop")).unwrap(), Parsed::Ignore);
    value["agent_id"] = Value::Null;
    assert!(matches!(parse(&value).unwrap(), Parsed::Final { .. }));
}

#[test]
fn codex_hook_merge_keeps_notify_user_entries_and_quoted_live_command() {
    let contract = HookContract::for_provider(ProviderKind::Codex);
    assert_eq!(
        contract.events,
        ["SessionStart", "UserPromptSubmit", "Stop"]
    );
    let binary = Path::new("/tmp/a path/it's bus");
    let original = json!({"notify":["user-notify"],"hooks":{"Stop":[{"hooks":[{"type":"command","command":"user-hook"}]}]}});
    let merged = merge_hooks(original.clone(), contract, binary).unwrap();
    assert_eq!(merged["notify"], original["notify"]);
    assert_eq!(merged["hooks"]["Stop"][0], original["hooks"]["Stop"][0]);
    assert_eq!(
        merged["hooks"]["Stop"][1],
        contract.entry(&contract.command(binary))
    );
    assert!(contract
        .command(binary)
        .ends_with(" --bus-callback codex-hook"));
    assert_eq!(
        merge_hooks(merged.clone(), contract, binary).unwrap(),
        merged
    );
    assert!(merge_hooks(json!({"hooks":{"Stop":"invalid"}}), contract, binary).is_err());
}
