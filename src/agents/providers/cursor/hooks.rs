//! Cursor hook facts; completion and settled reply resolution remain separate.
use super::super::spool::{field, Parsed};
use serde_json::Value;

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hook_event_name")?;
    if value.get("agent_id").is_some_and(|v| !v.is_null()) || event.starts_with("Subagent") {
        return Ok(Parsed::Ignore);
    }
    let session = field(value, "conversation_id")?;
    if event == "sessionStart" {
        return Ok(Parsed::Session {
            session,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    let turn = field(value, "generation_id")?;
    match event.as_str() {
        "beforeSubmitPrompt" => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "afterAgentResponse" => Ok(Parsed::Response {
            session,
            turn,
            text: field(value, "text")?,
        }),
        "stop" => match field(value, "status")?.as_str() {
            "completed" => Ok(Parsed::Completed { session, turn }),
            status => Ok(Parsed::Failure {
                session,
                turn,
                message: format!("Cursor turn {status}; inspect its terminal."),
            }),
        },
        _ => Err(format!("Unsupported Bus hook event {event}")),
    }
}
