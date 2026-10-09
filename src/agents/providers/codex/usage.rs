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
pub(crate) fn read_codex_rollout(path: &Path) -> std::io::Result<Option<UsageWindows>> {
    let mut file = std::fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(ROLLOUT_TAIL_BYTES).read_to_end(&mut tail)?;
    let tail = String::from_utf8_lossy(&tail);
    // A partial first line from the seek simply fails to parse.
    Ok(tail.lines().rev().find_map(codex_rate_limits))
}

fn codex_rate_limits(line: &str) -> Option<UsageWindows> {
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
    Some(windows)
}

#[cfg(test)]
#[path = "tests/usage_test.rs"]
mod tests;
