//! Grok Stop is a gate attempt, not proof that a turn committed its final.
//!
//! The binary-matched 1.0.5 guides document hook envelopes and ACP chunks, but
//! not the persisted turn-final schema. Unknown evidence deliberately remains
//! pending. Neither hook_execution success nor an assistant chunk proves finality.
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

/// Trusted Bus evidence, never deserialized from native hook/transcript JSON.
/// The producer must prove a post-gate committed terminal outcome and complete
/// text for the current launch/request revision, invalidating success on newer
/// submits, cancellation, failure, session replacement or ambiguous ownership.
/// No native producer is approved yet; production candidates supply None.
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
    for line in transcript.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(row) = serde_json::from_str::<Value>(line) else {
            return Ok(CompletionResolution::Pending);
        };
        if row.get("method").and_then(Value::as_str) == Some("_x.ai/session/update") {
            let Some(row_session) = row.pointer("/params/sessionId").and_then(Value::as_str) else {
                return Ok(CompletionResolution::Pending);
            };
            if row_session != session {
                return Err(
                    "Grok transcript session differs from its callback; no reply was published."
                        .into(),
                );
            }
        }
    }
    // Idle is necessary, but a Stop can precede continuation/cancel/failure,
    // and a successful hook_execution can describe the earlier gate attempt.
    // Keep this scoped candidate in the spool until committed completion and
    // final-text ownership can be established from a verified native schema.
    resolve_attested_completion(&session, &turn, settled_idle, None)
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
