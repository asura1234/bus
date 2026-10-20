//! Bounded Codex rollout quota parsing, without coordinator state.
use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::agents::providers::usage::{
    UsageWindow, UsageWindows, FIVE_HOUR_MINUTES, WEEKLY_MINUTES,
};
use serde_json::Value;

/// Enough rollout tail to reach the last `token_count` event after a turn.
const ROLLOUT_TAIL_BYTES: u64 = 256 * 1024;
/// The most rollout Bus reads back to see every turn since a message was
/// sent. One turn can write megabytes (live, 2026-10-20: 5.7 MB).
const ROLLOUT_WINDOW_BYTES: u64 = 64 * 1024 * 1024;
/// How far before a send the backward rollout read must reach. One line's
/// timestamp is no proof that every later line came after it: a wall clock
/// stepped back can stamp a line inside the window as older than the send.
/// Reading further only adds candidates the send time then rejects.
const SEND_CLOCK_MARGIN_MS: u64 = 60_000;

/// Accept only Codex rollout files: hook payloads name the path, and Bus
/// should not parse arbitrary files a payload points at.
pub(crate) fn codex_rollout_path(value: &Value) -> Option<&Path> {
    let path = Path::new(value.get("transcript_path")?.as_str()?);
    let name = path.file_name()?.to_str()?;
    (path.is_absolute() && name.starts_with("rollout-") && name.ends_with(".jsonl")).then_some(path)
}

/// The newest `token_count` rate limits in a Codex rollout. The format is
/// undocumented: windows are classified by duration because Codex may put
/// the weekly window in either slot. `Ok(None)` means none were found.
#[derive(Debug, PartialEq)]
pub(crate) struct CodexUsage {
    pub(crate) windows: UsageWindows,
    pub(crate) observed_at_ms: u64,
}

#[cfg(test)]
fn read_codex_rollout(path: &Path) -> std::io::Result<Option<UsageWindows>> {
    Ok(read_codex_observation(path)?.map(|observation| observation.windows))
}

pub(crate) fn read_codex_observation(path: &Path) -> std::io::Result<Option<CodexUsage>> {
    Ok(read_rollout_tail(path)?
        .lines()
        .rev()
        .find_map(codex_rate_limits))
}

/// A partial first line from the seek simply fails to parse.
fn read_rollout_tail(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(ROLLOUT_TAIL_BYTES).read_to_end(&mut tail)?;
    Ok(String::from_utf8_lossy(&tail).into_owned())
}

/// How a Codex turn ended, from its rollout `task_complete` event.
#[derive(Debug, PartialEq)]
pub(crate) enum CodexTurnEnd {
    /// The error that ended the turn. Codex fires no Stop hook for it.
    Error(String),
    /// The turn's final answer: what its Stop hook would have reported, for
    /// when that hook never ran (Codex runs none once its cwd is removed).
    Final(String),
}

/// How Codex turn `turn_id` ended; `Ok(None)` while the rollout tail records
/// no end for it, or its end carries neither an error nor an answer.
pub(crate) fn read_codex_turn_end(
    path: &Path,
    turn_id: &str,
) -> std::io::Result<Option<CodexTurnEnd>> {
    let tail = read_rollout_tail(path)?;
    Ok(tail.lines().rev().find_map(|line| {
        let event: Value = serde_json::from_str(line).ok()?;
        let payload = &event["payload"];
        if event["type"] != "event_msg"
            || payload["type"] != "task_complete"
            || payload["turn_id"] != turn_id
        {
            return None;
        }
        if let Some(message) = payload["error"]["message"].as_str() {
            return Some(CodexTurnEnd::Error(
                match payload["error"]["codex_error_info"].as_str() {
                    Some(code) => format!("{message} ({code})"),
                    None => message.to_owned(),
                },
            ));
        }
        let answer = payload["last_agent_message"].as_str()?;
        Some(CodexTurnEnd::Final(answer.to_owned()))
    }))
}

