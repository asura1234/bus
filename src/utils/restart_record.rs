//! On-disk restart handoff shared by the CLI, the server, and the coordinator.
//!
//! Files live under `$BUS_DATA_DIR/restarts/` so the helper and the server agree
//! without depending on the terminal session name.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(crate) const MIN_FREE_BYTES: u64 = 5_000_000_000;
const RESTARTS: &str = "restarts";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Identity {
    pub(crate) sha: String,
    pub(crate) providers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RestartLog {
    pub(crate) at_ms: u64,
    pub(crate) who: String,
    pub(crate) old_sha: String,
    pub(crate) new_sha: String,
    pub(crate) old_binary: String,
    pub(crate) new_binary: String,
    /// `restarted`, `rolled_back`, `rollback_failed`, or `refused`.
    pub(crate) outcome: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct RestartMarker {
    pub(crate) binary: String,
    pub(crate) session_id: Option<String>,
    pub(crate) explicit_data_dir: bool,
    pub(crate) data_dir: String,
    pub(crate) dev: bool,
    pub(crate) yolo: bool,
}

/// How the running server was started. Restart reuses this instead of the
/// restart command's defaults.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchFlags {
    pub(crate) dev: bool,
    pub(crate) yolo: bool,
    pub(crate) explicit_data_dir: bool,
    pub(crate) session_id: Option<String>,
}

/// Fields present in `restarts/build.json`. Missing keys stay unknown so an
/// older record is not treated as `--yolo` off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecordedLaunch {
    pub(crate) dev: Option<bool>,
    pub(crate) yolo: Option<bool>,
    pub(crate) explicit_data_dir: Option<bool>,
    pub(crate) session_id: Option<String>,
    pub(crate) session_known: bool,
}

/// Flags the restart command itself set. `None` means inherit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LaunchOverride {
    pub(crate) yolo: Option<bool>,
    pub(crate) explicit_data_dir: bool,
    pub(crate) cli_dev: bool,
    pub(crate) cli_session_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct LivePid {
    pub(crate) pid: u32,
    pub(crate) birth: u64,
}

pub(crate) fn restarts_dir(root: &Path) -> PathBuf {
    root.join(RESTARTS)
}

pub(crate) fn outcome_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("outcome.json")
}

pub(crate) fn log_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("log.jsonl")
}

pub(crate) fn marker_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("in-progress")
}

pub(crate) fn build_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("build.json")
}

pub(crate) fn handoff_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("handoff.json")
}

pub(crate) fn handoff_pid_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("handoff.pid")
}

pub(crate) fn coordinator_pid_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("coordinator.pid")
}

pub(crate) fn restart_lock_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("lock")
}

pub(crate) fn persist_error_path(root: &Path) -> PathBuf {
    restarts_dir(root).join("persist-error.txt")
}

pub(crate) fn valid_build_sha(sha: &str) -> bool {
    sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit())
}

pub(crate) fn who_label(agent: Option<&str>, user: Option<&str>) -> String {
    let agent = agent.filter(|value| !value.is_empty());
    let user = user.filter(|value| !value.is_empty());
    match (agent, user) {
        (Some(agent), Some(user)) => format!("agent:{agent} user:{user}"),
        (Some(agent), None) => format!("agent:{agent}"),
        (None, Some(user)) => format!("user:{user}"),
        (None, None) => "unknown".to_owned(),
    }
}

pub(crate) fn notice_text(record: &RestartLog) -> String {
    match record.outcome.as_str() {
        "restarted" => {
            let notice = format!(
                "Restarted {} -> {} (by {}).",
                record.old_sha, record.new_sha, record.who
            );
            if record.reason.is_empty() {
                notice
            } else {
                format!("{notice} {}", record.reason)
            }
        }
        "rolled_back" => format!(
            "Bus restart rolled back to {}. {} did not become healthy: {}.",
            record.old_sha, record.new_sha, record.reason
        ),
        "refused" => format!(
            "Bus restart was refused before any stop. The server is still running unchanged and still serving {}. {} was not started: {}.",
            record.old_sha, record.new_binary, record.reason
        ),
        _ => format!(
            "Bus restart failed and the session is not serving {}. {} did not become healthy: {}.",
            record.old_sha, record.new_sha, record.reason
        ),
    }
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c))
    {
        value.to_owned()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

/// Command the developer runs when this TUI cannot stay attached.
pub(crate) fn reattach_command(marker: &RestartMarker) -> String {
    let binary = shell_quote(&marker.binary);
    let mut flags = String::new();
    if marker.dev {
        flags.push_str(" --dev");
    }
    if marker.yolo {
        flags.push_str(" --yolo");
    }
    if marker.explicit_data_dir {
        return format!(
            "BUS_DATA_DIR={} {binary}{flags}",
            shell_quote(&marker.data_dir)
        );
    }
    match marker.session_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => format!("{binary}{flags} resume {id}"),
        None => format!("{binary}{flags}"),
    }
}

