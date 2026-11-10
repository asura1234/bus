//! Developer restart: save and quit, then relaunch the whole session in its terminal.
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use super::session_pick::RestartRequest;
use crate::utils::restart_record::{self, Identity, RestartLog, RestartMarker, MIN_FREE_BYTES};

#[path = "restart_build.rs"]
mod build;
#[path = "restart_prep.rs"]
mod prep;
#[path = "restart_session.rs"]
mod session;
pub(crate) use session::{resume_after_quit, run_handoff_entry};

const CONFIRM: &str = "This command changes or removes work; add --confirm to proceed";
pub(crate) const HEALTH_TIMEOUT: Duration = Duration::from_secs(45);
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) use restart_record::HandoffPlan;

pub(crate) trait HandoffIo {
    fn stop(&mut self) -> Result<(), String>;
    fn start(&mut self, binary: &Path, plan: &HandoffPlan) -> Result<u32, String>;
    fn healthy(&mut self, pid: u32, expected_sha: &str) -> Result<(), String>;
    fn kill(&mut self, pid: u32);
    fn now_ms(&self) -> u64;
}

pub(crate) fn record_dev_required_refusal() -> io::Error {
    prep::record_dev_required_refusal(super::session_pick::DEV_RESTART_REQUIRED)
}

pub(super) fn run(
    root: &Path,
    session_id: Option<&str>,
    explicit_data_dir: bool,
    dev: bool,
    yolo: bool,
    request: &RestartRequest,
) -> io::Result<()> {
    run_resolving(
        root,
        session_id,
        explicit_data_dir,
        dev,
        yolo,
        request,
        build::default_source_build,
    )
}

