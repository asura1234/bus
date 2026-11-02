//! Terminal-preserving quit/resume and health checks for the entire Bus.
use super::{prep, restart_record, HandoffIo, HandoffPlan, HEALTH_TIMEOUT};
use std::io;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const QUIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The serialized plan is immutable publication metadata, not a second read of
/// the request file that can disappear or become unreadable during shutdown.
pub(crate) fn run_handoff_entry(serialized: &str) -> io::Result<()> {
    let plan: HandoffPlan = serde_json::from_str(serialized).map_err(io::Error::other)?;
    let root = &plan.root;
    let _ = crate::utils::logging::init_file_logging_at(
        crate::utils::paths::data_dir(),
        "herdr-client.log",
        &super::super::logging_options(),
    );
    let deadline = Instant::now() + Duration::from_secs(3);
    while restart_record::read_live_pid(&restart_record::restart_lock_path(root))
        .is_none_or(|owner| owner.pid != std::process::id())
    {
        if Instant::now() >= deadline {
            return cancel_handoff(&plan, "restart helper was not assigned the lock".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let request = crate::messaging::control::Request {
        id: format!("restart-quit-{}", std::process::id()),
        method: "bus.quit".into(),
        params: serde_json::json!({"restart":{"helper_pid":std::process::id(), "plan":plan}}),
    };
    let quit = crate::messaging::control::request(root, &request).and_then(|response| {
        if response.ok {
            Ok(())
        } else {
            Err(format!("{:?}", response.error))
        }
    });
    if let Err(error) = quit {
        return cancel_handoff(&plan, error);
    }
    let deadline = Instant::now() + QUIT_TIMEOUT;
    loop {
        if matches_plan(&root.join("restarts/active.json"), &plan) {
            return Ok(());
        }
        if !crate::platform::process_still_live(plan.tui.pid, plan.tui.birth)
            || Instant::now() >= deadline
        {
            let error_path = root.join("restarts/handoff-error.json");
            let reason = std::fs::read(&error_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .filter(|error| error["id"].as_str() == Some(plan.id.as_str()))
                .and_then(|error| error["reason"].as_str().map(str::to_owned))
                .unwrap_or_else(|| {
                    "TUI did not finish saving and claiming the relaunch; inspect herdr-client.log"
                        .into()
                });
            return cancel_handoff(&plan, reason);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn matches_plan(path: &Path, plan: &HandoffPlan) -> bool {
    restart_record::read_handoff(path)
        .ok()
        .flatten()
        .is_some_and(|seen| seen.id == plan.id)
}

fn cancel_handoff(plan: &HandoffPlan, mut reason: String) -> io::Result<()> {
    let root = &plan.root;
    if matches_plan(&root.join("restarts/active.json"), plan) {
        return Ok(());
    }
    let cancelled = root.join("restarts/cancelled.json");
    if let Err(error) = std::fs::rename(restart_record::handoff_path(root), &cancelled) {
        if matches_plan(&root.join("restarts/active.json"), plan) {
            return Ok(());
        }
        reason.push_str(&format!("; could not claim cancelled handoff: {error}"));
        let _ = restart_record::write_json(&cancelled, plan);
    }
    let outcome = if crate::platform::process_still_live(plan.tui.pid, plan.tui.birth) {
        "quit_failed"
    } else {
        "relaunch_failed"
    };
    tracing::error!(%reason, outcome, restart_id = %plan.id, "restart handoff failed");
    let mut effects = LiveHandoff::new(root);
    super::finish(plan, &mut effects, outcome, &plan.old_binary, reason);
    prep::release_restart_lock(root);
    let _ = std::fs::remove_file(restart_record::handoff_pid_path(root));
    Ok(())
}

/// Normal quit has saved the UI and stopped the coordinator/server. No request
/// is read here: the internal quit event already validated and retained it.
pub(crate) fn resume_after_quit() -> io::Result<()> {
    let Some(plan) = crate::messaging::coordinator::take_restart_plan() else {
        return Ok(());
    };
    let root = &plan.root;
    let cancelled = root.join("restarts/cancelled.json");
    if matches_plan(&cancelled, &plan) {
        return Ok(());
    }
    let active = root.join("restarts/active.json");
    if let Err(error) = std::fs::rename(restart_record::handoff_path(root), &active) {
        if error.kind() == io::ErrorKind::NotFound && matches_plan(&cancelled, &plan) {
            return Ok(());
        }
        let reason = format!("could not claim the relaunch after quit: {error}");
        tracing::error!(%reason, restart_id = %plan.id, "restart terminal handoff failed");
        let _ = restart_record::write_json(
            &root.join("restarts/handoff-error.json"),
            &serde_json::json!({"id":plan.id,"reason":reason}),
        );
        return Err(io::Error::other(reason));
    }
    let mut effects = LiveHandoff::new(root);
    if let Err(reason) = prep::assign_restart_lock(root, std::process::id()) {
        tracing::error!(%reason, "restart terminal lock assignment failed");
        super::finish(
            &plan,
            &mut effects,
            "relaunch_failed",
            &plan.old_binary,
            reason.clone(),
        );
        return Err(io::Error::other(reason));
    }
    let outcome = super::execute_handoff(&plan, &mut effects);
    prep::release_restart_lock(root);
    let _ = std::fs::remove_file(restart_record::handoff_pid_path(root));
    // Keep active.json as the bounded acknowledgement of this attempt, so a
    // delayed helper cannot mistake completed relaunch for a vanished request.
    if let Some(mut child) = effects.child.take() {
        let status = child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("relaunched Bus exited {status}")));
        }
    }
    if matches!(
        outcome.outcome.as_str(),
        "rollback_failed" | "relaunch_failed"
    ) || restart_record::persist_error_path(root).exists()
    {
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
                let startup =
                    std::fs::read_to_string(crate::utils::paths::server_startup_error_path())
                        .unwrap_or_default();
                return Err(format!(
                    "TUI process {pid} exited {status} before becoming healthy{}",
                    if startup.trim().is_empty() {
                        String::new()
                    } else {
                        format!("; server startup failed: {}", startup.trim())
                    }
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
    fn post_quit_missing_request_returns_error_and_records_the_original_cause() {
        let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
        let root = crate::utils::test_temp::unique_temp_path("restart-missing-claim");
        std::fs::create_dir_all(root.join("restarts")).unwrap();
        let plan = super::super::tests::plan(&root);
        crate::messaging::coordinator::test_restart_plan(plan.clone());
        let error = resume_after_quit().unwrap_err().to_string();
        assert!(
            error.contains("could not claim the relaunch after quit"),
            "{error}"
        );
        let diagnostic: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join("restarts/handoff-error.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(diagnostic["reason"], error);
        cancel_handoff(&plan, error.clone()).unwrap();
        let outcome = restart_record::read_outcome(&restart_record::outcome_path(&root))
            .unwrap()
            .unwrap();
        assert_eq!(outcome.outcome, "relaunch_failed");
        assert!(outcome.reason.contains(&error));
        assert!(outcome.reason.contains("/old/bus"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_quit_lock_error_persists_a_failed_relaunch_instead_of_success() {
        let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
        let root = crate::utils::test_temp::unique_temp_path("restart-lock-error");
        let plan = super::super::tests::plan(&root);
        restart_record::write_json(&restart_record::handoff_path(&root), &plan).unwrap();
        std::fs::create_dir(restart_record::restart_lock_path(&root)).unwrap();
        crate::messaging::coordinator::test_restart_plan(plan);
        let error = resume_after_quit().unwrap_err().to_string();
        let outcome = restart_record::read_outcome(&restart_record::outcome_path(&root))
            .unwrap()
            .unwrap();
        assert_eq!(outcome.outcome, "relaunch_failed");
        assert!(outcome.reason.contains(&error));
        assert!(outcome.reason.contains("finish manually"));
        std::fs::remove_dir_all(root).unwrap();
    }

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
