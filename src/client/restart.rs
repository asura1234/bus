//! The TUI's own build receipt and an immutable executable for rollback.
use crate::utils::restart_record::{self, BuildStamp, LivePid, TuiBuild};

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
    let binary = std::env::current_exe().map_err(|error| error.to_string())?;
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