/// Where the prompt of a message Bus typed but never saw start ran, per
/// `codex_prompt_turn`. Each outcome but `Found` attributes nothing.
#[derive(Debug, PartialEq)]
pub(crate) enum PromptTurn {
    /// The one turn that ran it.
    Found(String),
    /// No turn in the window ran it.
    NoMatch,
    /// More than one turn could be the message's, so none is.
    Ambiguous,
    /// The one turn that ran it is stamped before the send.
    StartedBeforeSend,
    /// The rollout written since the send is too long to read whole: an
    /// earlier matching turn could hide past what was read.
    WindowTooLong,
}

/// The turn that ran a prompt `matches` accepts and started at or after
/// `since_ms`, the send. A clock stepped back after the send can stamp the
/// message's own turn as older, so every matching turn started within
/// `SEND_CLOCK_MARGIN_MS` before the send counts against uniqueness too.
pub(crate) fn codex_prompt_turn(
    path: &Path,
    since_ms: u64,
    matches: impl Fn(&str) -> bool,
) -> std::io::Result<PromptTurn> {
    prompt_turn_within(path, since_ms, matches, ROLLOUT_WINDOW_BYTES)
}

#[cfg(test)]
pub(crate) fn find_codex_prompt_turn(
    path: &Path,
    since_ms: u64,
    matches: impl Fn(&str) -> bool,
) -> std::io::Result<Option<String>> {
    find_prompt_turn_within(path, since_ms, matches, ROLLOUT_WINDOW_BYTES)
}

#[cfg(test)]
fn find_prompt_turn_within(
    path: &Path,
    since_ms: u64,
    matches: impl Fn(&str) -> bool,
    cap: u64,
) -> std::io::Result<Option<String>> {
    Ok(match prompt_turn_within(path, since_ms, matches, cap)? {
        PromptTurn::Found(turn) => Some(turn),
        _ => None,
    })
}

fn prompt_turn_within(
    path: &Path,
    since_ms: u64,
    matches: impl Fn(&str) -> bool,
    cap: u64,
) -> std::io::Result<PromptTurn> {
    let Some(text) = read_rollout_since(path, since_ms, cap)? else {
        return Ok(PromptTurn::WindowTooLong);
    };
    let earliest_ms = since_ms.saturating_sub(SEND_CLOCK_MARGIN_MS);
    let mut found: Vec<(String, u64)> = Vec::new();
    for prompt in turn_prompts(&text) {
        if prompt.started_at_ms >= earliest_ms
            && matches(&prompt.text)
            && !found.iter().any(|(seen, _)| *seen == prompt.turn)
        {
            found.push((prompt.turn, prompt.started_at_ms));
        }
    }
    Ok(match found.as_slice() {
        [] => PromptTurn::NoMatch,
        [(turn, started_at_ms)] if *started_at_ms >= since_ms => PromptTurn::Found(turn.clone()),
        [_] => PromptTurn::StartedBeforeSend,
        _ => PromptTurn::Ambiguous,
    })
}

/// Every prompt Codex recorded for turn `turn_id` since `since_ms`, once per
/// submission so identical prompts can be counted; none when the rollout
/// since then is too long to read whole.
pub(crate) fn codex_turn_prompts(
    path: &Path,
    turn_id: &str,
    since_ms: u64,
) -> std::io::Result<Vec<String>> {
    let Some(text) = read_rollout_since(path, since_ms, ROLLOUT_WINDOW_BYTES)? else {
        return Ok(Vec::new());
    };
    // Codex records each submission as a message item and as an event; the
    // events alone count each one once.
    Ok(turn_prompts(&text)
        .into_iter()
        .filter(|prompt| prompt.event && prompt.turn == turn_id)
        .map(|prompt| prompt.text)
        .collect())
}

/// A user prompt recorded in a rollout.
struct TurnPrompt {
    turn: String,
    started_at_ms: u64,
    text: String,
    /// Recorded as an event, which Codex writes once per submission.
    event: bool,
}

