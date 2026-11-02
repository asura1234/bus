//! Terminal-preserving quit/resume and health checks for the entire Bus.
use super::{prep, restart_record, HandoffIo, HandoffPlan, HEALTH_TIMEOUT};
use std::io;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

/// This helper only requests the normal save-and-quit. It never swaps servers
/// underneath a live TUI. The terminal process takes over after quit completes.
pub(crate) fn run_handoff_entry() -> io::Result<()> {
    let root = crate::utils::env::bus_data_dir()
        .ok_or_else(|| io::Error::other("BUS_DATA_DIR is required for a restart handoff"))?;
    let _ = crate::utils::logging::init_file_logging_at(
        crate::utils::paths::data_dir(),
        "herdr-client.log",
        &super::super::logging_options(),
    );
    let bytes = std::fs::read(restart_record::handoff_path(&root))?;
    let plan: HandoffPlan = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    // The publishing CLI assigns this helper the lock before it can request quit.
    let deadline = Instant::now() + Duration::from_secs(3);
    while restart_record::read_live_pid(&restart_record::restart_lock_path(&root))
        .is_none_or(|owner| owner.pid != std::process::id())
    {
        if Instant::now() >= deadline {
            return Err(io::Error::other("restart helper was not assigned the lock"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let request = crate::messaging::control::Request {
        id: format!("restart-quit-{}", std::process::id()),
        method: "bus.quit".into(),
        params: serde_json::json!({}),
    };
    let quit = crate::messaging::control::request(&root, &request).and_then(|response| {
        if response.ok {
            Ok(())
        } else {
            Err(format!("{:?}", response.error))
        }
    });
    let deadline = Instant::now() + QUIT_TIMEOUT;
    while quit.is_ok()
        && restart_record::handoff_path(&root).exists()
        && crate::platform::process_still_live(plan.tui.pid, plan.tui.birth)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Renaming claims the request atomically, either here or in the quitting TUI.
    // A timeout must not cancel a relaunch that already took over.
    let cancelled = restart_record::restarts_dir(&root).join("cancelled.json");
    if std::fs::rename(restart_record::handoff_path(&root), &cancelled).is_ok() {
        let reason = quit
            .err()
            .unwrap_or_else(|| "TUI did not finish saving and quitting".into());
        let mut effects = LiveHandoff::new(&root);
        super::finish(&plan, &mut effects, "quit_failed", &plan.new_binary, reason);
        let _ = std::fs::remove_file(cancelled);
        prep::release_restart_lock(&root);
    }
    Ok(())
}

/// Called only after the old client, its coordinator, runtime and terminal guard
/// have returned through the ordinary quit path. The caller stays as a passive
/// launcher so the shell keeps waiting for its terminal job, including rollback.
pub(crate) fn resume_after_quit() -> io::Result<()> {
    let Some(root) = crate::utils::env::bus_data_dir() else {
        return Ok(());
    };
    let pending = restart_record::handoff_path(&root);
    let Ok(bytes) = std::fs::read(&pending) else {
        return Ok(());
    };
    let plan: HandoffPlan = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if plan.tui.pid != std::process::id() {
        return Ok(());
    }
    let active = restart_record::restarts_dir(&root).join("active.json");
    if std::fs::rename(&pending, &active).is_err() {
        return Ok(()); // The quit timeout already cancelled this request.
    }
    prep::assign_restart_lock(&root, std::process::id()).map_err(io::Error::other)?;
    let mut effects = LiveHandoff::new(&root);
    let outcome = super::execute_handoff(&plan, &mut effects);
    prep::release_restart_lock(&root);
    let _ = std::fs::remove_file(active);
    let _ = std::fs::remove_file(restart_record::handoff_pid_path(&root));
    // All Bus work now belongs to the child. This process only holds the shell's
    // job open and reaps that child; it has no TUI, coordinator or API connection.
    if let Some(mut child) = effects.child.take() {
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("relaunched Bus exited {status}")));
        }
    }
    if outcome.outcome == "rollback_failed" || restart_record::persist_error_path(&root).exists() {
        return Err(io::Error::other(outcome.reason));
    }
    Ok(())
}

struct LiveHandoff {
    root: std::path::PathBuf,
    child: Option<Child>,
}

impl LiveHandoff {
    fn new(root: &Path) -> Self {
        Self {
            root: root.into(),
            child: None,
        }
    }
}

impl HandoffIo for LiveHandoff {
    fn stop(&mut self) -> Result<(), String> {
        super::stop_if_listening(
            super::super::launch::is_server_listening(),
            super::super::stop::stop_active_server,
        )
    }

    fn start(&mut self, binary: &Path, plan: &HandoffPlan) -> Result<u32, String> {
        // Public resume owns server launch and provider-session restoration.
        let mut command = resume_command(binary, plan);
        let child = command
            .spawn()
            .map_err(|error| format!("could not relaunch TUI {}: {error}", binary.display()))?;
        let pid = child.id();
        self.child = Some(child);
        Ok(pid)
    }

    fn healthy(&mut self, pid: u32, expected_sha: &str) -> Result<(), String> {
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .ok_or("TUI process is missing")?
                .try_wait()
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "TUI process {pid} exited {status} before becoming healthy"
                ));
            }
            let server = restart_record::read_build_stamp(&restart_record::build_path(&self.root));
            let tui = restart_record::read_tui_build(&self.root);
            let server_ready = server.as_ref().is_some_and(|server| {
                server.sha == expected_sha
                    && server
                        .pid
                        .is_some_and(|pid| crate::platform::process_instance(pid).is_some())
            });
            let tui_ready = tui.as_ref().is_some_and(|tui| {
                tui.sha == expected_sha
                    && tui.process.pid == pid
                    && crate::platform::process_still_live(pid, tui.process.birth)
            });
            if server_ready && tui_ready && super::server_answered(&super::restarted_api_socket()) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("restart health check failed: server ready={server_ready}, TUI/coordinator ready={tui_ready}; expected SHA {expected_sha}"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill(&mut self, pid: u32) {
        #[cfg(unix)]
        super::kill_pid(pid);
        if let Some(mut child) = self.child.take() {
            #[cfg(not(unix))]
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(not(unix))]
        let _ = pid;
    }

    fn now_ms(&self) -> u64 {
        crate::utils::time::now_ms()
    }
}