pub(crate) fn reattach_message(marker: &RestartMarker, why: &str) -> String {
    format!(
        "Bus restarted and this client could not stay attached ({why}).\nRun `{}` to reattach.",
        reattach_command(marker)
    )
}

pub(crate) fn parse_identity(stdout: &str) -> Result<Identity, String> {
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .ok_or("identity did not include a JSON object")?;
    let value: serde_json::Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("identity JSON is invalid: {error}"))?;
    let sha = value
        .get("sha")
        .and_then(|item| item.as_str())
        .ok_or("identity is missing sha")?;
    if !valid_build_sha(sha) {
        return Err(format!(
            "identity sha is not a 40-character build id: {sha}"
        ));
    }
    let providers = value
        .get("providers")
        .and_then(|item| item.as_array())
        .ok_or("identity is missing providers")?;
    if providers.is_empty() {
        return Err("identity listed no providers".to_owned());
    }
    let mut names = Vec::with_capacity(providers.len());
    for provider in providers {
        let Some(name) = provider.as_str() else {
            return Err("identity providers must be strings".to_owned());
        };
        if name.is_empty() {
            return Err("identity listed an empty provider".to_owned());
        }
        names.push(name.to_owned());
    }
    Ok(Identity {
        sha: sha.to_owned(),
        providers: names,
    })
}

pub(crate) fn identity_json() -> String {
    serde_json::json!({
        "sha": env!("BUS_BUILD_SHA"),
        "providers": ["claude", "codex", "cursor", "grok", "kimi"],
    })
    .to_string()
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let temporary = unique_temp_path(path);
    std::fs::write(&temporary, &body).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        error.to_string()
    })
}

fn unique_temp_path(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("record");
    let seq = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{seq}.tmp", std::process::id()))
}

pub(crate) fn read_outcome(path: &Path) -> Result<Option<RestartLog>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("restart outcome is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn read_marker(path: &Path) -> Result<Option<RestartMarker>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("restart marker is invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn append_log(path: &Path, record: &RestartLog) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut line = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    line.push(b'\n');
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(&line).map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BuildStamp {
    pub(crate) sha: String,
    pub(crate) binary: Option<String>,
    /// Process that wrote this record. Absent on servers that predate the field.
    pub(crate) pid: Option<u32>,
}

pub(crate) fn read_build_sha(path: &Path) -> Option<String> {
    read_build_stamp(path).map(|stamp| stamp.sha)
}

/// The serving binary recorded beside its SHA, when that file still exists.
pub(crate) fn read_build_binary(path: &Path) -> Option<PathBuf> {
    read_build_stamp(path)
        .and_then(|stamp| stamp.binary)
        .map(PathBuf::from)
        .filter(|binary| binary.is_file())
}

pub(crate) fn read_build_pid(path: &Path) -> Option<u32> {
    read_build_stamp(path).and_then(|stamp| stamp.pid)
}

pub(crate) fn read_build_stamp(path: &Path) -> Option<BuildStamp> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let sha = value.get("sha").and_then(|item| item.as_str())?.to_owned();
    let binary = value
        .get("binary")
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    let pid = value
        .get("pid")
        .and_then(|item| item.as_u64())
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0);
    Some(BuildStamp { sha, binary, pid })
}

/// Records the binary and process that are serving this data directory.
/// A failure must not stop the server: the next restart refuses, because
/// there is no recorded binary to roll back to. The pid is how a client tells
/// this process from the one a same-binary restart is replacing.
pub(crate) fn write_build_sha(root: Option<&Path>, sha: &str) {
    let Some(root) = root else {
        return;
    };
    let path = build_path(root);
    let binary = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_default();
    let launch = current_launch_flags();
    let body = serde_json::json!({
        "sha": sha,
        "binary": binary,
        "pid": std::process::id(),
        "dev": launch.dev,
        "yolo": launch.yolo,
        "explicit_data_dir": launch.explicit_data_dir,
        "session_id": launch.session_id,
    });
    if let Err(error) = write_json(&path, &body) {
        tracing::warn!(path = %path.display(), %error, "could not record the server build");
    }
}

