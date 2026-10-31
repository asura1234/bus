//! Facts from Kimi's beta hooks. Stop never carries reply text or a turn ID.
use crate::agents::providers::spool::{field, Parsed};
use serde_json::Value;

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hook_event_name")?;
    if !matches!(
        event.as_str(),
        "SessionStart" | "TurnStarted" | "Stop" | "StopFailure" | "Interrupt"
    ) {
        return Ok(Parsed::Ignore);
    }
    let session = field(value, "session_id")?;
    match event.as_str() {
        "SessionStart" => Ok(Parsed::Session {
            session,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "TurnStarted" if value.get("origin_kind").and_then(Value::as_str) == Some("user") => {
            let turn = value
                .get("turn_id")
                .and_then(Value::as_u64)
                .ok_or("Kimi TurnStarted lacks a numeric turn_id")?
                .to_string();
            Ok(Parsed::Started {
                session,
                turn,
                prompt: field(value, "prompt")?,
            })
        }
        "Stop" | "StopFailure" | "Interrupt" => Ok(Parsed::ReconcileSession { session }),
        _ => Ok(Parsed::Ignore),
    }
}
