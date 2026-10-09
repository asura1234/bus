//! Claude observation hooks and provider-input normalization.

use super::statusline;
use crate::agents::providers::{
    hook_json::{self, HookContext, HookContract},
    spool::{field, Parsed},
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
        "Stop" if claude_turn_awaits_background(value) => {
            Ok(Parsed::BackgroundPending { session, turn })
        }
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

#[cfg(test)]
#[path = "tests/hooks_test.rs"]
mod tests;
