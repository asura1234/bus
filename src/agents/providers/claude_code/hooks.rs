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
/// Bus's own control commands are not work the reply depends on:
/// an orchestrator's reply is final while its waits on workers run for
/// minutes or hours, so they are left out.
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
        .filter(|task| {
            let reason = match task.get("command").and_then(Value::as_str) {
                Some(command) if is_bus_control(command) => "bus_control",
                Some(_) => "work_or_opaque_command",
                None => "missing_command",
            };
            let shell_id = task.get("id").and_then(Value::as_str);
            tracing::debug!(
                event = "bus.callback.shell_classified",
                shell_id,
                reason,
                held = reason != "bus_control",
                "Background shell classification"
            );
            reason != "bus_control"
        })
        .filter_map(|task| task.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Exclude only shells made up of Bus controls and setup. A build that reports
/// through Bus is still real background work. Quotes and escapes keep message
/// text from being mistaken for command separators; unfamiliar shell constructs
/// conservatively keep the hold.
pub(crate) fn is_bus_control(command: &str) -> bool {
    let Some(segments) = bus_control_segments(command) else {
        return false;
    };
    let mut saw_bus = false;
    for segment in segments {
        let words = shell_words(segment);
        let mut words = words.iter().map(String::as_str).peekable();
        while words.peek().is_some_and(|word| shell_assignment(word)) {
            words.next();
        }
        let Some(program) = words.next() else {
            continue;
        };
        if program == "cd" {
            continue;
        }
        let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
        if name != "bus" && name != "bus.exe" {
            return false;
        }
        while words
            .peek()
            .is_some_and(|next| matches!(*next, "--dev" | "--yolo"))
        {
            words.next();
        }
        if !matches!(
            words.next(),
            Some(
                "send"
                    | "wait"
                    | "history"
                    | "state"
                    | "agent"
                    | "room"
                    | "message"
                    | "request"
                    | "settings"
                    | "diagnostics"
                    | "sounds"
                    | "quit"
            )
        ) {
            return false;
        }
        saw_bus = true;
    }
    saw_bus
}

fn shell_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && name.chars().enumerate().all(|(index, ch)| {
                ch == '_' || ch.is_ascii_alphabetic() || (index > 0 && ch.is_ascii_digit())
            })
    })
}

// Segmentation already validated quoting and rejected command substitution.
// Decode shell words so quoted executable paths and setup paths remain direct.
fn shell_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in segment.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

// Keep unsupported compound shell syntax held instead of guessing its work.
fn bus_control_segments(command: &str) -> Option<Vec<&str>> {
    let mut quote = None;
    let mut escaped = false;
    let mut start = 0;
    let mut chars = command.char_indices().peekable();
    let mut segments = Vec::new();
    while let Some((index, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if is_redirect_operator(command, index, ch) {
            continue;
        }
        if quote != Some('\'')
            && (ch == '`' || (ch == '$' && chars.peek().is_some_and(|(_, ch)| *ch == '(')))
        {
            return None;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if matches!(ch, '(' | ')' | '{' | '}') {
            return None;
        } else if matches!(ch, '&' | '|' | ';' | '\n') {
            segments.push(&command[start..index]);
            start = index + ch.len_utf8();
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    segments.push(&command[start..]);
    Some(segments)
}

fn is_redirect_operator(command: &str, index: usize, ch: char) -> bool {
    match ch {
        '&' => command[..index].ends_with(['>', '<']) || command[index + 1..].starts_with('>'),
        '|' => command[..index].ends_with('>'),
        _ => false,
    }
}

#[cfg(test)]
#[path = "tests/hooks_test.rs"]
mod tests;