/// Launch flags visible to this process. The server records them; a later
/// restart reads that record rather than the restart command's defaults.
pub(crate) fn current_launch_flags() -> LaunchFlags {
    let session_id = std::env::var("BUS_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty());
    LaunchFlags {
        dev: std::env::var_os("BUS_DEV").is_some_and(|value| value == "1"),
        yolo: crate::utils::env::yolo_enabled(),
        explicit_data_dir: crate::utils::env::bus_data_dir().is_some() && session_id.is_none(),
        session_id,
    }
}

pub(crate) fn read_recorded_launch(path: &Path) -> Result<RecordedLaunch, String> {
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("restart build record is invalid: {error}"))?;
    let flag = |name: &str| -> Result<Option<bool>, String> {
        match value.get(name) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::Bool(bit)) => Ok(Some(*bit)),
            Some(_) => Err(format!(
                "restart build record has an invalid {name} setting"
            )),
        }
    };
    let session_known = value.get("session_id").is_some();
    let session_id = match value.get("session_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(id)) if !id.is_empty() => Some(id.clone()),
        Some(serde_json::Value::String(_)) => None,
        Some(_) => {
            return Err("restart build record has an invalid session_id".to_owned());
        }
    };
    Ok(RecordedLaunch {
        dev: flag("dev")?,
        yolo: flag("yolo")?,
        explicit_data_dir: flag("explicit_data_dir")?,
        session_id,
        session_known,
    })
}

/// Names which `--yolo` side differs and the command that keeps the server's.
pub(crate) fn yolo_mismatch_message(server_yolo: bool) -> String {
    let (server, command, keep) = if server_yolo {
        (
            "with --yolo",
            "without --yolo",
            "rerun this command with --yolo",
        )
    } else {
        (
            "without --yolo",
            "with --yolo",
            "rerun this command without --yolo",
        )
    };
    format!(
        "This Bus server was started {server}, but this command was started {command}. To keep the running server's --yolo setting, {keep}. To change it, quit its UI or run `bus stop`, then resume with the desired --yolo setting."
    )
}

pub(crate) fn yolo_unrecorded_message() -> String {
    "The running server has no recorded --yolo setting and its status API did not report one. To keep that setting, restart only after the server has recorded it at startup, or pass --yolo when the server is already in yolo mode. To change it, quit its UI or run `bus stop`, then resume with the desired --yolo setting.".to_owned()
}

/// Recorded flags win. A flag passed to `restart` overrides only that setting.
/// `--dev` on the restart command unlocks the dev tool and does not replace a
/// recorded dev mode. An explicit data-dir override replaces the directory
/// and clears the session. The CLI sets that override only for a data dir
/// this command itself set, not for the server's inherited root.
pub(crate) fn resolve_restart_launch(
    recorded: &RecordedLaunch,
    status_yolo: Option<bool>,
    over: &LaunchOverride,
) -> Result<LaunchFlags, String> {
    let yolo = if let Some(value) = over.yolo {
        value
    } else if let Some(value) = recorded.yolo {
        value
    } else if let Some(value) = status_yolo {
        value
    } else {
        return Err(yolo_unrecorded_message());
    };
    let dev = recorded.dev.unwrap_or(over.cli_dev);
    if over.explicit_data_dir {
        return Ok(LaunchFlags {
            dev,
            yolo,
            explicit_data_dir: true,
            session_id: None,
        });
    }
    let explicit_data_dir = recorded.explicit_data_dir.unwrap_or(false);
    let session_id = if explicit_data_dir {
        None
    } else if recorded.session_known {
        recorded.session_id.clone()
    } else {
        over.cli_session_id.clone()
    };
    Ok(LaunchFlags {
        dev,
        yolo,
        explicit_data_dir,
        session_id,
    })
}

