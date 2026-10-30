//! `bus --dev restart`: an orchestrator replaces the running server with another binary.
//!
//! The public command preflights and spawns a detached helper, then returns.
//! The helper outlives the server it stops. It starts the new binary on the
//! same session and rolls back to the old binary when that server never
//! becomes healthy.
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::session_pick::RestartRequest;
use crate::utils::restart_record::{self, Identity, RestartLog, RestartMarker, MIN_FREE_BYTES};

#[path = "restart_prep.rs"]
mod prep;

const CONFIRM: &str = "This command changes or removes work; add --confirm to proceed";
pub(crate) const HEALTH_TIMEOUT: Duration = Duration::from_secs(45);
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(10);
const COORDINATOR_WAIT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct HandoffPlan {
    pub(crate) root: PathBuf,
    pub(crate) old_binary: PathBuf,
    pub(crate) new_binary: PathBuf,
    pub(crate) old_sha: String,
    pub(crate) new_sha: String,
    pub(crate) who: String,
    pub(crate) session_id: Option<String>,
    pub(crate) explicit_data_dir: bool,
    pub(crate) dev: bool,
    pub(crate) yolo: bool,
}

pub(crate) trait HandoffIo {
    fn stop(&mut self) -> Result<(), String>;
    fn start(&mut self, binary: &Path) -> Result<u32, String>;
    fn healthy(&mut self, pid: u32, expected_sha: &str) -> Result<(), String>;
    fn kill(&mut self, pid: u32);
    fn control_socket_up(&mut self) -> bool;
    fn spawn_coordinator(&mut self, binary: &Path) -> Result<(), String>;
    fn now_ms(&self) -> u64;
}

pub(super) fn run(
    root: &Path,
    session_id: Option<&str>,
    explicit_data_dir: bool,
    dev: bool,
    yolo: bool,
    request: &RestartRequest,
) -> io::Result<()> {
    if !dev {
        return Err(io::Error::other(
            "dev tool: run `bus --dev restart --binary PATH --confirm`",
        ));
    }
    if !request.confirm {
        return Err(io::Error::other(CONFIRM));
    }
    let Some(binary) = request.binary.as_deref() else {
        return Err(io::Error::other(
            "Usage: bus --dev [--yolo] restart --binary <path> --confirm [--as <who>]",
        ));
    };
    let new_binary = PathBuf::from(binary);
    let owner = crate::platform::process_instance(std::process::id())
        .ok_or_else(|| io::Error::other("could not read this process"))?;
    if let Err(error) = prep::acquire_restart_lock(
        root,
        &restart_record::LivePid {
            pid: owner.pid,
            birth: owner.birth,
        },
    ) {
        return Err(prep::record_refused(
            root,
            &prep::refused_record(
                request,
                &new_binary,
                Path::new(""),
                "unknown",
                "unknown",
                &error,
            ),
        ));
    }
    match prep::begin_handoff(
        root,
        session_id,
        explicit_data_dir,
        dev,
        yolo,
        request,
        &new_binary,
    ) {
        Ok(()) => Ok(()),
        Err(error) => {
            prep::release_restart_lock(root);
            let _ = std::fs::remove_file(restart_record::handoff_path(root));
            Err(error)
        }
    }
}

pub(crate) fn preflight_binary(
    binary: &Path,
    free: Result<u64, String>,
) -> Result<Identity, String> {
    if !binary.is_absolute() {
        return Err(format!(
            "binary path must be absolute: {}",
            binary.display()
        ));
    }
    if !binary.is_file() {
        return Err(format!("binary does not exist: {}", binary.display()));
    }
    if !is_executable(binary) {
        return Err(format!("binary is not executable: {}", binary.display()));
    }
    let stdout = run_identity(binary)?;
    let identity = restart_record::parse_identity(&stdout)?;
    let free = free?;
    if free < MIN_FREE_BYTES {
        return Err(format!(
            "not enough free disk for a restart: {} GB free, need 5 GB",
            free / 1_000_000_000
        ));
    }
    Ok(identity)
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn run_identity(binary: &Path) -> Result<String, String> {
    let mut child = Command::new(binary)
        .arg("--identity")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run {}: {error}", binary.display()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|error| format!("could not read identity: {error}"))?;
                if !status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(format!(
                        "{} --identity exited {}: {}",
                        binary.display(),
                        status,
                        stderr.trim()
                    ));
                }
                return String::from_utf8(output.stdout)
                    .map_err(|_| "identity output is not UTF-8".to_owned());
            }
            Ok(None) if started.elapsed() > IDENTITY_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} --identity timed out", binary.display()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => return Err(format!("could not wait for identity: {error}")),
        }
    }
}

