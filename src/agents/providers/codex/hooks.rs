//! Codex hook decoding; only persisted interactive sessions emit observations.
use super::super::spool::{field, Parsed};
use serde_json::Value;

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hook_event_name")?;
    // Title/memory sessions inherit hooks but have no interactive transcript.
    // Keep this before decoding their otherwise irrelevant session/turn fields.
    if value
        .get("transcript_path")
        .and_then(Value::as_str)
        .is_none_or(|path| path.is_empty())
    {
        return Ok(Parsed::Ignore);
    }
    if value.get("agent_id").is_some_and(|v| !v.is_null()) || event.starts_with("Subagent") {
        return Ok(Parsed::Ignore);
    }
    let session = field(value, "session_id")?;
    if event == "SessionStart" {
        return Ok(Parsed::Session {
            session,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    let turn = field(value, "turn_id")?;
    match event.as_str() {
        "UserPromptSubmit" => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "Stop" => Ok(Parsed::Final {
            session,
            turn,
            text: field(value, "last_assistant_message")?,
        }),
        "StopFailure" => Ok(Parsed::Failure {
            session,
            turn,
            message: "Provider turn failed; inspect its terminal.".into(),
        }),
        _ => Err(format!("Unsupported Bus hook event {event}")),
    }
}
