//! Claude's status-line tap. Quota capture is advisory; stdout belongs to the
//! user's original command, including its ANSI escapes and trailing newlines.
#[cfg(test)]
use std::time::Duration;
use std::{
    io,
    path::{Path, PathBuf},
};

use serde_json::{json, Value};

use crate::agents::providers::statusline_tap::{
    self, atomic_write, read_json, USAGE_FILE as USAGE,
};
#[cfg(test)]
use crate::agents::providers::statusline_tap::{
    passthrough, read_usage, COMMAND_TIMEOUT, MAX_BYTES,
};
use crate::agents::providers::usage::{
    Observation, ObservationManifest, UsageWindow, UsageWindows, FIVE_HOUR_MINUTES, WEEKLY_MINUTES,
};
use crate::utils::time as bus_io;

const ORIGINAL: &str = "claude-statusline.json";

/// Resolve ordinary settings from low to high precedence. Keep presentation
/// fields (padding, refreshInterval, etc.) as well as the original command.
fn original_settings(paths: &[PathBuf]) -> Value {
    let mut result = json!({});
    for path in paths {
        let Ok(document) = read_json(path) else {
            continue;
        };
        if let Some(status) = document.get("statusLine") {
            if let (Some(dest), Some(source)) = (result.as_object_mut(), status.as_object()) {
                dest.extend(source.clone());
            } else {
                result = status.clone();
            }
        }
    }
    result
}

/// Install only in the launch spool; never modify the user's settings.
pub(crate) fn install(spool: &Path, project: &Path, binary: &Path) -> io::Result<()> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".claude")));
    let mut paths: Vec<_> = config
        .into_iter()
        .map(|p| p.join("settings.json"))
        .collect();
    paths.extend([
        project.join(".claude/settings.json"),
        project.join(".claude/settings.local.json"),
    ]);
    install_with_settings(spool, binary, original_settings(&paths))
}

fn install_with_settings(spool: &Path, binary: &Path, original: Value) -> io::Result<()> {
    atomic_write(&spool.join(ORIGINAL), &serde_json::to_vec(&original)?)?;
    let path = spool.join("claude-settings.json");
    let mut settings = read_json(&path)?;
    let mut status = original.as_object().cloned().unwrap_or_default();
    status.insert("type".into(), json!("command"));
    status.insert("command".into(), json!(callback_command(binary)));
    settings["statusLine"] = Value::Object(status);
    atomic_write(&path, &serde_json::to_vec_pretty(&settings)?)
}

/// Resume moves only an existing Bus-owned tap to this executable. The saved
/// original HUD is left alone so the tap is never captured as the user's HUD.
pub(crate) fn rebind(spool: &Path, binary: &Path) -> io::Result<bool> {
    let path = spool.join("claude-settings.json");
    let mut settings = read_json(&path)?;
    let command = callback_command(binary);
    let Some(status) = settings
        .get_mut("statusLine")
        .and_then(Value::as_object_mut)
    else {
        return Ok(false);
    };
    let owned = status.get("type").and_then(Value::as_str) == Some("command")
        && status
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|c| c != command && c.ends_with(" --bus-callback claude-statusline"));
    if !owned {
        return Ok(false);
    }
    status.insert("command".into(), json!(command));
    atomic_write(&path, &serde_json::to_vec_pretty(&settings)?)?;
    Ok(true)
}

fn callback_command(binary: &Path) -> String {
    statusline_tap::callback_command(binary, "claude-statusline", true)
}

fn windows(value: &Value) -> UsageWindows {
    let parse = |key: &str, window_minutes| {
        let value = &value["rate_limits"][key];
        let used_percent = value["used_percentage"].as_f64()?;
        if !used_percent.is_finite() || !(0.0..=100.0).contains(&used_percent) {
            return None;
        }
        Some(UsageWindow {
            used_percent,
            resets_at: value["resets_at"].as_u64(),
            window_minutes,
        })
    };
    UsageWindows {
        five_hour: parse("five_hour", FIVE_HOUR_MINUTES),
        weekly: parse("seven_day", WEEKLY_MINUTES),
    }
}

fn capture(spool: &Path, launch: &str, input: &[u8]) -> io::Result<()> {
    let manifest: ObservationManifest =
        serde_json::from_value(read_json(&spool.join("manifest.json"))?)?;
    if !manifest.provider.uses_claude_harness() || manifest.launch_id != launch {
        return Err(io::Error::other("Status line launch mismatch"));
    }
    let value: Value = serde_json::from_slice(input)?;
    let session_id = value["session_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| io::Error::other("Status line has no session"))?
        .to_owned();
    // Write missing windows too: a later observation without allowance data
    // invalidates an older one instead of leaving an apparently fresh quota.
    let record = Observation {
        manifest,
        session_id,
        read_at_ms: bus_io::now_ms(),
        windows: windows(&value),
    };
    atomic_write(&spool.join(USAGE), &serde_json::to_vec(&record)?)
}

/// This callback deliberately skips ordinary hook logging/locks and the `{}`
/// hook response. Even a broken spool must not suppress the user's status line.
pub(crate) fn run() {
    statusline_tap::run_tap(ORIGINAL, b"Claude | Bus\n", false, capture);
}

#[cfg(test)]
fn render(spool: Option<&Path>, launch: &str, input: &[u8], timeout: Duration) -> Vec<u8> {
    statusline_tap::render_tap(
        spool,
        launch,
        input,
        timeout,
        ORIGINAL,
        b"Claude | Bus\n",
        false,
        capture,
    )
}

#[cfg(test)]
#[path = "tests/statusline_test.rs"]
mod tests;
