//! AGY status-line tap and session-only `--settings` file management.
//!
//! AGY pipes live `session_id`, `model`, and `quota` (`gemini-5h`, `gemini-weekly`,
//! `3p-5h`, `3p-weekly`) JSON to its configured `statusLine` command. Bus
//! installs this tap only inside `<spool>/agy-settings.json` passed via
//! `--settings`, leaving `~/.gemini/antigravity-cli/settings.json` unmodified.
use crate::agents::providers::{
    statusline_tap::{self, atomic_write, read_json, USAGE_FILE},
    usage::{
        Observation, ObservationManifest, UsageWindow, UsageWindows, FIVE_HOUR_MINUTES,
        WEEKLY_MINUTES,
    },
    ProviderKind,
};
use crate::utils::time as bus_time;
use serde_json::{json, Map, Value};
use std::{
    io,
    path::{Path, PathBuf},
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

const SETTINGS_FILE: &str = "agy-settings.json";
const ORIGINAL_FILE: &str = "agy-statusline.json";

fn callback_command(binary: &Path) -> String {
    statusline_tap::callback_command(binary, "agy-statusline", false)
}

fn global_settings() -> Option<Value> {
    let home = std::env::home_dir()?;
    read_json(&home.join(".gemini/antigravity-cli/settings.json")).ok()
}

fn resolve_settings_path(trimmed: &str, launch_cwd: &Path) -> PathBuf {
    if trimmed == "~" || trimmed.starts_with("~/") {
        if let Some(home) = std::env::home_dir() {
            return if trimmed == "~" {
                home
            } else {
                home.join(&trimmed[2..])
            };
        }
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        launch_cwd.join(path)
    }
}

fn parse_user_settings_value(raw: &str, launch_cwd: &Path) -> Result<Map<String, Value>, String> {
    let trimmed = raw.trim();
    let value: Value = if trimmed.starts_with('{') {
        serde_json::from_str(trimmed)
            .map_err(|e| format!("Invalid inline AGY --settings JSON: {e}"))?
    } else {
        let resolved = resolve_settings_path(trimmed, launch_cwd);
        read_json(&resolved)
            .map_err(|e| format!("Cannot read AGY --settings file {trimmed:?}: {e}"))?
    };
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "AGY --settings must be a JSON object".into())
}

/// Build `<spool>/agy-settings.json` merging Bus worker defaults, any user
/// `--settings` values, and the Bus statusline tap. Reads
/// `~/.gemini/antigravity-cli/settings.json` read-only and never writes to `~/.gemini/`.
pub(crate) fn install(
    spool: &Path,
    launch_cwd: &Path,
    binary: &Path,
    user_settings_values: &[String],
    explicit_sandbox: bool,
) -> Result<PathBuf, String> {
    let mut merged = Map::new();
    merged.insert("queuedMessages".into(), json!("send-immediately"));
    merged.insert("allowNonWorkspaceAccess".into(), json!(true));

    let mut user_statusline: Option<Value> = None;
    for raw in user_settings_values {
        let map = parse_user_settings_value(raw, launch_cwd)?;
        if let Some(status) = map.get("statusLine").filter(|v| v.is_object()) {
            user_statusline = Some(status.clone());
        }
        merged.extend(map);
    }

    let global = global_settings();
    let global_sandbox_configured = global
        .as_ref()
        .and_then(|doc| doc.get("enableTerminalSandbox"))
        .and_then(Value::as_bool)
        .is_some();

    if !explicit_sandbox
        && !merged.contains_key("enableTerminalSandbox")
        && !global_sandbox_configured
    {
        merged.insert("enableTerminalSandbox".into(), json!(false));
    }

    let original = user_statusline.unwrap_or_else(|| {
        global
            .and_then(|doc| doc.get("statusLine").cloned())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}))
    });
    atomic_write(
        &spool.join(ORIGINAL_FILE),
        &serde_json::to_vec(&original).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    let mut status = original.as_object().cloned().unwrap_or_default();
    status.insert("enabled".into(), json!(true));
    status.insert("type".into(), json!("command"));
    status.insert("command".into(), json!(callback_command(binary)));
    status
        .entry("stack_with_default")
        .or_insert_with(|| json!(true));
    merged.insert("statusLine".into(), Value::Object(status));

    let path = spool.join(SETTINGS_FILE);
    let formatted = serde_json::to_vec_pretty(&Value::Object(merged)).map_err(|e| e.to_string())?;
    atomic_write(&path, &formatted).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Rebind `<spool>/agy-settings.json` to `binary` during cold resume.
pub(crate) fn rebind(spool: &Path, binary: &Path) -> Result<(PathBuf, bool), String> {
    let path = spool.join(SETTINGS_FILE);
    let mut settings = match read_json(&path) {
        Ok(settings) => settings,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err("Bus AGY resume settings are missing; relaunch required".into());
        }
        Err(e) => {
            tracing::warn!(
                event = "bus.resume.settings_invalid",
                provider = "agy",
                error_kind = ?e.kind(),
                "Cannot read AGY resume settings file"
            );
            return Err(format!(
                "Bus AGY resume settings cannot be read ({e}); relaunch required"
            ));
        }
    };
    let command = callback_command(binary);
    let Some(status) = settings
        .get_mut("statusLine")
        .and_then(Value::as_object_mut)
    else {
        return Ok((path, false));
    };
    let owned = status.get("type").and_then(Value::as_str) == Some("command")
        && status
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|c| c.ends_with(" --bus-callback agy-statusline"));
    if !owned {
        return Ok((path, false));
    }
    if status.get("command").and_then(Value::as_str) == Some(command.as_str()) {
        return Ok((path, false));
    }
    status.insert("command".into(), json!(command));
    let formatted = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    atomic_write(&path, &formatted).map_err(|e| e.to_string())?;
    Ok((path, true))
}