pub(crate) fn read_live_pid(path: &Path) -> Option<LivePid> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_text_names_the_outcome() {
        let restarted = RestartLog {
            at_ms: 1,
            who: "agent:9 user:ada".into(),
            old_sha: "a".repeat(40),
            new_sha: "b".repeat(40),
            old_binary: "/old".into(),
            new_binary: "/new".into(),
            outcome: "restarted".into(),
            reason: String::new(),
        };
        assert_eq!(
            notice_text(&restarted),
            format!(
                "Restarted {} -> {} (by agent:9 user:ada).",
                "a".repeat(40),
                "b".repeat(40)
            )
        );
        let rolled = RestartLog {
            outcome: "rolled_back".into(),
            reason: "socket timeout".into(),
            ..restarted
        };
        assert_eq!(
            notice_text(&rolled),
            format!(
                "Bus restart rolled back to {}. {} did not become healthy: socket timeout.",
                "a".repeat(40),
                "b".repeat(40)
            )
        );
        let failed = RestartLog {
            outcome: "rollback_failed".into(),
            reason: "server exited".into(),
            ..rolled
        };
        let failed_notice = notice_text(&failed);
        assert!(!failed_notice.contains("rolled back"), "{failed_notice}");
        assert!(failed_notice.contains("not serving"));
        let refused = RestartLog {
            outcome: "refused".into(),
            ..failed
        };
        let refused_notice = notice_text(&refused);
        assert!(
            refused_notice.contains("refused before any stop"),
            "{refused_notice}"
        );
        assert!(
            refused_notice.contains("still running unchanged"),
            "{refused_notice}"
        );
        let old_sha = "a".repeat(40);
        assert!(
            refused_notice.contains(&format!("still serving {old_sha}")),
            "{refused_notice}"
        );
        assert!(
            refused_notice.contains("/new was not started"),
            "{refused_notice}"
        );
        assert!(
            !refused_notice.contains(&"b".repeat(40)),
            "{refused_notice}"
        );
        assert!(!refused_notice.contains("not serving"), "{refused_notice}");
    }

    #[test]
    fn reattach_command_matches_how_the_session_was_opened() {
        let marker = RestartMarker {
            binary: "/opt/bus".into(),
            session_id: Some("abc".into()),
            explicit_data_dir: false,
            data_dir: "/tmp/bus".into(),
            dev: true,
            yolo: false,
        };
        assert_eq!(reattach_command(&marker), "/opt/bus --dev resume abc");
        let explicit = RestartMarker {
            explicit_data_dir: true,
            dev: true,
            yolo: true,
            ..marker
        };
        assert_eq!(
            reattach_command(&explicit),
            "BUS_DATA_DIR=/tmp/bus /opt/bus --dev --yolo"
        );
        let spaced = RestartMarker {
            binary: "/opt/my bus".into(),
            session_id: None,
            explicit_data_dir: false,
            dev: false,
            yolo: false,
            ..explicit
        };
        assert_eq!(reattach_command(&spaced), "'/opt/my bus'");
    }

    #[test]
    fn who_label_uses_agent_and_user_when_they_exist() {
        assert_eq!(who_label(Some("9"), Some("ada")), "agent:9 user:ada");
        assert_eq!(who_label(None, None), "unknown");
        assert_eq!(who_label(Some(""), Some("ada")), "user:ada");
    }

    #[test]
    fn identity_includes_every_supported_provider() {
        let identity = parse_identity(&identity_json()).unwrap();
        assert_eq!(
            identity.providers,
            ["claude", "codex", "cursor", "grok", "kimi"]
        );
    }

    #[test]
    fn parse_identity_refuses_a_bad_build() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let ok = parse_identity(&format!(
            "{{\"sha\":\"{sha}\",\"providers\":[\"claude\",\"grok\"]}}\n"
        ))
        .unwrap();
        assert_eq!(ok.sha, sha);
        assert_eq!(ok.providers, ["claude", "grok"]);
        assert!(parse_identity(r#"{"sha":"unknown","providers":["claude"]}"#).is_err());
        assert!(parse_identity(r#"{"sha":"abc","providers":["claude"]}"#).is_err());
        assert!(parse_identity(&format!(r#"{{"sha":"{sha}","providers":[]}}"#)).is_err());
        assert!(parse_identity("not json").is_err());
    }

    #[test]
    fn log_appends_one_json_line_and_outcome_roundtrips() {
        let root = crate::utils::test_temp::unique_temp_path("restart-record");
        std::fs::create_dir_all(&root).unwrap();
        let record = RestartLog {
            at_ms: 5,
            who: "user:ada".into(),
            old_sha: "a".repeat(40),
            new_sha: "b".repeat(40),
            old_binary: "/old".into(),
            new_binary: "/new".into(),
            outcome: "restarted".into(),
            reason: String::new(),
        };
        append_log(&log_path(&root), &record).unwrap();
        append_log(&log_path(&root), &record).unwrap();
        let text = std::fs::read_to_string(log_path(&root)).unwrap();
        assert_eq!(text.lines().count(), 2);
        write_json(&outcome_path(&root), &record).unwrap();
        assert_eq!(read_outcome(&outcome_path(&root)).unwrap(), Some(record));
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn recorded_yolo_server() -> RecordedLaunch {
        RecordedLaunch {
            dev: Some(true),
            yolo: Some(true),
            explicit_data_dir: Some(false),
            session_id: Some("abc".into()),
            session_known: true,
        }
    }

    fn inherit_override() -> LaunchOverride {
        LaunchOverride {
            yolo: None,
            explicit_data_dir: false,
            cli_dev: true,
            cli_session_id: Some("other".into()),
        }
    }

    #[test]
    fn restart_keeps_recorded_yolo_unless_the_command_overrides_it() {
        let recorded = recorded_yolo_server();
        let inherit = inherit_override();
        let kept = resolve_restart_launch(&recorded, None, &inherit).unwrap();
        assert!(kept.yolo, "omitted --yolo keeps the running server's yolo");
        assert!(kept.dev);
        assert!(!kept.explicit_data_dir);
        assert_eq!(kept.session_id.as_deref(), Some("abc"));

        let forced = resolve_restart_launch(
            &RecordedLaunch {
                yolo: Some(false),
                ..recorded.clone()
            },
            Some(false),
            &LaunchOverride {
                yolo: Some(true),
                ..inherit.clone()
            },
        )
        .unwrap();
        assert!(forced.yolo, "explicit --yolo turns yolo on");
        assert!(forced.dev, "the yolo override does not change dev");
        assert_eq!(forced.session_id.as_deref(), Some("abc"));

        let quiet = resolve_restart_launch(
            &RecordedLaunch {
                dev: Some(false),
                yolo: Some(false),
                ..recorded
            },
            None,
            &inherit,
        )
        .unwrap();
        assert!(
            !quiet.dev,
            "the required --dev tool gate does not replace recorded dev"
        );
        assert!(!quiet.yolo);

        let directory = resolve_restart_launch(
            &recorded_yolo_server(),
            None,
            &LaunchOverride {
                explicit_data_dir: true,
                ..inherit_override()
            },
        )
        .unwrap();
        assert!(directory.yolo);
        assert!(directory.explicit_data_dir);
        assert!(directory.session_id.is_none());

        let legacy = RecordedLaunch {
            dev: None,
            yolo: None,
            explicit_data_dir: None,
            session_id: None,
            session_known: false,
        };
        let from_status = resolve_restart_launch(&legacy, Some(true), &inherit_override()).unwrap();
        assert!(
            from_status.yolo,
            "a legacy record uses the live status yolo"
        );
        assert!(from_status.dev);
        assert_eq!(from_status.session_id.as_deref(), Some("other"));

        let missing = resolve_restart_launch(&legacy, None, &inherit_override()).unwrap_err();
        assert!(missing.contains("--yolo"), "{missing}");
        assert!(missing.contains("To keep that setting"), "{missing}");
    }

    #[test]
    fn yolo_mismatch_names_the_side_that_differs_and_how_to_keep_it() {
        let on = yolo_mismatch_message(true);
        assert!(on.contains("started with --yolo"), "{on}");
        assert!(on.contains("started without --yolo"), "{on}");
        assert!(on.contains("rerun this command with --yolo"), "{on}");
        assert!(
            on.contains("To keep the running server's --yolo setting"),
            "{on}"
        );
        let off = yolo_mismatch_message(false);
        assert!(off.contains("started without --yolo"), "{off}");
        assert!(off.contains("rerun this command without --yolo"), "{off}");
        assert!(!off.contains("rerun this command with --yolo"), "{off}");
    }

    #[test]
    fn write_build_sha_records_the_serving_pid() {
        let root = crate::utils::test_temp::unique_temp_path("build-pid");
        std::fs::create_dir_all(&root).unwrap();
        let sha = "ab".repeat(20);
        write_build_sha(Some(&root), &sha);
        let stamp = read_build_stamp(&build_path(&root)).unwrap();
        assert_eq!(stamp.sha, sha);
        assert_eq!(stamp.pid, Some(std::process::id()));
        assert!(stamp.binary.is_some());
        write_json(
            &build_path(&root),
            &serde_json::json!({ "sha": sha, "binary": "/tmp/bus" }),
        )
        .unwrap();
        let legacy = read_build_stamp(&build_path(&root)).unwrap();
        assert_eq!(legacy.pid, None);
        assert_eq!(legacy.binary.as_deref(), Some("/tmp/bus"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