fn host_free_bytes(root: &Path) -> Result<u64, String> {
    crate::utils::disk_space::available_bytes(root)
        .map_err(|error| format!("could not read free disk space: {error}"))
}

fn spawn_helper() -> Result<u32, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("could not find this bus binary: {error}"))?;
    let mut command = Command::new(exe);
    command
        .arg("--restart-handoff")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);
    spawn_detached(&mut command)
        .map_err(|error| format!("could not start the restart helper: {error}"))
}

pub(crate) fn run_handoff_entry() -> io::Result<()> {
    let root = crate::utils::env::bus_data_dir()
        .ok_or_else(|| io::Error::other("BUS_DATA_DIR is required for a restart handoff"))?;
    let _ = crate::utils::logging::init_file_logging_at(
        root.clone(),
        "herdr-server.log",
        &super::logging_options(),
    );
    let bytes = std::fs::read(restart_record::handoff_path(&root)).map_err(|error| {
        io::Error::other(format!("restart handoff request is missing: {error}"))
    })?;
    let plan: HandoffPlan = serde_json::from_slice(&bytes).map_err(|error| {
        io::Error::other(format!("restart handoff request is invalid: {error}"))
    })?;
    let _ = prep::assign_restart_lock(&root, std::process::id());
    if let Some(live) = crate::platform::process_instance(std::process::id()) {
        let _ = restart_record::write_json(
            &restart_record::handoff_pid_path(&root),
            &restart_record::LivePid {
                pid: live.pid,
                birth: live.birth,
            },
        );
    }
    let mut effects = LiveHandoff;
    execute_handoff(&plan, &mut effects);
    prep::release_restart_lock(&root);
    let _ = std::fs::remove_file(restart_record::handoff_path(&root));
    let _ = std::fs::remove_file(restart_record::handoff_pid_path(&root));
    let persist = restart_record::persist_error_path(&root);
    if persist.exists() {
        let detail = std::fs::read_to_string(&persist).unwrap_or_default();
        return Err(io::Error::other(format!(
            "restart record was not stored: {detail}"
        )));
    }
    Ok(())
}

pub(crate) fn run_session_coordinator() -> io::Result<()> {
    let root = crate::utils::env::bus_data_dir()
        .ok_or_else(|| io::Error::other("BUS_DATA_DIR is required for a session coordinator"))?;
    let handle = crate::messaging::coordinator::BusHandle::start(
        root.clone(),
        crate::protocol::api::client::ConnectionTarget::LocalSession(Some(
            crate::messaging::coordinator::DEFAULT_SESSION.into(),
        )),
    )
    .map_err(io::Error::other)?;
    if let Some(live) = crate::platform::process_instance(std::process::id()) {
        let _ = restart_record::write_json(
            &restart_record::coordinator_pid_path(&root),
            &restart_record::LivePid {
                pid: live.pid,
                birth: live.birth,
            },
        );
    }
    park_until_terminate();
    drop(handle);
    if let Some(record) =
        restart_record::read_live_pid(&restart_record::coordinator_pid_path(&root))
    {
        if record.pid == std::process::id() {
            let _ = std::fs::remove_file(restart_record::coordinator_pid_path(&root));
        }
    }
    Ok(())
}

