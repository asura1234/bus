//! AGY status-line tap and session-only `--settings` file management.
//!
//! AGY pipes live `session_id` and `quota` (`gemini-5h`, `gemini-weekly`,
//! `3p-5h`, `3p-weekly`) JSON to its configured `statusLine` command. Bus
//! installs this tap only inside `<spool>/agy-settings.json` passed via
//! `--settings`, leaving `~/.gemini/antigravity-cli/settings.json` untouched.
use crate::agents::providers::{
    usage::{
        Observation, ObservationManifest, UsageWindow, UsageWindows, FIVE_HOUR_MINUTES,
        WEEKLY_MINUTES,
    },
    ProviderKind,
};
use crate::utils::time as bus_time;
use serde_json::{json, Map, Value};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const SETTINGS_FILE: &str = "agy-settings.json";
const ORIGINAL_FILE: &str = "agy-statusline.json";
const USAGE_FILE: &str = "usage.json";
const RECURSION_GUARD: &str = "BUS_STATUSLINE_ACTIVE";

fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() && std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("Bus refuses to replace a symlink"));
    }
    crate::platform::atomic_write(path, bytes)
}

fn read_json(path: &Path) -> io::Result<Value> {
    if !path.metadata()?.is_file() {
        return Err(io::Error::other("AGY settings/statusline data is not a regular file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("AGY settings/statusline data is too large"));
    }
    let trimmed = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    Ok(serde_json::from_slice(trimmed)?)
}

pub(crate) fn read_usage(spool: &Path) -> io::Result<Observation> {
    Ok(serde_json::from_value(read_json(&spool.join(USAGE_FILE))?)?)
}

pub(crate) fn callback_command(binary: &Path) -> String {
    let program = binary.to_string_lossy();
    #[cfg(windows)]
    let program = program.replace('\\', "/");
    format!(
        "'{}' --bus-callback agy-statusline",
        program.replace('\'', "'\\''")
    )
}

fn global_original_statusline() -> Value {
    let Some(home) = std::env::home_dir() else {
        return json!({});
    };
    let path = home.join(".gemini/antigravity-cli/settings.json");
    read_json(&path)
        .ok()
        .and_then(|doc| doc.get("statusLine").cloned())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn parse_user_settings_value(raw: &str) -> Result<Map<String, Value>, String> {
    let trimmed = raw.trim();
    let value: Value = if trimmed.starts_with('{') {
        serde_json::from_str(trimmed)
            .map_err(|e| format!("Invalid inline AGY --settings JSON: {e}"))?
    } else {
        read_json(Path::new(trimmed))
            .map_err(|e| format!("Cannot read AGY --settings file {trimmed:?}: {e}"))?
    };
    value
        .as_object()
        .cloned()
        .ok_or_else(|| "AGY --settings must be a JSON object".into())
}

/// Build `<spool>/agy-settings.json` merging Bus worker defaults, any user
/// `--settings` values, and the Bus statusline tap. Never touches `~/.gemini/`.
pub(crate) fn install(
    spool: &Path,
    binary: &Path,
    user_settings_values: &[String],
    explicit_sandbox: bool,
) -> Result<PathBuf, String> {
    let mut merged = Map::new();
    merged.insert("queuedMessages".into(), json!("send-immediately"));
    merged.insert("allowNonWorkspaceAccess".into(), json!(true));

    let mut user_statusline: Option<Value> = None;
    for raw in user_settings_values {
        let map = parse_user_settings_value(raw)?;
        if let Some(status) = map.get("statusLine").filter(|v| v.is_object()) {
            user_statusline = Some(status.clone());
        }
        merged.extend(map);
    }

    if !explicit_sandbox && !merged.contains_key("enableTerminalSandbox") {
        merged.insert("enableTerminalSandbox".into(), json!(false));
    }

    let original = user_statusline.unwrap_or_else(global_original_statusline);
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
    let mut settings = read_json(&path)
        .map_err(|_| "Bus AGY resume settings are missing; relaunch required".to_owned())?;
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

pub(crate) fn windows(value: &Value, now_ms: u64) -> UsageWindows {
    let quota = &value["quota"];
    let five_hour = parse_window(&quota["gemini-5h"], FIVE_HOUR_MINUTES, now_ms)
        .or_else(|| parse_window(&quota["3p-5h"], FIVE_HOUR_MINUTES, now_ms));
    let weekly = parse_window(&quota["gemini-weekly"], WEEKLY_MINUTES, now_ms)
        .or_else(|| parse_window(&quota["3p-weekly"], WEEKLY_MINUTES, now_ms));
    UsageWindows { five_hour, weekly }
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
    if std::env::var_os(RECURSION_GUARD).is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::Builder::new().spawn(move || {
        let mut bytes = Vec::new();
        let result = io::stdin()
            .lock()
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes);
        let _ = tx.send(
            result
                .ok()
                .filter(|_| bytes.len() as u64 <= MAX_BYTES)
                .map(|_| bytes),
        );
    });
    if reader.is_err() {
        return;
    }
    let Ok(Some(input)) = rx.recv_timeout(Duration::from_secs(1)) else {
        return;
    };
    let spool = std::env::var_os("BUS_CALLBACK_DIR").map(PathBuf::from);
    let launch = std::env::var("BUS_LAUNCH_ID").unwrap_or_default();
    let output = render(spool.as_deref(), &launch, &input, COMMAND_TIMEOUT);
    let _ = io::stdout().lock().write_all(&output);
}

fn render(spool: Option<&Path>, launch: &str, input: &[u8], timeout: Duration) -> Vec<u8> {
    let original = spool.and_then(|p| read_json(&p.join(ORIGINAL_FILE)).ok());
    if let Some(spool) = spool {
        let _ = capture(spool, launch, input);
    }
    let Some(original) = original else {
        return Vec::new();
    };
    let enabled = original
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !enabled {
        return Vec::new();
    }
    let command = original
        .get("command")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    match command {
        Some(command) => passthrough(command, input, timeout).unwrap_or_default(),
        None => Vec::new(),
    }
}

fn passthrough(command: &str, input: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut process = tokio::process::Command::new(if cfg!(windows) {
            "cmd.exe"
        } else {
            "/bin/sh"
        });
        if cfg!(windows) {
            process.args(["/d", "/s", "/c", command]);
        } else {
            process.args(["-c", command]);
        }
        process
            .env(RECURSION_GUARD, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        process.process_group(0);
        let mut child = process.spawn()?;
        #[cfg(unix)]
        let group = child.id();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("Missing piped stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Missing piped stdout"))?;
        let mut output = Vec::new();
        let finished = tokio::time::timeout(timeout, async {
            tokio::try_join!(
                async {
                    let _ = stdin.write_all(input).await;
                    drop(stdin);
                    Ok::<(), io::Error>(())
                },
                async {
                    stdout.take(MAX_BYTES + 1).read_to_end(&mut output).await?;
                    if output.len() as u64 > MAX_BYTES {
                        return Err(io::Error::other("Status line output is too large"));
                    }
                    Ok(())
                },
                child.wait(),
            )
        })
        .await;
        if !matches!(finished, Ok(Ok(_))) {
            #[cfg(unix)]
            if let Some(group) = group {
                // SAFETY: fresh child's private process group.
                unsafe {
                    libc::kill(-(group as i32), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_millis(100), child.wait()).await;
        }
        if output.len() as u64 > MAX_BYTES {
            output.clear();
        }
        Ok(output)
    })
}
