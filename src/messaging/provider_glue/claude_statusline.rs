//! Claude's status-line tap. Quota capture is advisory; stdout belongs to the
//! user's original command, including its ANSI escapes and trailing newlines.
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{UsageWindow, UsageWindows, FIVE_HOUR_MINUTES, WEEKLY_MINUTES};
use crate::messaging::{
    model::Provider, provider_glue::callbacks::Manifest, storage::io as bus_io,
};

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
const ORIGINAL: &str = "claude-statusline.json";
const USAGE: &str = "usage.json";
const RECURSION_GUARD: &str = "BUS_STATUSLINE_ACTIVE";

#[derive(Deserialize, Serialize)]
pub(super) struct Observation {
    pub(super) manifest: Manifest,
    pub(super) session_id: String,
    pub(super) read_at_ms: u64,
    pub(super) windows: UsageWindows,
}

fn read_json(path: &Path) -> io::Result<Value> {
    if !path.metadata()?.is_file() {
        return Err(io::Error::other("Status line data is not a regular file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("Status line data is too large"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) fn read_usage(spool: &Path) -> io::Result<Observation> {
    Ok(serde_json::from_value(read_json(&spool.join(USAGE))?)?)
}

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
    bus_io::atomic_write(&spool.join(ORIGINAL), &serde_json::to_vec(&original)?)?;
    let path = spool.join("claude-settings.json");
    let mut settings = read_json(&path)?;
    let mut status = original.as_object().cloned().unwrap_or_default();
    status.insert("type".into(), json!("command"));
    status.insert("command".into(), json!(callback_command(binary)));
    settings["statusLine"] = Value::Object(status);
    bus_io::atomic_write(&path, &serde_json::to_vec_pretty(&settings)?)
}

fn callback_command(binary: &Path) -> String {
    let program = binary.to_string_lossy();
    #[cfg(windows)]
    if git_bash().is_none() {
        return format!(
            "& '{}' --bus-callback claude-statusline",
            program.replace('\'', "''")
        );
    }
    // Claude uses Git Bash on Windows when available. Its status-line command
    // therefore needs POSIX quoting, not the native terminal's PowerShell `&`.
    #[cfg(windows)]
    let program = program.replace('\\', "/");
    format!(
        "'{}' --bus-callback claude-statusline",
        program.replace('\'', "'\\''")
    )
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
    let manifest: Manifest = serde_json::from_value(read_json(&spool.join("manifest.json"))?)?;
    if manifest.provider != Provider::ClaudeCode || manifest.launch_id != launch {
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
    bus_io::atomic_write(&spool.join(USAGE), &serde_json::to_vec(&record)?)
}

/// This callback deliberately skips ordinary hook logging/locks and the `{}`
/// hook response. Even a broken spool must not suppress the user's status line.
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
    let original = spool.and_then(|p| read_json(&p.join(ORIGINAL)).ok());
    if let Some(spool) = spool {
        let _ = capture(spool, launch, input);
    }
    let Some(original) = original else {
        return Vec::new();
    };
    let command = original
        .get("command")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    match command {
        Some(command) => passthrough(command, input, timeout).unwrap_or_default(),
        None => b"Claude | Bus\n".to_vec(),
    }
}

fn passthrough(command: &str, input: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut process = statusline_shell();
        process
            .arg(command)
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
        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut output = Vec::new();
        let finished = tokio::time::timeout(timeout, async {
            tokio::try_join!(
                async {
                    // Commands that do not read stdin may close it immediately.
                    // Still drain their stdout and let them finish normally.
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
            // Kill the whole shell group on Unix so a timed-out HUD cannot
            // leave a grandchild holding pipes or running after each redraw.
            #[cfg(unix)]
            if let Some(group) = group {
                // SAFETY: this is the fresh child's private process group.
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

fn statusline_shell() -> tokio::process::Command {
    #[cfg(not(windows))]
    {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.arg("-c");
        command
    }
    #[cfg(windows)]
    {
        if let Some(bash) = git_bash() {
            let mut command = tokio::process::Command::new(bash);
            command.arg("-c");
            command
        } else {
            let mut command = tokio::process::Command::new("powershell.exe");
            command.args(["-NoProfile", "-Command"]);
            command
        }
    }
}

#[cfg(windows)]
fn git_bash() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|p| p.join("bash.exe"))
                    .find(|p| p.is_file())
            })
        })
        .or_else(|| {
            ["ProgramFiles", "ProgramFiles(x86)"]
                .into_iter()
                .filter_map(std::env::var_os)
                .map(|p| PathBuf::from(p).join("Git/bin/bash.exe"))
                .find(|p| p.is_file())
        })
}

#[cfg(test)]
#[path = "claude_statusline_tests.rs"]
mod tests;
