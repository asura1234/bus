//! Deterministic completion reader over AGY's `transcript_full.jsonl`.
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TurnResolution {
    Pending,
    Final {
        turn_id: String,
        text: String,
        prompts: Vec<String>,
    },
}

/// Prefer `transcript_full.jsonl` when `transcriptPath` points to compact `transcript.jsonl`.
pub(crate) fn resolve_transcript_path(raw: &str, session: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(raw);
    if !candidate.is_absolute() {
        return Err("AGY transcriptPath must be an absolute path".into());
    }
    if !candidate
        .components()
        .any(|component| component.as_os_str().to_str() == Some(session))
    {
        return Err("AGY transcriptPath does not match callback conversationId".into());
    }
    if candidate.file_name().and_then(|n| n.to_str()) == Some("transcript.jsonl") {
        let full = candidate.with_file_name("transcript_full.jsonl");
        if full.is_file() {
            return Ok(full);
        }
    }
    Ok(candidate.to_path_buf())
}

/// Strip AGY's `<USER_REQUEST>\n...\n</USER_REQUEST>` envelope around raw prompt text.
pub(crate) fn unwrap_user_request(content: &str) -> String {
    if let Some(after_open) = content.strip_prefix("<USER_REQUEST>") {
        let after_open = after_open
            .strip_prefix("\r\n")
            .or_else(|| after_open.strip_prefix('\n'))
            .unwrap_or(after_open);
        if let Some((body, _tail)) = after_open.split_once("</USER_REQUEST>") {
            let body = body
                .strip_suffix("\r\n")
                .or_else(|| body.strip_suffix('\n'))
                .unwrap_or(body);
            return body.to_owned();
        }
    }
    content.to_owned()
}

pub(crate) fn read_turn_completion(
    transcript_path: &Path,
    target_turn: &str,
) -> Result<TurnResolution, String> {
    let target_step: u64 = target_turn
        .parse()
        .map_err(|_| format!("Invalid AGY numeric turn id {target_turn:?}"))?;
    let metadata = match std::fs::metadata(transcript_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TurnResolution::Pending)
        }
        Err(error) => return Err(format!("Cannot stat AGY transcript: {error}")),
    };
    if !metadata.is_file() {
        return Err("AGY transcript path is not a regular file".into());
    }
    if metadata.len() > MAX_TRANSCRIPT_BYTES {
        return Err("AGY transcript exceeds 64 MiB safety limit".into());
    }
    let file =
        File::open(transcript_path).map_err(|e| format!("Cannot open AGY transcript: {e}"))?;
    let reader = BufReader::new(file);
    scan_lines(reader.lines(), target_step, target_turn)
}

pub(crate) fn scan_lines<I, E>(
    lines: I,
    target_step: u64,
    target_turn: &str,
) -> Result<TurnResolution, String>
where
    I: IntoIterator<Item = Result<String, E>>,
    E: std::fmt::Display,
{
    let mut in_turn = false;
    let mut prompts: Vec<String> = Vec::new();
    let mut latest_terminal_reply: Option<String> = None;
    let mut terminal_response_seen = false;

    for line in lines {
        let line = line.map_err(|e| format!("Cannot read AGY transcript line: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        // A torn trailing JSONL line while AGY flushes stays Pending for retry.
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            return Ok(TurnResolution::Pending);
        };
        let Some(step_index) = row.get("step_index").and_then(Value::as_u64) else {
            continue;
        };
        let source = row.get("source").and_then(Value::as_str).unwrap_or_default();
        let kind = row.get("type").and_then(Value::as_str).unwrap_or_default();
        let status = row.get("status").and_then(Value::as_str).unwrap_or_default();

        if source == "USER_EXPLICIT" && kind == "USER_INPUT" {
            if step_index == target_step {
                in_turn = true;
                let raw = row.get("content").and_then(Value::as_str).unwrap_or_default();
                prompts.push(unwrap_user_request(raw));
                continue;
            }
            if in_turn && step_index > target_step {
                if terminal_response_seen {
                    // A new outer user turn started after a terminal response.
                    break;
                }
                // Mid-turn steered input (`queuedMessages: "send-immediately"`).
                let raw = row.get("content").and_then(Value::as_str).unwrap_or_default();
                prompts.push(unwrap_user_request(raw));
            }
            continue;
        }

        if !in_turn {
            continue;
        }

        if source == "MODEL" && kind == "PLANNER_RESPONSE" {
            if status != "DONE" {
                return Ok(TurnResolution::Pending);
            }
            let has_tool_calls = row
                .get("tool_calls")
                .and_then(Value::as_array)
                .is_some_and(|calls| !calls.is_empty());
            if has_tool_calls {
                terminal_response_seen = false;
            } else {
                let content = row
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                latest_terminal_reply = Some(content);
                terminal_response_seen = true;
            }
        } else if terminal_response_seen {
            // Non-explicit continuation (e.g. background task / subagent wake).
            terminal_response_seen = false;
        }
    }

    match latest_terminal_reply {
        Some(text) if in_turn && terminal_response_seen => Ok(TurnResolution::Final {
            turn_id: target_turn.to_owned(),
            text,
            prompts,
        }),
        _ => Ok(TurnResolution::Pending),
    }
}
