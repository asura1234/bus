//! Exclusion lock and the preflight that runs before a restart helper exists.
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{restart_record, HandoffPlan, RestartLog, RestartRequest};

const RESTART_BUSY: &str = "a bus restart is already in progress for this session";

pub(super) fn begin_handoff(
    root: &Path,
    session_id: Option<&str>,
    explicit_data_dir: bool,
    dev: bool,
    yolo: bool,
    request: &RestartRequest,
    new_binary: &Path,
) -> io::Result<()> {
    // Read the serving build before identity. A refusal must name the SHA that
    // is still running, not a placeholder, and the path that was not started.
    let serving = serving_target(root);
    let identity =
        super::preflight_binary(new_binary, super::host_free_bytes(root)).map_err(|error| {
            let (old_binary, old_sha) = match &serving {
                Ok((binary, sha)) => (binary.as_path(), sha.as_str()),
                Err(_) => (Path::new(""), "unknown"),
            };
            record_refused(
                root,
                &refused_record(request, new_binary, old_binary, old_sha, "unknown", &error),
            )
        })?;
    let (old_binary, old_sha) = serving.map_err(|error| {
        record_refused(
            root,
            &refused_record(
                request,
                new_binary,
                Path::new(""),
                "unknown",
                &identity.sha,
                &error,
            ),
        )
    })?;
    let recorded = restart_record::read_recorded_launch(&restart_record::build_path(root))
        .map_err(|error| {
            record_refused(
                root,
                &refused_record(
                    request,
                    new_binary,
                    &old_binary,
                    &old_sha,
                    "unknown",
                    &error,
                ),
            )
        })?;
    let status_yolo = if recorded.yolo.is_none() {
        super::super::launch::running_server_yolo()
    } else {
        None
    };
    let launch = restart_record::resolve_restart_launch(
        &recorded,
        status_yolo,
        &restart_record::LaunchOverride {
            yolo: yolo.then_some(true),
            explicit_data_dir,
            cli_dev: dev,
            cli_session_id: session_id.map(str::to_owned),
        },
    )
    .map_err(|error| {
        record_refused(
            root,
            &refused_record(
                request,
                new_binary,
                &old_binary,
                &old_sha,
                "unknown",
                &error,
            ),
        )
    })?;
    let plan = HandoffPlan {
        root: root.to_path_buf(),
        old_binary,
        new_binary: new_binary.to_path_buf(),
        old_sha,
        new_sha: identity.sha,
        who: restart_record::who_label(
            request.as_who.as_deref(),
            std::env::var("USER").ok().as_deref(),
        ),
        session_id: launch.session_id,
        explicit_data_dir: launch.explicit_data_dir,
        dev: launch.dev,
        yolo: launch.yolo,
    };
    if let Err(error) = restart_record::write_json(&restart_record::handoff_path(root), &plan) {
        return Err(record_refused(
            root,
            &refused_record(
                request,
                new_binary,
                &plan.old_binary,
                &plan.old_sha,
                &plan.new_sha,
                &error,
            ),
        ));
    }
    publish_handoff(root, request, &plan)
}

fn publish_handoff(root: &Path, request: &RestartRequest, plan: &HandoffPlan) -> io::Result<()> {
    let new_binary = &plan.new_binary;
    let pid = match super::spawn_helper() {
        Ok(pid) => pid,
        Err(error) => {
            return Err(record_refused(
                root,
                &refused_record(
                    request,
                    new_binary,
                    &plan.old_binary,
                    &plan.old_sha,
                    &plan.new_sha,
                    &error,
                ),
            ));
        }
    };
    if let Err(error) = assign_restart_lock(root, pid) {
        super::kill_pid(pid);
        return Err(record_refused(
            root,
            &refused_record(
                request,
                new_binary,
                &plan.old_binary,
                &plan.old_sha,
                &plan.new_sha,
                &format!("could not publish the restart lock: {error}"),
            ),
        ));
    }
    if let Some(live) = crate::platform::process_instance(pid) {
        let _ = restart_record::write_json(
            &restart_record::handoff_pid_path(root),
            &restart_record::LivePid {
                pid: live.pid,
                birth: live.birth,
            },
        );
    }
    super::super::help::write_stdout_line(format_args!(
        "{}",
        serde_json::json!({
            "ok": true,
            "stage": "handoff",
            "pid": pid,
            "old_sha": plan.old_sha,
            "new_sha": plan.new_sha,
            "session_id": plan.session_id,
        })
    ));
    Ok(())
}

/// The binary and SHA the running server recorded. Never the invoking CLI.
pub(super) fn serving_target(root: &Path) -> Result<(PathBuf, String), String> {
    let path = restart_record::build_path(root);
    let binary = restart_record::read_build_binary(&path).ok_or_else(|| {
        "cannot restart: the running server has no recorded binary to roll back to".to_owned()
    })?;
    let sha = restart_record::read_build_sha(&path)
        .filter(|sha| restart_record::valid_build_sha(sha))
        .ok_or_else(|| {
            "cannot restart: the running server has no recorded build SHA to roll back to"
                .to_owned()
        })?;
    Ok((binary, sha))
}

