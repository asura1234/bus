//! TUI restart entry, build receipt and immutable executable for rollback.
use crate::utils::restart_record::{self, BuildStamp, LivePid, TuiBuild};
use std::io;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

static RESTART_PENDING: AtomicBool = AtomicBool::new(false);

struct PendingRestart;

impl Drop for PendingRestart {
    fn drop(&mut self) {
        RESTART_PENDING.store(false, Ordering::Release);
    }
}

/// Run the public restart entry point off the UI thread. The CLI owns building,
/// preflight, the receipt and helper; only its helper may request save-and-quit.
/// Capture command output: the receipt and build errors must not enter the terminal.
pub(super) fn start_handoff() -> io::Result<()> {
    // The CLI locks after building; repeated keypresses must not start parallel builds.
    if RESTART_PENDING.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let pending = PendingRestart;
    let binary = crate::platform::fs::running_executable()?;
    let mut command = Command::new(binary);
    // Runtime --dev has the same meaning as for the public CLI, independently
    // of debug assertions or the dev-tools Cargo feature.
    if crate::messaging::diagnostics::dev_enabled() {
        command.arg("--dev");
    }
    command.args(["restart", "--confirm"]).stdin(Stdio::null());
    std::thread::Builder::new()
        .name("bus-restart".into())
        .spawn(move || {
            let _pending = pending;
            match command.output() {
                Ok(output) if output.status.success() => {
                    tracing::info!(event = "bus.restart.handoff", "Restart handoff queued");
                }
                Ok(output) => {
                    tracing::warn!(event = "bus.restart.failed", status = %output.status,
                        reason = %String::from_utf8_lossy(&output.stderr).trim());
                }
                Err(error) => tracing::warn!(event = "bus.restart.failed", %error),
            }
        })?;
    Ok(())
}

pub(super) fn record_ready(server: Option<&BuildStamp>) {
    if let Err(error) = record_ready_inner(server) {
        // Startup still works; restart will refuse without a complete receipt.
        tracing::warn!(%error, "could not record the running TUI build");
    }
}

fn record_ready_inner(server: Option<&BuildStamp>) -> Result<(), String> {
    let root = crate::utils::env::bus_data_dir().ok_or("BUS_DATA_DIR is missing")?;
    let sha = env!("BUS_BUILD_SHA");
    if server.is_none_or(|server| server.sha != sha) {
        return Err("TUI and server build SHAs differ; quit and resume the selected binary".into());
    }
    let binary = crate::platform::fs::running_executable().map_err(|error| error.to_string())?;
    let directory = restart_record::restarts_dir(&root).join("binaries");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let backup_binary = directory.join(sha);
    // Cargo may replace target/debug/bus while this TUI is running. Keep the
    // executable now, while its path still names the code loaded in this process.
    if !backup_binary.exists() {
        let temporary = directory.join(format!(".{}.tmp", std::process::id()));
        std::fs::copy(&binary, &temporary).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &backup_binary).map_err(|error| error.to_string())?;
    }
    let live = crate::platform::process_instance(std::process::id())
        .ok_or("could not identify this TUI process")?;
    restart_record::write_json(
        &restart_record::tui_build_path(&root),
        &TuiBuild {
            sha: sha.into(),
            binary,
            backup_binary,
            process: LivePid {
                pid: live.pid,
                birth: live.birth,
            },
        },
    )
}
