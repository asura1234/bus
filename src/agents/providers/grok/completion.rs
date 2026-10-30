//! Grok Stop is a gate attempt. Build 1.0.70's owned transcript commits a
//! reply with a prompt-scoped `turn_completed/end_turn` after its final text.
use super::hooks;
use crate::agents::providers::spool::Parsed;
use serde_json::Value;
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const MAX_TRANSCRIPT_TAIL: u64 = 2 * 1024 * 1024;

#[derive(Debug, PartialEq)]
pub(crate) enum CompletionResolution {
    Pending,
    Final {
        session: String,
        turn: String,
        text: String,
    },
    Cancelled {
        session: String,
        turn: String,
        reason: String,
    },
}

/// Trusted Bus evidence for the isolated synthetic driver, never deserialized
/// from native JSON. Native Build 1.0.70 uses the owned transcript reader below.
pub(crate) struct CompletionAttestation {
    pub(crate) session: String,
    pub(crate) turn: String,
    pub(crate) text: String,
}

pub(crate) fn resolve_attested_completion(
    expected_session: &str,
    expected_turn: &str,
    settled_idle: bool,
    attestation: Option<&CompletionAttestation>,
) -> Result<CompletionResolution, String> {
    if expected_session.trim().is_empty() || expected_turn.trim().is_empty() {
        return Err(
            "Grok completion requires nonempty expected session and prompt identities.".into(),
        );
    }
    let Some(attestation) = attestation else {
        return Ok(CompletionResolution::Pending);
    };
    if attestation.session.trim().is_empty()
        || attestation.turn.trim().is_empty()
        || attestation.session != expected_session
        || attestation.turn != expected_turn
    {
        return Err(
            "Grok completion attestation differs from its expected session or prompt.".into(),
        );
    }
    if !settled_idle {
        return Ok(CompletionResolution::Pending);
    }
    Ok(CompletionResolution::Final {
        session: attestation.session.clone(),
        turn: attestation.turn.clone(),
        text: attestation.text.clone(),
    })
}

pub(crate) fn resolve_candidate(
    value: &Value,
    settled_idle: bool,
) -> Result<CompletionResolution, String> {
    let parsed = hooks::parse(value)?;
    let (session, turn) = match parsed {
        Parsed::Ignore | Parsed::ReconcileSession { .. } => {
            return Ok(CompletionResolution::Pending)
        }
        Parsed::CompletionCandidate { session, turn } => (session, turn),
        Parsed::Failure {
            session,
            turn,
            error,
            ..
        } => {
            return Ok(CompletionResolution::Cancelled {
                session,
                turn,
                reason: error.unwrap_or_else(|| "unknown".into()),
            });
        }
        _ => return Err("Grok completion resolver requires a turn-end observation.".into()),
    };
    let Some(path) = value.get("transcriptPath") else {
        return Ok(CompletionResolution::Pending);
    };
    let path = path
        .as_str()
        .filter(|path| !path.is_empty())
        .ok_or("Grok transcriptPath is not a nonempty string.")?;
    let Some(transcript) = read_transcript(Path::new(path), &session)? else {
        return Ok(CompletionResolution::Pending);
    };
    let mut final_text = None;
    let mut completed = false;
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            return Ok(CompletionResolution::Pending);
        };
        let method = row.get("method").and_then(Value::as_str);
        if !matches!(method, Some("session/update" | "_x.ai/session/update")) {
            continue;
        }
        let Some(row_session) = row.pointer("/params/sessionId").and_then(Value::as_str) else {
            return Ok(CompletionResolution::Pending);
        };
        if row_session != session {
            return Err(
                "Grok transcript session differs from its callback; no reply was published.".into(),
            );
        }
        match row
            .pointer("/params/update/sessionUpdate")
            .and_then(Value::as_str)
        {
            Some("agent_message_chunk")
                if row
                    .pointer("/params/_meta/promptId")
                    .and_then(Value::as_str)
                    == Some(turn.as_str()) =>
            {
                if completed {
                    return Ok(CompletionResolution::Pending);
                }
                if row
                    .pointer("/params/update/content/type")
                    .and_then(Value::as_str)
                    != Some("text")
                {
                    return Ok(CompletionResolution::Pending);
                }
                let Some(text) = row
                    .pointer("/params/update/content/text")
                    .and_then(Value::as_str)
                else {
                    return Ok(CompletionResolution::Pending);
                };
                // A tool turn can emit an assistant preamble before its final
                // answer. Build 1.0.70 writes each whole message as a chunk.
                final_text = Some(text.to_owned());
            }
            Some("turn_completed")
                if row
                    .pointer("/params/update/prompt_id")
                    .and_then(Value::as_str)
                    == Some(turn.as_str()) =>
            {
                if completed {
                    return Ok(CompletionResolution::Pending);
                }
                if row
                    .pointer("/params/update/stop_reason")
                    .and_then(Value::as_str)
                    != Some("end_turn")
                {
                    return Ok(CompletionResolution::Pending);
                }
                completed = true;
            }
            _ => {}
        }
    }
    if !completed || !settled_idle {
        return Ok(CompletionResolution::Pending);
    }
    let Some(text) = final_text else {
        return Ok(CompletionResolution::Pending);
    };
    Ok(CompletionResolution::Final {
        session,
        turn,
        text,
    })
}

/// Read only a bounded, complete JSONL suffix from the callback's session.
/// Missing/lagging/torn writes are pending; wrong ownership fails explicitly.
fn read_transcript(path: &Path, session: &str) -> Result<Option<String>, String> {
    if !path.is_absolute()
        || path.file_name().and_then(|v| v.to_str()) != Some("updates.jsonl")
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|v| v.to_str())
            != Some(session)
    {
        return Err("Grok transcript path does not identify its callback session.".into());
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot inspect Grok transcript.".into()),
    };
    if !metadata.file_type().is_file() {
        return Err("Grok transcript must be an owned regular file.".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "Cannot resolve Grok transcript.")?;
    if canonical
        .parent()
        .and_then(Path::file_name)
        .and_then(|v| v.to_str())
        != Some(session)
    {
        return Err("Grok transcript resolves outside its callback session.".into());
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot open Grok transcript.".into()),
    };
    let length = file
        .metadata()
        .map_err(|_| "Cannot inspect open Grok transcript.")?
        .len();
    let offset = length.saturating_sub(MAX_TRANSCRIPT_TAIL);
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| "Cannot seek Grok transcript.")?;
    let mut bytes = Vec::new();
    file.take(MAX_TRANSCRIPT_TAIL)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read Grok transcript.")?;
    if bytes.last().is_some_and(|byte| *byte != b'\n') {
        return Ok(None);
    }
    let start = if offset == 0 {
        0
    } else {
        match bytes.iter().position(|byte| *byte == b'\n') {
            Some(index) => index + 1,
            None => return Ok(None),
        }
    };
    Ok(std::str::from_utf8(&bytes[start..]).ok().map(str::to_owned))
}

#[cfg(test)]
#[path = "tests/completion_test.rs"]
mod tests;