pub(super) fn refused_record(
    request: &RestartRequest,
    new_binary: &Path,
    old_binary: &Path,
    old_sha: &str,
    new_sha: &str,
    reason: &str,
) -> RestartLog {
    RestartLog {
        at_ms: crate::utils::time::now_ms(),
        who: restart_record::who_label(
            request.as_who.as_deref(),
            std::env::var("USER").ok().as_deref(),
        ),
        old_sha: old_sha.to_owned(),
        new_sha: new_sha.to_owned(),
        old_binary: old_binary.display().to_string(),
        new_binary: new_binary.display().to_string(),
        outcome: "refused".to_owned(),
        reason: reason.to_owned(),
    }
}

/// A restart that never reaches `--dev` still leaves a refused record when a
/// session can be named. No session means no write, including no touch of a
/// home Bus directory that this process was not given.
pub(super) fn record_dev_required_refusal(reason: &str) {
    let Some(root) = refusal_root() else {
        return;
    };
    let (old_binary, old_sha) = match serving_target(&root) {
        Ok((binary, sha)) => (binary.display().to_string(), sha),
        Err(_) => (String::new(), "unknown".to_owned()),
    };
    let record = RestartLog {
        at_ms: crate::utils::time::now_ms(),
        who: restart_record::who_label(None, std::env::var("USER").ok().as_deref()),
        old_sha,
        new_sha: "unknown".to_owned(),
        old_binary,
        new_binary: "bus".to_owned(),
        outcome: "refused".to_owned(),
        reason: reason.to_owned(),
    };
    let _ = restart_record::append_log(&restart_record::log_path(&root), &record);
    let _ = restart_record::write_json(&restart_record::outcome_path(&root), &record);
}

fn refusal_root() -> Option<PathBuf> {
    if let Some(root) = crate::utils::env::bus_data_dir() {
        return root.is_dir().then_some(root);
    }
    let base = crate::messaging::storage::sessions::default_base_dir().ok()?;
    let registry = crate::messaging::storage::sessions::LocalSessionRegistry::new(base);
    registry
        .last()
        .ok()
        .map(|session| session.root)
        .filter(|root| root.is_dir())
}

pub(super) fn record_refused(root: &Path, record: &RestartLog) -> io::Error {
    let log_error = restart_record::append_log(&restart_record::log_path(root), record).err();
    let outcome_error = if record.reason == RESTART_BUSY {
        None
    } else {
        restart_record::write_json(&restart_record::outcome_path(root), record).err()
    };
    let mut extra = String::new();
    if let Some(error) = &log_error {
        extra.push_str(&format!(" (could not append restarts/log.jsonl: {error})"));
    }
    if let Some(error) = &outcome_error {
        extra.push_str(&format!(
            " (could not write restarts/outcome.json: {error})"
        ));
    }
    if log_error.is_some() || outcome_error.is_some() {
        let body = format!(
            "restart attempt was not stored\n{extra}\nrecord: {}\n",
            serde_json::to_string(record).unwrap_or_default()
        );
        let path = restart_record::persist_error_path(root);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(&path, body) {
            extra.push_str(&format!(" (could not write {}: {error})", path.display()));
        }
    }
    io::Error::other(format!("{}{extra}", record.reason))
}

pub(super) fn acquire_restart_lock(
    root: &Path,
    owner: &restart_record::LivePid,
) -> Result<(), String> {
    acquire_restart_lock_once(root, owner, true)
}

fn acquire_restart_lock_once(
    root: &Path,
    owner: &restart_record::LivePid,
    retry_stale: bool,
) -> Result<(), String> {
    let path = restart_record::restart_lock_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            let body = serde_json::to_vec(owner).map_err(|error| error.to_string())?;
            file.write_all(&body).map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            observe_existing_lock(root, owner, retry_stale)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn observe_existing_lock(
    root: &Path,
    owner: &restart_record::LivePid,
    retry_stale: bool,
) -> Result<(), String> {
    let path = restart_record::restart_lock_path(root);
    for _ in 0..40 {
        if let Some(live) = restart_record::read_live_pid(&path) {
            if crate::platform::process_still_live(live.pid, live.birth) {
                return Err(RESTART_BUSY.to_owned());
            }
            return replace_stale_lock(root, owner, retry_stale);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    replace_stale_lock(root, owner, retry_stale)
}

fn replace_stale_lock(
    root: &Path,
    owner: &restart_record::LivePid,
    retry_stale: bool,
) -> Result<(), String> {
    if !retry_stale {
        return Err(RESTART_BUSY.to_owned());
    }
    let _ = std::fs::remove_file(restart_record::restart_lock_path(root));
    acquire_restart_lock_once(root, owner, false)
}

pub(super) fn assign_restart_lock(root: &Path, pid: u32) -> Result<(), String> {
    let live = crate::platform::process_instance(pid)
        .ok_or_else(|| format!("restart helper {pid} exited before it could own the lock"))?;
    restart_record::write_json(
        &restart_record::restart_lock_path(root),
        &restart_record::LivePid {
            pid: live.pid,
            birth: live.birth,
        },
    )
}

pub(super) fn release_restart_lock(root: &Path) {
    let path = restart_record::restart_lock_path(root);
    if let Some(live) = restart_record::read_live_pid(&path) {
        if live.pid != std::process::id()
            && crate::platform::process_still_live(live.pid, live.birth)
        {
            return;
        }
    }
    let _ = std::fs::remove_file(path);
}