fn park_until_terminate() {
    let (sender, receiver) = std::sync::mpsc::channel();
    if ctrlc::set_handler(move || {
        let _ = sender.send(());
    })
    .is_err()
    {
        // A handler can be installed only once. Stay up so this process keeps
        // the coordinator lock; the default SIGTERM still ends the process.
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }
    let _ = receiver.recv();
}

/// Stop, swap, and record. Injected effects keep the tests off a real server.
pub(crate) fn execute_handoff(plan: &HandoffPlan, effects: &mut impl HandoffIo) -> RestartLog {
    let _ = restart_record::write_json(
        &restart_record::marker_path(&plan.root),
        &marker_for(plan, &plan.new_binary),
    );
    if let Err(error) = effects.stop() {
        return finish(
            plan,
            effects,
            "rolled_back",
            &plan.old_binary,
            format!("could not stop the server: {error}"),
        );
    }
    match effects.start(&plan.new_binary) {
        Ok(pid) => match effects.healthy(pid, &plan.new_sha) {
            Ok(()) => match ensure_coordinator(effects, &plan.new_binary) {
                Ok(()) => finish(plan, effects, "restarted", &plan.new_binary, String::new()),
                Err(error) => rollback(plan, effects, pid, format!("coordinator: {error}")),
            },
            Err(reason) => rollback(plan, effects, pid, reason),
        },
        Err(reason) => rollback(plan, effects, 0, reason),
    }
}

fn rollback(
    plan: &HandoffPlan,
    effects: &mut impl HandoffIo,
    failed_pid: u32,
    reason: String,
) -> RestartLog {
    if failed_pid != 0 {
        effects.kill(failed_pid);
    }
    let _ = effects.stop();
    match effects.start(&plan.old_binary) {
        Ok(pid) => match effects.healthy(pid, &plan.old_sha) {
            Ok(()) => finish(plan, effects, "rolled_back", &plan.old_binary, reason),
            Err(rollback_reason) => {
                effects.kill(pid);
                finish(
                    plan,
                    effects,
                    "rollback_failed",
                    &plan.old_binary,
                    format!("{reason}; rollback failed: {rollback_reason}"),
                )
            }
        },
        Err(rollback_reason) => finish(
            plan,
            effects,
            "rollback_failed",
            &plan.old_binary,
            format!("{reason}; rollback failed: {rollback_reason}"),
        ),
    }
}

