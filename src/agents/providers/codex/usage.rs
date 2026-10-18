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

/// The error that ended Codex turn `turn_id`, from its `task_complete`
/// event. Codex fires no Stop hook for such turns.
pub(crate) fn read_codex_turn_error(path: &Path, turn_id: &str) -> std::io::Result<Option<String>> {
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
        let message = payload["error"]["message"].as_str()?;
        Some(match payload["error"]["codex_error_info"].as_str() {
            Some(code) => format!("{message} ({code})"),
            None => message.to_owned(),
        })
    }))
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
        .and_then(|value| {
            time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).ok()
        })
        .and_then(|at| u64::try_from(at.unix_timestamp_nanos() / 1_000_000).ok())
        .unwrap_or(0);
    Some(CodexUsage {
        windows,
        observed_at_ms,
    })
}

#[cfg(test)]
#[path = "tests/usage_test.rs"]
mod tests;
