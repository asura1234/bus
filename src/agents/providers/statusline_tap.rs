//! Shared non-blocking status-line tap IO, recursion guard, and HUD passthrough.
use crate::agents::providers::usage::Observation;
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(in crate::agents::providers) const MAX_BYTES: u64 = 2 * 1024 * 1024;
pub(in crate::agents::providers) const COMMAND_TIMEOUT: Duration = Duration::from_secs(2);
pub(in crate::agents::providers) const USAGE_FILE: &str = "usage.json";
const RECURSION_GUARD: &str = "BUS_STATUSLINE_ACTIVE";

pub(in crate::agents::providers) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() && std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("Bus refuses to replace a symlink"));
    }
    crate::platform::atomic_write(path, bytes)
}

pub(in crate::agents::providers) fn read_json(path: &Path) -> io::Result<Value> {
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
    let trimmed = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    Ok(serde_json::from_slice(trimmed)?)
}

pub(crate) fn read_usage(spool: &Path) -> io::Result<Observation> {
    Ok(serde_json::from_value(read_json(&spool.join(USAGE_FILE))?)?)
}

pub(in crate::agents::providers) fn callback_command(
    binary: &Path,
    adapter: &str,
    powershell_fallback: bool,
) -> String {
    let program = binary.to_string_lossy();
    #[cfg(windows)]
    if powershell_fallback && git_bash().is_none() {
        return format!(
            "& '{}' --bus-callback {adapter}",
            program.replace('\'', "''")
        );
    }
    #[cfg(not(windows))]
    let _ = powershell_fallback;
    #[cfg(windows)]
    let program = program.replace('\\', "/");
    format!(
        "'{}' --bus-callback {adapter}",
        program.replace('\'', "'\\''")
    )
}

pub(in crate::agents::providers) fn run_tap(
    original_file: &str,
    default_output: &[u8],
    respect_enabled_flag: bool,
    capture: impl FnOnce(&Path, &str, &[u8]) -> io::Result<()>,
) {
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
    let output = render_tap(
        spool.as_deref(),
        &launch,
        &input,
        COMMAND_TIMEOUT,
        original_file,
        default_output,
        respect_enabled_flag,
        capture,
    );
    let _ = io::stdout().lock().write_all(&output);
}

#[allow(clippy::too_many_arguments)]
pub(in crate::agents::providers) fn render_tap(
    spool: Option<&Path>,
    launch: &str,
    input: &[u8],
    timeout: Duration,
    original_file: &str,
    default_output: &[u8],
    respect_enabled_flag: bool,
    capture: impl FnOnce(&Path, &str, &[u8]) -> io::Result<()>,
) -> Vec<u8> {
    let original = spool.and_then(|p| read_json(&p.join(original_file)).ok());
    if let Some(spool) = spool {
        let _ = capture(spool, launch, input);
    }
    let Some(original) = original else {
        return Vec::new();
    };
    if respect_enabled_flag
        && !original
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    {
        return Vec::new();
    }
    let command = original
        .get("command")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty());
    match command {
        Some(command) => passthrough(command, input, timeout).unwrap_or_default(),
        None => default_output.to_vec(),
    }
}

pub(in crate::agents::providers) fn passthrough(
    command: &str,
    input: &[u8],
    timeout: Duration,
) -> io::Result<Vec<u8>> {
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
            crate::utils::env::path_var().and_then(|path| {
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
