//! Observation-hook parser for Google Antigravity CLI (`agy-hook`).
use super::completion;
use crate::agents::providers::spool::{field, Parsed};
use serde_json::Value;
use std::{
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

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
        "SessionStart" => {
            if is_subagent_transcript(value, &session)
                || brain_root(value, &session)
                    .is_some_and(|brain| is_registered_subagent_any_parent(&brain, &session))
            {
                return Ok(Parsed::Ignore);
            }
            Ok(Parsed::Session {
                session,
                source: Some("startup".into()),
            })
        }
        "PreInvocation" => {
            let invocation = value
                .get("invocationNum")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if invocation > 0 {
                return Ok(Parsed::Ignore);
            }
            let prompt = field(value, "lastUserInput")?;
            if prompt.is_empty() || is_subagent_transcript(value, &session) {
                return Ok(Parsed::Ignore);
            }
            let initial_steps = value
                .get("initialNumSteps")
                .and_then(Value::as_u64)
                .filter(|&steps| steps >= 1)
                .ok_or("AGY PreInvocation lacks positive initialNumSteps")?;
            let target_step = initial_steps - 1;
            if is_non_user_wakeup_step(value, &session, target_step) {
                return Ok(Parsed::Ignore);
            }
            Ok(Parsed::Started {
                session,
                turn: target_step.to_string(),
                prompt,
            })
        }
        "Stop" => {
            if value.get("lastUserInput").and_then(Value::as_str) == Some("")
                || is_subagent_transcript(value, &session)
            {
                return Ok(Parsed::Ignore);
            }
            let _ = stop_verdict(value)?;
            Ok(Parsed::ReconcileSession { session })
        }
        _ => Err(format!("Unsupported Bus AGY hook event {event}")),
    }
}

/// Check whether `candidate` is recorded under `root_session`'s `.system_generated/subagents/`
/// tree (including nested subagents).
pub(crate) fn is_descendant_subagent(root_session: &str, candidate: &str, value: &Value) -> bool {
    if root_session == candidate {
        return false;
    }
    if is_subagent_transcript(value, candidate) {
        return true;
    }
    let Some(brain) = brain_root(value, candidate) else {
        return false;
    };
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::from([root_session.to_owned()]);
    while let Some(current) = queue.pop_front() {
        if visited.len() >= 32 || !visited.insert(current.clone()) {
            continue;
        }
        let subagents_dir = brain.join(&current).join(".system_generated/subagents");
        if subagents_dir.join(format!("{candidate}.json")).is_file() {
            return true;
        }
        let Ok(entries) = std::fs::read_dir(&subagents_dir) else {
            continue;
        };
        for entry in entries.flatten().take(32) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    queue.push_back(stem.to_owned());
                }
            }
        }
    }
    false
}

fn brain_root(value: &Value, session: &str) -> Option<PathBuf> {
    if let Some(artifact) = value.get("artifactDirectoryPath").and_then(Value::as_str) {
        let path = Path::new(artifact);
        if path.is_absolute() && path.file_name().and_then(|n| n.to_str()) == Some(session) {
            if let Some(parent) = path.parent() {
                return Some(parent.to_path_buf());
            }
        }
    }
    let transcript = value.get("transcriptPath").and_then(Value::as_str)?;
    let path = Path::new(transcript);
    let session_dir = path.parent()?.parent()?.parent()?;
    if session_dir.is_absolute()
        && session_dir.file_name().and_then(|n| n.to_str()) == Some(session)
    {
        session_dir.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

fn is_registered_subagent_any_parent(brain: &Path, candidate: &str) -> bool {
    let target_name = format!("{candidate}.json");
    let Ok(entries) = std::fs::read_dir(brain) else {
        return false;
    };
    for entry in entries.flatten().take(512) {
        if entry
            .path()
            .join(".system_generated/subagents")
            .join(&target_name)
            .is_file()
        {
            return true;
        }
    }
    false
}

fn is_subagent_transcript(value: &Value, session: &str) -> bool {
    let Some(raw) = value.get("transcriptPath").and_then(Value::as_str) else {
        return false;
    };
    let Ok(path) = completion::resolve_transcript_path(raw, session) else {
        return false;
    };
    completion::first_step_source(&path).as_deref() == Some("SYSTEM")
}

fn is_non_user_wakeup_step(value: &Value, session: &str, target_step: u64) -> bool {
    let Some(raw) = value.get("transcriptPath").and_then(Value::as_str) else {
        return false;
    };
    let Ok(path) = completion::resolve_transcript_path(raw, session) else {
        return false;
    };
    completion::step_source_at(&path, target_step).is_some_and(|source| source != "USER_EXPLICIT")
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
        "ERROR" | "MAX_INVOCATIONS" | "MAX_FORCED_INVOCATIONS" | "MAX_TOKEN_BUDGET_EXCEEDED" => {
            Ok(StopVerdict::Failed {
                message: "AGY turn failed; inspect its terminal.".into(),
                error: detail.unwrap_or(reason).to_owned(),
            })
        }
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