/// Each user prompt in rollout `text`, with the turn it ran in.
fn turn_prompts(text: &str) -> Vec<TurnPrompt> {
    let mut current: Option<(String, u64)> = None;
    let mut prompts = Vec::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = &event["payload"];
        if event["type"] == "event_msg" && payload["type"] == "task_started" {
            current = payload["turn_id"]
                .as_str()
                .zip(event["timestamp"].as_str().and_then(rfc3339_ms))
                .map(|(turn, at)| (turn.to_owned(), at));
            continue;
        }
        if let (Some((turn, started_at_ms)), Some(text)) = (&current, user_prompt(&event)) {
            // A completed item names its turn; trust that over line order.
            let turn = payload["turn_id"].as_str().unwrap_or(turn);
            prompts.push(TurnPrompt {
                turn: turn.to_owned(),
                started_at_ms: *started_at_ms,
                text,
                event: event["type"] == "event_msg",
            });
        }
    }
    prompts
}

/// The rollout's whole lines from a line stamped `SEND_CLOCK_MARGIN_MS`
/// before `since_ms` on, when the last `cap` bytes reach back that far;
/// `Ok(None)` when they do not. Codex appends events as they happen, so every
/// line written since `since_ms` follows that one, unless the clock stepped
/// back by more than the margin.
fn read_rollout_since(path: &Path, since_ms: u64, cap: u64) -> std::io::Result<Option<String>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let mut size = ROLLOUT_TAIL_BYTES.min(cap);
    loop {
        let start = length.saturating_sub(size);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        (&mut file).take(size).read_to_end(&mut bytes)?;
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if start > 0 {
            // The first line begins before the seek.
            let whole = text.find('\n').map_or(text.len(), |at| at + 1);
            text.drain(..whole);
        }
        let reached = start == 0
            || text
                .lines()
                .find_map(|line| {
                    let event: Value = serde_json::from_str(line).ok()?;
                    rfc3339_ms(event["timestamp"].as_str()?)
                })
                .is_some_and(|at| at < since_ms.saturating_sub(SEND_CLOCK_MARGIN_MS));
        if reached {
            return Ok(Some(text));
        }
        if size >= cap {
            return Ok(None);
        }
        size = size.saturating_mul(4).min(cap);
    }
}

/// The prompt text of a user input event: an `item_completed` `UserMessage`
/// (Codex 0.163.0), a `user` message item, or a legacy `user_message` event.
fn user_prompt(event: &Value) -> Option<String> {
    let payload = &event["payload"];
    let content = match (event["type"].as_str()?, payload["type"].as_str()?) {
        ("event_msg", "user_message") => return Some(payload["message"].as_str()?.to_owned()),
        ("event_msg", "item_completed") if payload["item"]["type"] == "UserMessage" => {
            payload["item"]["content"].as_array()?
        }
        ("response_item", "message") if payload["role"] == "user" => {
            payload["content"].as_array()?
        }
        _ => return None,
    };
    Some(
        content
            .iter()
            .filter_map(|part| part["text"].as_str())
            .collect(),
    )
}

fn rfc3339_ms(value: &str) -> Option<u64> {
    time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339)
        .ok()
        .and_then(|at| u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok())
}

fn codex_rate_limits(line: &str) -> Option<CodexUsage> {
    let event: Value = serde_json::from_str(line).ok()?;
    if event["type"] != "event_msg" || event["payload"]["type"] != "token_count" {
        return None;
    }
    let limits = event["payload"]["rate_limits"].as_object()?;
    let mut windows = UsageWindows::default();
    for slot in ["primary", "secondary"] {
        let Some(window) = limits.get(slot).filter(|w| w.is_object()) else {
            continue;
        };
        let (Some(used_percent), Some(window_minutes)) = (
            window["used_percent"].as_f64(),
            window["window_minutes"].as_u64(),
        ) else {
            continue;
        };
        let parsed = UsageWindow {
            used_percent,
            resets_at: window["resets_at"].as_u64(),
            window_minutes,
        };
        match window_minutes {
            FIVE_HOUR_MINUTES => windows.five_hour = Some(parsed),
            WEEKLY_MINUTES => windows.weekly = Some(parsed),
            _ => {}
        }
    }
    let observed_at_ms = event["timestamp"]
        .as_str()
        .and_then(rfc3339_ms)
        .unwrap_or(0);
    Some(CodexUsage {
        windows,
        observed_at_ms,
    })
}

#[cfg(test)]
#[path = "tests/usage_test.rs"]
mod tests;
