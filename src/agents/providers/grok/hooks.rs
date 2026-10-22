//! Native Grok observations. A Stop gate is never a publishable final.
use crate::agents::providers::{
    spool::{field, Parsed},
    ProviderKind,
};
use serde_json::Value;

/// Grok imports Claude/Cursor hook commands but sends its own camelCase payload.
/// Filter that traffic before another adapter can spool a malformed observation;
/// actual native callbacks on those adapters keep their existing behavior.
pub(crate) fn is_foreign_callback(adapter: ProviderKind, value: &Value) -> bool {
    match adapter {
        ProviderKind::Grok => value.get("hook_event_name").is_some(),
        _ => value.get("hookEventName").is_some(),
    }
}

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    if is_foreign_callback(ProviderKind::Grok, value) {
        return Ok(Parsed::Ignore);
    }
    let event = field(value, "hookEventName")?;
    if event == "notification"
        && value.get("notificationType").and_then(Value::as_str) == Some("idle_prompt")
        && value.get("subagentType").is_none_or(Value::is_null)
    {
        // Session-level idle requests reconciliation, never finality by itself.
        return Ok(Parsed::ReconcileSession {
            session: identity(value, "sessionId")?,
        });
    }
    if value.get("subagentType").is_some_and(|v| !v.is_null())
        || event.starts_with("subagent_")
        || matches!(event.as_str(), "session_end" | "notification")
    {
        return Ok(Parsed::Ignore);
    }
    if event == "stop"
        && matches!(
            value.get("reason").and_then(Value::as_str),
            Some("shutdown" | "channel_closed")
        )
    {
        return Ok(Parsed::Ignore);
    }
    let session = identity(value, "sessionId")?;
    if event == "session_start" {
        return Ok(Parsed::Session {
            session,
            source: value
                .get("source")
                .and_then(Value::as_str)
                .map(str::to_owned),
        });
    }
    let turn = identity(value, "promptId")?;
    match event.as_str() {
        "user_prompt_submit" => Ok(Parsed::Started {
            session,
            turn,
            prompt: field(value, "prompt")?,
        }),
        "stop" if value.get("reason").and_then(Value::as_str) == Some("end_turn") => {
            Ok(Parsed::CompletionCandidate { session, turn })
        }
        "stop" => Err("Grok Stop has no supported turn-end reason; no reply was published.".into()),
        "stop_failure" => Ok(Parsed::Failure {
            session,
            turn,
            message: "Grok turn failed; inspect its terminal.".into(),
            error: Some(failure_reason(value)),
        }),
        "stop_cancelled" => Ok(Parsed::Failure {
            session,
            turn,
            message: "Grok turn cancelled; no successful reply was published.".into(),
            error: Some(cancellation_reason(value)),
        }),
        _ => Err(format!("Unsupported Bus Grok hook event {event}")),
    }
}

fn identity(value: &Value, key: &str) -> Result<String, String> {
    let id = field(value, key)?;
    if id.trim().is_empty() {
        return Err(format!(
            "Grok hook has an empty {key}; no prompt will be retried."
        ));
    }
    Ok(id)
}

fn cancellation_reason(value: &Value) -> String {
    detail(value, "reason").unwrap_or_else(|| "unknown".into())
}

fn failure_reason(value: &Value) -> String {
    let category = detail(value, "error").unwrap_or_else(|| "unknown".into());
    // lastAssistantMessage on StopFailure is rendered error text, never a reply.
    let message = detail(value, "errorDetails").or_else(|| detail(value, "lastAssistantMessage"));
    match message {
        Some(message) => format!("{message} ({category})"),
        None => category,
    }
}

fn detail(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|text| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(300)
                .collect::<String>()
        })
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
#[path = "tests/hooks_test.rs"]
mod tests;
