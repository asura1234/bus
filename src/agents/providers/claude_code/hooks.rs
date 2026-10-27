//! Claude observation hooks and provider-input normalization.

use super::statusline;
use crate::agents::providers::{
    hook_json::{self, HookContext, HookContract},
    spool::{field, optional_text, Parsed},
    ProviderKind,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Install capture only in the launch's settings file. Claude needs no project
/// hook consent; the user's settings and project hook files are left intact.
pub(crate) fn install(context: &HookContext, cwd: &Path) -> Result<PathBuf, String> {
    let settings = context.spool.join("claude-settings.json");
    hook_json::install_hooks(
        &settings,
        HookContract::for_provider(ProviderKind::ClaudeCode),
        &context.binary,
    )?;
    statusline::install(&context.spool, &project_root(cwd), &context.binary)
        .map_err(|e| e.to_string())?;
    Ok(settings)
}

fn project_root(cwd: &Path) -> PathBuf {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        }
        _ => cwd.to_path_buf(),
    }
}

/// Decode facts only; session ownership and reply settlement remain callers'
/// decisions. Required fields retain the frozen parser's error ordering.
pub(crate) fn parse(value: &Value) -> Result<Parsed, String> {
    let event = field(value, "hook_event_name")?;
    if value.get("agent_id").is_some_and(|v| !v.is_null()) {
        return Ok(Parsed::Ignore);
    }
    if event.starts_with("Subagent") {
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
    let turn = field(value, "prompt_id")?;
    match event.as_str() {
        "UserPromptSubmit" => {
            let prompt = field(value, "prompt")?;
            Ok(Parsed::Started {
                session,
                turn,
                prompt: unwrap_claude_paste(prompt),
            })
        }
        "Stop"
            if optional_text(value, "last_assistant_message")?
                .trim()
                .is_empty()
                && claude_turn_awaits_background(value) =>
        {
            Ok(Parsed::BackgroundPending { session, turn })
        }
        "Stop" => Ok(Parsed::Final {
            session,
            turn,
            text: optional_text(value, "last_assistant_message")?,
        }),
        "StopFailure" => Ok(Parsed::Failure {
            session,
            turn,
            message: "Provider turn failed; inspect its terminal.".into(),
            error: Some(claude_api_error(value)),
        }),
        _ => Err(format!("Unsupported Bus hook event {event}")),
    }
}

/// Longest provider error text Bus keeps for a reason and the agent's error.
const API_ERROR_MAX_CHARS: usize = 300;

/// Claude Code fires StopFailure when an API error ends a turn. Measured on
/// 2.1.296: `last_assistant_message` is the "API Error: ..." text it shows
/// (and records as a synthetic transcript message), `error` its category,
/// e.g. `server_error`. Kept to one bounded line for statuses and logs.
fn claude_api_error(value: &Value) -> String {
    let text = |key| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty())
    };
    let (message, category) = (text("last_assistant_message"), text("error"));
    let message = match message {
        Some(message) if message.chars().count() > API_ERROR_MAX_CHARS => {
            let cut: String = message.chars().take(API_ERROR_MAX_CHARS).collect();
            format!("{}…", cut.trim_end())
        }
        Some(message) => message,
        None => return category.unwrap_or_else(|| "unknown".into()),
    };
    match category {
        Some(category) => format!("{message} ({category})"),
        None => message,
    }
}

/// Unwrap only one complete terminal paste frame, preserving leading image
/// placeholders and every byte of the submitted text. Other input is literal.
fn unwrap_claude_paste(prompt: String) -> String {
    let placeholders = claude_image_placeholders(&prompt).0;
    let (images, body) = prompt.split_at(placeholders);
    let unwrapped = (|| {
        let framed = body.trim_matches(|c: char| c.is_ascii_whitespace());
        let rest = framed.strip_prefix("<pasted_content id=\"")?;
        let (id, rest) = rest.split_once("\">\n")?;
        if id.is_empty() || id.contains(['"', '<', '>', '\n']) {
            return None;
        }
        rest.strip_suffix(&format!("\n</pasted_content id=\"{id}\">"))
            .map(str::to_owned)
    })();
    match unwrapped {
        Some(text) => format!("{images}{text}"),
        None => prompt,
    }
}

/// A complete leading paste can coexist with text typed beside its atomic
/// composer element. Return that paste as a correlation candidate; callers
/// must still match its entire body to their owned submission, never a substring.
pub(crate) fn claude_paste_candidate(prompt: &str) -> Option<String> {
    let placeholders = claude_image_placeholders(prompt).0;
    let (images, body) = prompt.split_at(placeholders);
    let framed = body.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let rest = framed.strip_prefix("<pasted_content id=\"")?;
    let (id, rest) = rest.split_once("\">\n")?;
    if id.is_empty() || id.contains(['"', '<', '>', '\n']) {
        return None;
    }
    let (text, suffix) = rest.split_once(&format!("\n</pasted_content id=\"{id}\">"))?;
    if text.contains("<pasted_content")
        || text.contains("</pasted_content")
        || suffix.contains("<pasted_content")
        || suffix.contains("</pasted_content")
    {
        return None;
    }
    Some(format!("{images}{text}"))
}