fn run_resolving(
    root: &Path,
    session_id: Option<&str>,
    explicit_data_dir: bool,
    dev: bool,
    yolo: bool,
    request: &RestartRequest,
    build_source: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<()> {
    if !dev {
        return Err(io::Error::other(super::session_pick::DEV_RESTART_REQUIRED));
    }
    if !request.confirm {
        return Err(io::Error::other(CONFIRM));
    }
    // Build before the lock. A failed build must not stop or record a restart.
    let new_binary = match request.binary.as_deref() {
        Some(binary) => PathBuf::from(binary),
        None => build_source()?,
    };
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

fn spawn_helper(plan: &HandoffPlan) -> Result<u32, String> {
    let exe = crate::platform::fs::running_executable()
        .map_err(|error| format!("could not find this bus binary: {error}"))?;
    let mut command = Command::new(exe);
    command
        .arg("--restart-handoff")
        .arg(serde_json::to_string(plan).map_err(|error| error.to_string())?)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    detach(&mut command);
    spawn_detached(&mut command)
        .map_err(|error| format!("could not start the restart helper: {error}"))
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
            "quit_failed",
            &plan.old_binary,
            format!("could not stop the server: {error}"),
        );
    }
    match effects.start(&plan.new_binary, plan) {
        Ok(pid) => match effects.healthy(pid, &plan.new_sha) {
            Ok(()) => finish(plan, effects, "restarted", &plan.new_binary, String::new()),
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
    match effects.start(&plan.old_binary, plan) {
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

fn finish(
    plan: &HandoffPlan,
    effects: &mut impl HandoffIo,
    outcome: &str,
    serving: &Path,
    mut reason: String,
) -> RestartLog {
    // The saved copy is the rollback binary until the new build is healthy.
    if outcome == "restarted" {
        if let Err(error) = prune_saved_binaries(&plan.root) {
            tracing::error!(%error, "could not prune saved restart binaries");
        }
    }
    match outcome {
        "restarted" => {}
        "rolled_back" => reason.push_str("; the previous build is healthy; no additional TUI launch is needed"),
        "quit_failed" => reason.push_str(&format!(
            "; inspect and save/quit the old Bus before retrying restart; if its TUI has exited, resume the previous build with `{}`",
            restart_record::reattach_command(&marker_for(plan, &plan.old_binary))
        )),
        _ => reason.push_str(&format!(
            "; no healthy replacement TUI was confirmed; finish manually: `{}`",
            restart_record::reattach_command(&marker_for(plan, serving))
        )),
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

/// Keep the newest file under `restarts/binaries`. Dotfiles are leftovers of
/// the copy into that directory, so they are removed and cannot win.
fn prune_saved_binaries(root: &Path) -> io::Result<()> {
    let directory = restart_record::restarts_dir(root).join("binaries");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut saved = Vec::new();
    for entry in entries {
        classify_saved_binary(&entry?, &mut saved)?;
    }
    saved.sort_by(newest_saved_first);
    for (_, path) in saved.into_iter().skip(1) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn classify_saved_binary(
    entry: &std::fs::DirEntry,
    saved: &mut Vec<(std::time::SystemTime, PathBuf)>,
) -> io::Result<()> {
    let path = entry.path();
    let dotted = entry.file_name().to_string_lossy().starts_with('.');
    if dotted && path.is_file() {
        std::fs::remove_file(&path)?;
    }
    if dotted {
        return Ok(());
    }
    let meta = entry.metadata()?;
    if meta.is_file() {
        let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        saved.push((modified, path));
    }
    Ok(())
}

fn newest_saved_first(
    left: &(std::time::SystemTime, PathBuf),
    right: &(std::time::SystemTime, PathBuf),
) -> std::cmp::Ordering {
    right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1))
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

/// Socket the replacement server binds: `HERDR_SESSION=bus` and no socket override.
fn restarted_api_socket() -> PathBuf {
    crate::utils::paths::api_socket_path_for(Some(crate::messaging::coordinator::DEFAULT_SESSION))
}

/// `restarted` requires the new server to answer `agent.list`. A socket that
/// accepts and never writes a JSON result is not an answer.
fn server_answered(socket: &Path) -> bool {
    server_answered_within(socket, Duration::from_millis(400))
}

fn server_answered_within(socket: &Path, timeout: Duration) -> bool {
    let client = crate::protocol::api::client::ApiClient::for_target(
        crate::protocol::api::client::ConnectionTarget::SocketPath(socket.to_path_buf()),
    );
    let request = crate::protocol::api::schema::Request {
        id: "restart-health".into(),
        method: crate::protocol::api::schema::Method::AgentList(
            crate::protocol::api::schema::EmptyParams {},
        ),
    };
    matches!(
        client.request_value_with_timeout(&request, timeout),
        Ok(value) if value.get("result").is_some()
    )
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

    pub(super) fn plan(root: &Path) -> HandoffPlan {
        HandoffPlan {
            id: "test-attempt".into(),
            root: root.to_path_buf(),
            old_binary: PathBuf::from("/old/bus"),
            new_binary: PathBuf::from("/new/bus"),
            old_sha: sha(0x11),
            new_sha: sha(0x22),
            who: "agent:9 user:ada".into(),
            tui: restart_record::LivePid { pid: 99, birth: 1 },
            session_id: Some("abc".into()),
            explicit_data_dir: false,
            dev: true,
            yolo: false,
            clear_data: false,
        }
    }

    struct Fake {
        stop: Result<(), String>,
        starts: VecDeque<Result<u32, String>>,
        started: Vec<PathBuf>,
        healthy_shas: Vec<String>,
        health_error: String,
        killed: Vec<u32>,
        now: u64,
    }

    impl HandoffIo for Fake {
        fn stop(&mut self) -> Result<(), String> {
            self.stop.clone()
        }
        fn start(&mut self, binary: &Path, _plan: &HandoffPlan) -> Result<u32, String> {
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
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rolled_back");
        assert!(record.reason.contains("socket timeout"));
        assert!(record.reason.contains("previous build is healthy"));
        assert!(!record.reason.contains("finish manually"));
        assert_eq!(effects.killed, vec![7]);
        assert_eq!(
            effects.started,
            vec![plan.new_binary.clone(), plan.old_binary.clone()]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn quit_failure_keeps_recovery_on_the_old_build_and_requires_inspection_first() {
        let root = crate::utils::test_temp::unique_temp_path("restart-quit-recovery");
        std::fs::create_dir_all(&root).unwrap();
        let plan = plan(&root);
        let record = finish(
            &plan,
            &mut fake(),
            "quit_failed",
            &plan.new_binary,
            "save failed".into(),
        );
        assert!(record.reason.contains("inspect and save/quit the old Bus"));
        assert!(record.reason.contains("if its TUI has exited"));
        assert!(record.reason.contains("/old/bus --dev resume abc"));
        assert!(!record.reason.contains("/new/bus --dev resume"));
        std::fs::remove_dir_all(root).unwrap();
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
        let notice = restart_record::notice_text(&record);
        assert!(notice.is_none());
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
        assert!(notice.is_none());
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
        assert!(notice.is_none());
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
                clear_data: false,
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
        assert!(restart_record::notice_text(&record).is_none());
        assert!(!record.reason.is_empty());
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
                clear_data: false,
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
    fn a_failed_source_build_leaves_the_running_bus_untouched() {
        let root = crate::utils::test_temp::unique_temp_path("restart-build-abort");
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("untouched");
        std::fs::write(&marker, b"stay").unwrap();
        let error = run_resolving(
            &root,
            None,
            true,
            true,
            false,
            &RestartRequest {
                binary: None,
                confirm: true,
                clear_data: false,
                as_who: None,
            },
            || {
                Err(io::Error::other(
                    "cargo build failed; the running Bus was not restarted:\nrustc exploded",
                ))
            },
        )
        .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("cargo build failed"), "{text}");
        assert!(text.contains("was not restarted"), "{text}");
        assert!(text.contains("rustc exploded"), "{text}");
        assert_eq!(std::fs::read(&marker).unwrap(), b"stay");
        assert!(!root.join("restarts").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_explicit_binary_does_not_build_the_source_checkout() {
        let root = crate::utils::test_temp::unique_temp_path("restart-skip-build");
        std::fs::create_dir_all(&root).unwrap();
        let error = run_resolving(
            &root,
            None,
            true,
            true,
            false,
            &RestartRequest {
                binary: Some("bus".into()),
                confirm: true,
                clear_data: false,
                as_who: None,
            },
            || panic!("an explicit --binary must not build"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("absolute"), "{error}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn write_saved(path: &Path, secs: u64) {
        std::fs::write(path, b"bin").unwrap();
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn a_successful_restart_keeps_only_the_newest_saved_binary() {
        let root = crate::utils::test_temp::unique_temp_path("restart-prune");
        let directory = restart_record::restarts_dir(&root).join("binaries");
        std::fs::create_dir_all(&directory).unwrap();
        let older = directory.join("aaaaaaaa");
        let earlier_name = directory.join("bbbbbbbb");
        let kept = directory.join("cccccccc");
        let leftover = directory.join(".123.tmp");
        write_saved(&older, 10);
        write_saved(&earlier_name, 30);
        write_saved(&kept, 30);
        write_saved(&leftover, 40);
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.healthy_shas.push(plan.new_sha.clone());
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "restarted");
        assert!(kept.exists(), "equal mtimes keep the greater path");
        assert!(!older.exists());
        assert!(!earlier_name.exists());
        assert!(!leftover.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_rolled_back_restart_keeps_every_saved_binary() {
        let root = crate::utils::test_temp::unique_temp_path("restart-prune-rollback");
        let directory = restart_record::restarts_dir(&root).join("binaries");
        std::fs::create_dir_all(&directory).unwrap();
        let older = directory.join("older");
        let newer = directory.join("newer");
        write_saved(&older, 10);
        write_saved(&newer, 20);
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.starts.push_back(Ok(8));
        effects.healthy_shas.push(plan.old_sha.clone());
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "rolled_back");
        assert!(older.exists());
        assert!(newer.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_prune_failure_still_records_restarted() {
        let root = crate::utils::test_temp::unique_temp_path("restart-prune-fail");
        let binaries = restart_record::restarts_dir(&root).join("binaries");
        std::fs::create_dir_all(binaries.parent().unwrap()).unwrap();
        std::fs::write(&binaries, b"not a directory").unwrap();
        let plan = plan(&root);
        let mut effects = fake();
        effects.starts.push_back(Ok(7));
        effects.healthy_shas.push(plan.new_sha.clone());
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "restarted");
        assert!(binaries.is_file());
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
        let record = execute_handoff(&plan, &mut effects);
        assert_eq!(record.outcome, "quit_failed");
        assert!(record.reason.contains("could not stop"));
        assert!(effects.started.is_empty());
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
        assert!(
            text.contains("safe metadata after a successful restart"),
            "{text}"
        );
        assert!(
            text.contains("Failure diagnostics go to herdr-client.log"),
            "{text}"
        );
        assert!(text.contains("outcome and reason"), "{text}");
        assert!(text.contains("Omit --yolo to keep"), "{text}");
        assert!(
            text.contains("does not change a recorded --dev mode"),
            "{text}"
        );
        let restart = crate::cli::control::command_tree(true)
            .find_subcommand("restart")
            .unwrap()
            .clone();
        let yolo_help = restart
            .get_arguments()
            .find(|arg| arg.get_id() == "yolo")
            .unwrap()
            .get_help()
            .unwrap()
            .to_string();
        assert!(
            yolo_help.contains("Omit to keep the running server's --yolo setting"),
            "{yolo_help}"
        );
        assert!(crate::cli::help::DEV_HELP.contains("bus --dev restart"));
    }

    #[test]
    fn server_answered_requires_a_json_result() {
        use std::io::{BufRead, Write};
        let root = crate::utils::test_temp::unique_temp_path("restart-answer");
        std::fs::create_dir_all(&root).unwrap();
        let silent = root.join("silent.sock");
        let listener = crate::platform::ipc::bind_local_listener(&silent).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            use interprocess::local_socket::traits::Listener as _;
            let mut stream = listener.accept().unwrap();
            let mut request = String::new();
            std::io::BufReader::new(&mut stream)
                .read_line(&mut request)
                .unwrap();
            ready_tx.send(request).unwrap();
            let _ = done_rx.recv();
        });
        assert!(
            !super::server_answered_within(&silent, Duration::from_millis(80)),
            "an accepted socket that never writes is not an answer"
        );
        let request = ready_rx.recv().unwrap();
        assert!(request.contains("agent.list"), "{request}");
        done_tx.send(()).unwrap();
        server.join().unwrap();

        let speaking = root.join("speak.sock");
        let listener = crate::platform::ipc::bind_local_listener(&speaking).unwrap();
        let server = std::thread::spawn(move || {
            use interprocess::local_socket::traits::Listener as _;
            let mut stream = listener.accept().unwrap();
            let mut request = String::new();
            std::io::BufReader::new(&mut stream)
                .read_line(&mut request)
                .unwrap();
            stream
                .write_all(br#"{"id":"restart-health","result":[]}"#)
                .unwrap();
            stream.write_all(b"\n").unwrap();
            let _ = stream.flush();
        });
        assert!(super::server_answered_within(
            &speaking,
            Duration::from_millis(400)
        ));
        server.join().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