fn ensure_coordinator(effects: &mut impl HandoffIo, binary: &Path) -> Result<(), String> {
    if effects.control_socket_up() {
        return Ok(());
    }
    match effects.spawn_coordinator(binary) {
        Ok(()) => Ok(()),
        Err(error)
            if error.contains("Another Bus coordinator owns this data directory")
                && effects.control_socket_up() =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn finish(
    plan: &HandoffPlan,
    effects: &mut impl HandoffIo,
    outcome: &str,
    serving: &Path,
    mut reason: String,
) -> RestartLog {
    if matches!(outcome, "restarted" | "rolled_back" | "rollback_failed") {
        if let Err(error) = ensure_coordinator(effects, serving) {
            if outcome != "restarted" {
                reason = if reason.is_empty() {
                    format!("coordinator: {error}")
                } else {
                    format!("{reason}; coordinator: {error}")
                };
            }
        }
    }
    let _ = restart_record::write_json(
        &restart_record::marker_path(&plan.root),
        &marker_for(plan, serving),
    );
    let record = RestartLog {
        at_ms: effects.now_ms(),
        who: plan.who.clone(),
        old_sha: plan.old_sha.clone(),
        new_sha: plan.new_sha.clone(),
        old_binary: plan.old_binary.display().to_string(),
        new_binary: plan.new_binary.display().to_string(),
        outcome: outcome.to_owned(),
        reason,
    };
    if let Err(error) = persist_restart_record(&plan.root, &record) {
        tracing::error!(%error, "restart attempt was not stored");
    }
    let _ = std::fs::remove_file(restart_record::marker_path(&plan.root));
    record
}

fn persist_restart_record(root: &Path, record: &RestartLog) -> Result<(), String> {
    let log_result = restart_record::append_log(&restart_record::log_path(root), record);
    let outcome_result = restart_record::write_json(&restart_record::outcome_path(root), record);
    if log_result.is_ok() && outcome_result.is_ok() {
        let _ = std::fs::remove_file(restart_record::persist_error_path(root));
        return Ok(());
    }
    let body = format!(
        "restart attempt was not stored\nlog: {log_result:?}\noutcome: {outcome_result:?}\nrecord: {}\n",
        serde_json::to_string(record).unwrap_or_default()
    );
    let path = restart_record::persist_error_path(root);
    let wrote = std::fs::write(&path, &body).map_err(|error| error.to_string());
    let primary = log_result
        .err()
        .or(outcome_result.err())
        .unwrap_or_default();
    match wrote {
        Ok(()) => Err(primary),
        Err(error) => Err(format!(
            "{primary}; could not write {}: {error}",
            path.display()
        )),
    }
}

fn marker_for(plan: &HandoffPlan, binary: &Path) -> RestartMarker {
    RestartMarker {
        binary: binary.display().to_string(),
        session_id: plan.session_id.clone(),
        explicit_data_dir: plan.explicit_data_dir,
        data_dir: plan.root.display().to_string(),
        dev: plan.dev,
        yolo: plan.yolo,
    }
}

/// A stop that finds no server is success. A running server must actually exit.
pub(crate) fn stop_if_listening(
    listening: bool,
    stop: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !listening {
        return Ok(());
    }
    stop()
}

struct LiveHandoff;

impl HandoffIo for LiveHandoff {
    fn stop(&mut self) -> Result<(), String> {
        stop_if_listening(
            super::launch::is_server_listening(),
            super::stop::stop_active_server,
        )
    }

    fn start(&mut self, binary: &Path) -> Result<u32, String> {
        let _ = std::fs::remove_file(crate::utils::paths::server_startup_error_path());
        let mut command = server_command(binary);
        command.arg("server");
        spawn_detached(&mut command).map_err(|error| {
            format!(
                "could not start {binary}: {error}",
                binary = binary.display()
            )
        })
    }

    fn healthy(&mut self, pid: u32, expected_sha: &str) -> Result<(), String> {
        let root = crate::utils::env::bus_data_dir()
            .ok_or_else(|| "BUS_DATA_DIR is required".to_owned())?;
        let deadline = Instant::now() + HEALTH_TIMEOUT;
        loop {
            if crate::platform::process_instance(pid).is_none() {
                let startup =
                    std::fs::read_to_string(crate::utils::paths::server_startup_error_path())
                        .unwrap_or_default();
                return Err(if startup.trim().is_empty() {
                    format!("server process {pid} exited")
                } else {
                    startup.trim().to_owned()
                });
            }
            let sha = restart_record::read_build_sha(&restart_record::build_path(&root));
            if super::launch::is_server_listening() && sha.as_deref() == Some(expected_sha) {
                return Ok(());
            }
            if let Some(error) =
                std::fs::read_to_string(crate::utils::paths::server_startup_error_path())
                    .ok()
                    .filter(|error| !error.trim().is_empty())
            {
                return Err(error.trim().to_owned());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "server did not become healthy within {}s",
                    HEALTH_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill(&mut self, pid: u32) {
        kill_pid(pid);
    }

    fn control_socket_up(&mut self) -> bool {
        let Some(root) = crate::utils::env::bus_data_dir() else {
            return false;
        };
        crate::platform::ipc::connect_local_stream(&root.join("control.sock")).is_ok()
    }

    fn spawn_coordinator(&mut self, binary: &Path) -> Result<(), String> {
        let mut command = server_command(binary);
        command.arg("--session-coordinator");
        let pid = spawn_detached(&mut command)
            .map_err(|error| format!("could not start the coordinator: {error}"))?;
        let deadline = Instant::now() + COORDINATOR_WAIT;
        while Instant::now() < deadline {
            if self.control_socket_up() {
                return Ok(());
            }
            if crate::platform::process_instance(pid).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.control_socket_up() {
            return Ok(());
        }
        Err("headless coordinator did not open control.sock".to_owned())
    }

    fn now_ms(&self) -> u64 {
        crate::utils::time::now_ms()
    }
}

fn server_command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command
        .env_remove(crate::utils::env::STOP_WITHOUT_CLIENTS_ENV_VAR)
        .env_remove(crate::utils::env::SOCKET_PATH_ENV_VAR)
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env(
            "HERDR_SESSION",
            crate::messaging::coordinator::DEFAULT_SESSION,
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(cwd) = std::env::current_dir() {
        command.env(crate::utils::env::STARTUP_CWD_ENV_VAR, cwd);
    }
    detach(&mut command);
    command
}

fn detach(command: &mut Command) {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    crate::platform::detach_server_daemon_command(command);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = command;
}

fn spawn_detached(command: &mut Command) -> io::Result<u32> {
    #[cfg(not(windows))]
    {
        crate::platform::launch_server_daemon_command(command)
    }
    #[cfg(windows)]
    {
        command.spawn().map(|child| child.id())
    }
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && crate::platform::process_instance(pid).is_some() {
            std::thread::sleep(Duration::from_millis(20));
        }
        if crate::platform::process_instance(pid).is_some() {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn sha(byte: u8) -> String {
        format!("{byte:02x}").repeat(20)
    }

    fn plan(root: &Path) -> HandoffPlan {
        HandoffPlan {
            root: root.to_path_buf(),
            old_binary: PathBuf::from("/old/bus"),
            new_binary: PathBuf::from("/new/bus"),
            old_sha: sha(0x11),
            new_sha: sha(0x22),
            who: "agent:9 user:ada".into(),
            session_id: Some("abc".into()),
            explicit_data_dir: false,
            dev: true,
            yolo: false,
        }
    }

    struct Fake {
        stop: Result<(), String>,
        starts: VecDeque<Result<u32, String>>,
        started: Vec<PathBuf>,
        healthy_shas: Vec<String>,
        health_error: String,
        killed: Vec<u32>,
        control_up: bool,
        coordinator_error: Option<String>,
        coordinators: Vec<PathBuf>,
        now: u64,
    }

    impl HandoffIo for Fake {
        fn stop(&mut self) -> Result<(), String> {
            self.stop.clone()
        }
        fn start(&mut self, binary: &Path) -> Result<u32, String> {
            self.started.push(binary.to_path_buf());
            self.starts
                .pop_front()
                .unwrap_or_else(|| Err("no start scripted".into()))
        }
        fn healthy(&mut self, _pid: u32, expected_sha: &str) -> Result<(), String> {
            if self.healthy_shas.iter().any(|sha| sha == expected_sha) {
                Ok(())
            } else {
                Err(self.health_error.clone())
            }
        }
        fn kill(&mut self, pid: u32) {
            self.killed.push(pid);
        }
        fn control_socket_up(&mut self) -> bool {
            self.control_up
        }
        fn spawn_coordinator(&mut self, binary: &Path) -> Result<(), String> {
            self.coordinators.push(binary.to_path_buf());
            if let Some(error) = self.coordinator_error.clone() {
                if error.contains("Another Bus coordinator owns this data directory") {
                    self.control_up = true;
                }
                return Err(error);
            }
            self.control_up = true;
            Ok(())
        }
        fn now_ms(&self) -> u64 {
            self.now
        }
    }

    fn fake() -> Fake {
        Fake {
            stop: Ok(()),
            starts: VecDeque::new(),
            started: Vec::new(),
            healthy_shas: Vec::new(),
            health_error: "socket timeout".into(),
            killed: Vec::new(),
            control_up: false,
            coordinator_error: None,
            coordinators: Vec::new(),
            now: 42,
        }
    }

    #[test]
    fn handoff_restarts_onto_the_new_binary_and_records_it() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-ok");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.healthy_shas.push(plan.new_sha.clone());
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "restarted");
        assert_eq!(record.old_sha, plan.old_sha);
        assert_eq!(record.new_sha, plan.new_sha);
        assert_eq!(record.who, plan.who);
        assert!(effects.killed.is_empty());
        assert_eq!(effects.coordinators, vec![plan.new_binary.clone()]);
        assert!(!restart_record::marker_path(&root).exists());
        let logged = std::fs::read_to_string(restart_record::log_path(&root)).unwrap();
        assert!(logged.contains("\"outcome\":\"restarted\""));
        let outcome = restart_record::read_outcome(&restart_record::outcome_path(&root))
            .unwrap()
            .unwrap();
        assert_eq!(outcome, record);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn handoff_rolls_back_when_the_new_server_is_unhealthy() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-back");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.starts.push_back(Ok(8));
        effects.healthy_shas.push(plan.old_sha.clone());
        effects.control_up = true;
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rolled_back");
        assert!(record.reason.contains("socket timeout"));
        assert_eq!(effects.killed, vec![7]);
        assert_eq!(
            effects.started,
            vec![plan.new_binary.clone(), plan.old_binary.clone()]
        );
        assert!(
            effects.coordinators.is_empty(),
            "the old client still owns the coordinator"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn handoff_records_both_failures_when_rollback_also_fails() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-both");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Err("new binary missing".into()));
        effects.starts.push_back(Err("old binary missing".into()));
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rollback_failed");
        assert!(record.reason.contains("new binary missing"));
        assert!(record
            .reason
            .contains("rollback failed: old binary missing"));
        assert_eq!(effects.coordinators, vec![plan.old_binary.clone()]);
        let notice = restart_record::notice_text(&record);
        assert!(!notice.contains("rolled back"), "{notice}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_unhealthy_rollback_server_is_stopped_and_not_described_as_serving() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-unhealthy-back");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.starts.push_back(Ok(8));
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rollback_failed");
        assert!(effects.killed.contains(&8), "{:?}", effects.killed);
        let notice = restart_record::notice_text(&record);
        assert!(!notice.contains("rolled back to"), "{notice}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn handoff_rejects_success_without_messaging_coordinator() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-no-coord");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.starts.push_back(Ok(8));
        effects.healthy_shas.push(plan.new_sha.clone());
        effects.healthy_shas.push(plan.old_sha.clone());
        effects.coordinator_error = Some("coordinator could not start".into());
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rolled_back");
        assert_eq!(effects.killed, vec![7]);
        assert_eq!(
            effects.started,
            vec![plan.new_binary.clone(), plan.old_binary.clone()]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn failed_rollback_has_distinct_outcome() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-distinct");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Err("new binary missing".into()));
        effects.starts.push_back(Err("old binary missing".into()));
        let record = execute_handoff(&plan, &mut effects);
        assert_ne!(record.outcome, "rolled_back");
        let notice = restart_record::notice_text(&record);
        assert!(!notice.contains("rolled back"), "{notice}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn preflight_refuses_a_binary_path_that_is_not_absolute() {
        let error = preflight_binary(Path::new("bus"), Ok(MIN_FREE_BYTES)).unwrap_err();
        assert!(error.contains("absolute"), "{error}");
    }

    #[test]
    fn restart_refuses_without_a_recorded_serving_binary() {
        let root = crate::utils::test_temp::unique_temp_path("serving-missing");
        std::fs::create_dir_all(&root).unwrap();
        let missing = super::prep::serving_target(&root).unwrap_err();
        assert!(missing.contains("no recorded binary"), "{missing}");
        let sha = sha(0x11);
        let gone = root.join("gone");
        restart_record::write_json(
            &restart_record::build_path(&root),
            &serde_json::json!({ "sha": sha, "binary": gone.display().to_string() }),
        )
        .unwrap();
        let deleted = super::prep::serving_target(&root).unwrap_err();
        assert!(deleted.contains("no recorded binary"), "{deleted}");
        std::fs::write(&gone, b"bin").unwrap();
        restart_record::write_json(
            &restart_record::build_path(&root),
            &serde_json::json!({ "sha": "short", "binary": gone.display().to_string() }),
        )
        .unwrap();
        let bad_sha = super::prep::serving_target(&root).unwrap_err();
        assert!(bad_sha.contains("no recorded build SHA"), "{bad_sha}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_attempt_log_is_durable() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-log-dir");
        std::fs::create_dir_all(restart_record::log_path(&root)).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.healthy_shas.push(plan.new_sha.clone());
        effects.control_up = true;
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "restarted");
        let saved = std::fs::read_to_string(restart_record::persist_error_path(&root)).unwrap();
        assert!(saved.contains("restarted"), "{saved}");
        assert!(
            saved.contains("not stored") || saved.contains("log:"),
            "{saved}"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_refused_identity_names_the_serving_sha_and_binary_path() {
        let root = crate::utils::test_temp::unique_temp_path("restart-refuse-sha");
        std::fs::create_dir_all(&root).unwrap();
        let serving = root.join("serving");
        std::fs::write(&serving, b"bus").unwrap();
        let recorded = sha(0x11);
        restart_record::write_json(
            &restart_record::build_path(&root),
            &serde_json::json!({ "sha": recorded, "binary": serving.display().to_string() }),
        )
        .unwrap();
        let refuse = root.join("refuse");
        std::fs::write(
            &refuse,
            "#!/bin/sh\necho preflight identity refused >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&refuse, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let error = run(
            &root,
            None,
            true,
            true,
            false,
            &super::super::session_pick::RestartRequest {
                binary: Some(refuse.display().to_string()),
                confirm: true,
                as_who: None,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("--identity"), "{error}");
        let log = std::fs::read_to_string(restart_record::log_path(&root)).unwrap();
        let record: RestartLog = serde_json::from_str(log.lines().next().unwrap()).unwrap();
        assert_eq!(record.outcome, "refused");
        assert_eq!(record.old_sha, recorded);
        assert_eq!(record.new_binary, refuse.display().to_string());
        let notice = restart_record::notice_text(&record);
        assert!(
            notice.contains(&format!("still serving {recorded}")),
            "{notice}"
        );
        assert!(
            notice.contains(&format!("{} was not started", refuse.display())),
            "{notice}"
        );
        assert!(!notice.contains("not serving"), "{notice}");
        assert!(!restart_record::restart_lock_path(&root).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_refused_restart_keeps_the_log_error() {
        let root = crate::utils::test_temp::unique_temp_path("restart-refuse-log");
        std::fs::create_dir_all(restart_record::log_path(&root)).unwrap();
        let error = run(
            &root,
            None,
            true,
            true,
            false,
            &super::super::session_pick::RestartRequest {
                binary: Some("bus".into()),
                confirm: true,
                as_who: None,
            },
        )
        .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("absolute"), "{text}");
        assert!(text.contains("log.jsonl"), "{text}");
        assert!(!restart_record::restart_lock_path(&root).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn restart_lock_admits_only_one_live_owner() {
        let root = crate::utils::test_temp::unique_temp_path("restart-lock");
        std::fs::create_dir_all(&root).unwrap();
        let owner = crate::platform::process_instance(std::process::id()).unwrap();
        let live = restart_record::LivePid {
            pid: owner.pid,
            birth: owner.birth,
        };
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let live = live.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                super::prep::acquire_restart_lock(&root, &live)
            }));
        }
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(results.iter().any(|result| {
            result
                .as_ref()
                .is_err_and(|error| error.contains("already in progress"))
        }));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_stale_restart_lock_can_be_replaced() {
        let root = crate::utils::test_temp::unique_temp_path("restart-stale-lock");
        std::fs::create_dir_all(&root).unwrap();
        restart_record::write_json(
            &restart_record::restart_lock_path(&root),
            &restart_record::LivePid {
                pid: u32::MAX,
                birth: 1,
            },
        )
        .unwrap();
        let owner = crate::platform::process_instance(std::process::id()).unwrap();
        super::prep::acquire_restart_lock(
            &root,
            &restart_record::LivePid {
                pid: owner.pid,
                birth: owner.birth,
            },
        )
        .unwrap();
        let held =
            restart_record::read_live_pid(&restart_record::restart_lock_path(&root)).unwrap();
        assert_eq!(held.pid, owner.pid);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_stop_does_not_start_a_second_server() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-stop");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.stop = Err("did not stop within 15000ms".into());
        effects.control_up = true;
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rolled_back");
        assert!(record.reason.contains("could not stop"));
        assert!(effects.started.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_existing_coordinator_is_not_a_failure() {
        let root = crate::utils::test_temp::unique_temp_path("handoff-coord");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(4));
        effects.healthy_shas.push(plan.new_sha.clone());
        effects.coordinator_error =
            Some("Another Bus coordinator owns this data directory, or it is inaccessible".into());
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "restarted");
        assert!(record.reason.is_empty(), "{}", record.reason);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn not_listening_counts_as_stopped() {
        assert!(stop_if_listening(false, || Err("nope".into())).is_ok());
        assert!(stop_if_listening(true, || Err("timeout".into())).is_err());
    }

    #[test]
    fn preflight_refuses_confirmation_disk_and_identity() {
        let root = crate::utils::test_temp::unique_temp_path("preflight");
        std::fs::create_dir_all(&root).unwrap();
        let missing = root.join("missing");
        assert!(preflight_binary(&missing, Ok(MIN_FREE_BYTES))
            .unwrap_err()
            .contains("does not exist"));
        let binary = root.join("bus");
        std::fs::write(&binary, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(preflight_binary(&binary, Ok(MIN_FREE_BYTES))
                .unwrap_err()
                .contains("not executable"));
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let sha = sha(0xab);
        std::fs::write(
            &binary,
            format!("#!/bin/sh\nprintf '%s\\n' '{{\"sha\":\"{sha}\",\"providers\":[\"claude\",\"grok\"]}}'\n"),
        )
        .unwrap();
        assert!(preflight_binary(&binary, Ok(MIN_FREE_BYTES - 1))
            .unwrap_err()
            .contains("not enough free disk"));
        assert!(preflight_binary(&binary, Err("statvfs failed".into()))
            .unwrap_err()
            .contains("statvfs failed"));
        let identity = preflight_binary(&binary, Ok(MIN_FREE_BYTES)).unwrap();
        assert_eq!(identity.sha, sha);
        assert_eq!(identity.providers, ["claude", "grok"]);
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s\\n' '{\"sha\":\"unknown\",\"providers\":[\"claude\"]}'\n",
        )
        .unwrap();
        assert!(preflight_binary(&binary, Ok(MIN_FREE_BYTES))
            .unwrap_err()
            .contains("40-character"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn restart_help_names_the_pause_protocol() {
        assert!(crate::cli::control::command_tree(false)
            .find_subcommand("restart")
            .is_none());
        let normal = crate::cli::help::agent_help();
        assert!(!normal.contains("bus restart"), "{normal}");
        assert!(!normal.contains("\n  restart"), "{normal}");
        assert!(!crate::cli::session_pick::USAGE.contains("restart"));
        let text = crate::cli::control::command_tree(true)
            .find_subcommand("restart")
            .unwrap()
            .get_long_about()
            .unwrap()
            .to_string();
        assert!(text.contains("pause"), "{text}");
        assert!(text.contains("slot locks"), "{text}");
        assert!(text.contains("--confirm"), "{text}");
        assert!(text.contains("log.jsonl"), "{text}");
        assert!(text.contains("--dev"), "{text}");
        assert!(text.contains("keep this session"), "{text}");
        assert!(
            text.contains("never restarts a server on its own"),
            "{text}"
        );
        assert!(text.contains("not serving"), "{text}");
        assert!(text.contains("refused before any stop"), "{text}");
        assert!(text.contains("still serving"), "{text}");
        assert!(crate::cli::help::DEV_HELP.contains("bus --dev restart"));
    }
}