/// Byte length and count of consecutive leading `[Image #N]` placeholders.
/// Messaging may use this provider fact without copying its normalization.
pub(crate) fn claude_image_placeholders(prompt: &str) -> (usize, usize) {
    let (mut length, mut count) = (0, 0);
    while let Some(digits) = prompt[length..].strip_prefix("[Image #") {
        let Some(end) = digits.find(']') else { break };
        if end == 0 || !digits[..end].bytes().all(|b| b.is_ascii_digit()) {
            break;
        }
        length += "[Image #".len() + end + 1;
        count += 1;
    }
    (length, count)
}

/// Claude Code turns each typed line that is one quoted image path into an
/// image attachment. Its composer and its submit hook then show one
/// `[Image #N]` placeholder per image first (N counts the session's images),
/// followed by the remaining lines with blank ones dropped. Returns how many
/// lines of `typed` it lifts and the text left behind.
pub(crate) fn claude_lifted_images(typed: &str) -> (usize, String) {
    let mut lifted = 0;
    let remaining = typed
        .split('\n')
        .filter(|line| {
            let image = is_quoted_image_path(line);
            lifted += usize::from(image);
            !image && !line.trim().is_empty()
        })
        .collect::<Vec<_>>()
        .join("\n");
    (lifted, remaining)
}

/// Whether `line` is exactly one path quoted the way Bus quotes attachments
/// (`"path"`, with `\\` and `\"` escaped), naming an image Claude Code attaches.
fn is_quoted_image_path(line: &str) -> bool {
    let Some(path) = line
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return false;
    };
    let mut escaped = false;
    for character in path.chars() {
        match (escaped, character) {
            (false, '\\') => escaped = true,
            (false, '"') => return false,
            _ => escaped = false,
        }
    }
    std::path::Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["png", "jpg", "jpeg", "gif", "webp"]
                .iter()
                .any(|image| extension.eq_ignore_ascii_case(image))
        })
}

/// Background agents and session crons wake the turn again. Claude reports a
/// shell-only turn settled even while those unrelated shells remain running.
fn claude_turn_awaits_background(value: &Value) -> bool {
    let listed = |key: &str| value.get(key).and_then(Value::as_array);
    listed("session_crons").is_some_and(|crons| !crons.is_empty())
        || listed("background_tasks").is_some_and(|tasks| {
            tasks
                .iter()
                .any(|task| task.get("type").and_then(Value::as_str) != Some("shell"))
        })
}

/// The ids of the background shells a Claude `Stop` lists as running. Each one
/// that finishes wakes a turn of its own, whose `Stop` no longer lists it.
/// This inventory only guards wake ownership; it never delays an answer.
pub(crate) fn claude_running_shells(value: &Value) -> std::collections::BTreeSet<String> {
    value
        .get("background_tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|task| {
            task.get("type").and_then(Value::as_str) == Some("shell")
                && task.get("status").and_then(Value::as_str) == Some("running")
        })
        .filter_map(|task| task.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Claude can submit a background notification inside an existing prompt turn.
/// Keep recognition of its complete envelope shared by binding and shell wakes.
pub(crate) fn task_notification_body(prompt: &str) -> Option<&str> {
    prompt
        .trim()
        .strip_prefix("<task-notification>")?
        .strip_suffix("</task-notification>")
}

/// Claude's recurring self reminders are independent turns, not continuations
/// answering Bus input. The transcript records their origin; prompt wording
/// alone is insufficient. None means the matching row is not available yet:
/// Claude may publish it only after UserPromptSubmit hooks have run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PromptOrigin {
    Scheduled,
    Human,
    Other,
}

pub(crate) fn prompt_origin(value: &Value) -> Option<PromptOrigin> {
    use std::io::{Read, Seek, SeekFrom};
    let ((session, turn), path) = value
        .get("session_id")
        .and_then(Value::as_str)
        .zip(value.get("prompt_id").and_then(Value::as_str))
        .zip(value.get("transcript_path").and_then(Value::as_str))?;
    let read = || -> std::io::Result<Option<PromptOrigin>> {
        if !std::fs::metadata(path)?.is_file() {
            return Ok(None);
        }
        let mut file = std::fs::File::open(path)?;
        const MAX_TAIL: u64 = 2 * 1024 * 1024;
        let offset = file.metadata()?.len().saturating_sub(MAX_TAIL);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::new();
        file.take(MAX_TAIL).read_to_end(&mut bytes)?;
        let start = if offset == 0 {
            0
        } else {
            bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |index| index + 1)
        };
        for line in bytes[start..].split(|byte| *byte == b'\n').rev() {
            let Ok(row) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if row["type"] == "user"
                && row["sessionId"] == session
                && row["promptId"] == turn
                // Tool-result rows share the prompt id but do not describe
                // the input's origin. Keep looking for the submitted row.
                && row["promptSource"].is_string()
                && row["turnOrigin"].is_string()
            {
                return Ok(Some(
                    if row["promptSource"] == "typed" && row["turnOrigin"] == "human" {
                        PromptOrigin::Human
                    } else if row["isMeta"] == true
                        && row["promptSource"] == "system"
                        && row["turnOrigin"] == "scheduled"
                        && row["scheduledTaskId"]
                            .as_str()
                            .is_some_and(|id| !id.is_empty())
                    {
                        PromptOrigin::Scheduled
                    } else {
                        PromptOrigin::Other
                    },
                ));
            }
        }
        Ok(None)
    };
    read().ok().flatten()
}

#[cfg(test)]
#[path = "tests/hooks_test.rs"]
mod tests;
