//! Observation-hook parser for Google Antigravity CLI (`agy-hook`).
use crate::agents::providers::spool::{field, Parsed};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StopVerdict {
    Completed,
    BackgroundPending,
    Cancelled,
    Failed { message: String, error: String },
}

pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hookEventName")?;
    if matches!(
        event.as_str(),
        "PostInvocation" | "PreToolUse" | "PostToolUse" | "Notification"
    ) {
        return Ok(Parsed::Ignore);
    }
    let session = identity(value, "conversationId")?;
    match event.as_str() {
        "SessionStart" => Ok(Parsed::Session {
            session,
            source: Some("startup".into()),
        }),
        "PreInvocation" => {
            let invocation = value
                .get("invocationNum")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if invocation > 0 {
                return Ok(Parsed::Ignore);
            }
            let initial_steps = value
                .get("initialNumSteps")
                .and_then(Value::as_u64)
                .filter(|&steps| steps >= 1)
                .ok_or("AGY PreInvocation lacks positive initialNumSteps")?;
            let turn = (initial_steps - 1).to_string();
            let prompt = field(value, "lastUserInput")?;
            Ok(Parsed::Started {
                session,
                turn,
                prompt,
            })
        }
        "Stop" => {
            let _ = stop_verdict(value)?;
            Ok(Parsed::ReconcileSession { session })
        }
        _ => Err(format!("Unsupported Bus AGY hook event {event}")),
    }
}

pub(crate) fn stop_verdict(value: &Value) -> Result<StopVerdict, String> {
    let reason = value
        .get("terminationReason")
        .and_then(Value::as_str)
        .unwrap_or("NO_TOOL_CALL");
    let fully_idle = value
        .get("fullyIdle")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let detail = value
        .get("error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());

    match reason {
        "NO_TOOL_CALL" | "TERMINAL_STEP_TYPE" | "TERMINAL_CUSTOM_HOOK" | "INJECTED_RESPONSE" => {
            if fully_idle {
                Ok(StopVerdict::Completed)
            } else {
                Ok(StopVerdict::BackgroundPending)
            }
        }
        "USER_CANCELED" => Ok(StopVerdict::Cancelled),
        "ERROR"
        | "MAX_INVOCATIONS"
        | "MAX_FORCED_INVOCATIONS"
        | "MAX_TOKEN_BUDGET_EXCEEDED" => Ok(StopVerdict::Failed {
            message: "AGY turn failed; inspect its terminal.".into(),
            error: detail.unwrap_or(reason).to_owned(),
        }),
        other => Err(format!("Unsupported AGY Stop terminationReason {other}")),
    }
}

fn identity(value: &Value, key: &str) -> Result<String, String> {
    let id = field(value, key)?;
    if id.trim().is_empty() {
        return Err(format!(
            "AGY hook has an empty {key}; no prompt will be retried."
        ));
    }
    Ok(id)
}