fn resume_command(binary: &Path, plan: &HandoffPlan) -> Command {
    let mut command = Command::new(binary);
    // The inherited descriptors, process group, working directory and host
    // terminal environment stay intact in iTerm2, Ghostty and other hosts.
    command
        .env_remove("BUS_SESSION_ID")
        .env_remove(crate::utils::env::SOCKET_PATH_ENV_VAR)
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove(crate::utils::env::STOP_WITHOUT_CLIENTS_ENV_VAR)
        .env_remove("HERDR_SESSION");
    if plan.dev {
        command.arg("--dev");
    }
    if plan.yolo {
        command.arg("--yolo");
    }
    if plan.explicit_data_dir {
        command.env("BUS_DATA_DIR", &plan.root);
    } else {
        command.env_remove("BUS_DATA_DIR");
        if let Some(id) = &plan.session_id {
            command.args(["resume", id]);
        } else {
            command.args(["resume", "--last"]);
        }
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resume_keeps_the_recorded_flags_and_selects_the_exact_session() {
        let mut plan = super::super::tests::plan(Path::new("/tmp/restart"));
        plan.yolo = true;
        let command = resume_command(Path::new("/new/bus"), &plan);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--dev", "--yolo", "resume", "abc"]
        );
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "BUS_DATA_DIR" && value.is_none()));
        plan.explicit_data_dir = true;
        let command = resume_command(Path::new("/new/bus"), &plan);
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["--dev", "--yolo"]);
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "BUS_DATA_DIR" && value == Some(plan.root.as_os_str())));
    }
}