fn parse_window(bucket: &Value, window_minutes: u64, now_ms: u64) -> Option<UsageWindow> {
    let remaining = bucket.get("remaining_fraction")?.as_f64()?;
    if !remaining.is_finite() || !(0.0..=1.0).contains(&remaining) {
        return None;
    }
    let used_percent = ((1.0 - remaining) * 100.0).clamp(0.0, 100.0);
    let resets_at = bucket
        .get("reset_time")
        .and_then(Value::as_str)
        .and_then(|text| OffsetDateTime::parse(text, &Rfc3339).ok())
        .and_then(|dt| u64::try_from(dt.unix_timestamp()).ok())
        .or_else(|| {
            bucket
                .get("reset_in_seconds")
                .and_then(Value::as_u64)
                .map(|secs| (now_ms / 1000).saturating_add(secs))
        });
    Some(UsageWindow {
        used_percent,
        resets_at,
        window_minutes,
    })
}

fn active_model_hint(value: &Value) -> Option<&str> {
    let model = value.get("model")?;
    if let Some(s) = model.as_str() {
        return Some(s).filter(|s| !s.trim().is_empty());
    }
    model
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            model
                .get("display_name")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
        })
}

fn pick_higher(a: Option<UsageWindow>, b: Option<UsageWindow>) -> Option<UsageWindow> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if b.used_percent > a.used_percent {
                Some(b)
            } else {
                Some(a)
            }
        }
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

pub(in crate::agents::providers) fn windows(value: &Value, now_ms: u64) -> UsageWindows {
    let quota = &value["quota"];
    let gemini_5h = parse_window(&quota["gemini-5h"], FIVE_HOUR_MINUTES, now_ms);
    let gemini_wk = parse_window(&quota["gemini-weekly"], WEEKLY_MINUTES, now_ms);
    let third_5h = parse_window(&quota["3p-5h"], FIVE_HOUR_MINUTES, now_ms);
    let third_wk = parse_window(&quota["3p-weekly"], WEEKLY_MINUTES, now_ms);

    match active_model_hint(value) {
        Some(model) if model.trim().to_ascii_lowercase().starts_with("gemini") => UsageWindows {
            five_hour: gemini_5h.or(third_5h),
            weekly: gemini_wk.or(third_wk),
        },
        Some(_) => UsageWindows {
            five_hour: third_5h.or(gemini_5h),
            weekly: third_wk.or(gemini_wk),
        },
        None => UsageWindows {
            five_hour: pick_higher(gemini_5h, third_5h),
            weekly: pick_higher(gemini_wk, third_wk),
        },
    }
}

fn capture(spool: &Path, launch: &str, input: &[u8]) -> io::Result<()> {
    let manifest: ObservationManifest =
        serde_json::from_value(read_json(&spool.join("manifest.json"))?)?;
    if manifest.provider != ProviderKind::Agy || manifest.launch_id != launch {
        return Err(io::Error::other("AGY status line launch mismatch"));
    }
    let value: Value = serde_json::from_slice(input)?;
    let session_id = value["session_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| value["conversation_id"].as_str().filter(|s| !s.is_empty()))
        .ok_or_else(|| io::Error::other("AGY status line has no active session yet"))?
        .to_owned();
    let now_ms = bus_time::now_ms();
    let record = Observation {
        manifest,
        session_id,
        read_at_ms: now_ms,
        windows: windows(&value, now_ms),
    };
    atomic_write(&spool.join(USAGE_FILE), &serde_json::to_vec(&record)?)
}

pub(crate) fn run() {
    statusline_tap::run_tap(ORIGINAL_FILE, b"", true, capture);
}
