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
        "providers": ["claude", "codex", "cursor", "grok"],
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

pub(crate) fn read_build_sha(path: &Path) -> Option<String> {
    read_build(path).map(|(sha, _)| sha)
}

/// The serving binary recorded beside its SHA, when that file still exists.
pub(crate) fn read_build_binary(path: &Path) -> Option<PathBuf> {
    read_build(path)
        .and_then(|(_, binary)| binary)
        .map(PathBuf::from)
        .filter(|binary| binary.is_file())
}

fn read_build(path: &Path) -> Option<(String, Option<String>)> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let sha = value.get("sha").and_then(|item| item.as_str())?.to_owned();
    let binary = value
        .get("binary")
        .and_then(|item| item.as_str())
        .map(str::to_owned);
    Some((sha, binary))
}

/// Records the binary that is serving this data directory. A failure must not
/// stop the server: the next restart then logs the old SHA as unknown.
pub(crate) fn write_build_sha(root: Option<&Path>, sha: &str) {
    let Some(root) = root else {
        return;
    };
    let path = build_path(root);
    let binary = std::env::current_exe()
        .map(|exe| exe.display().to_string())
        .unwrap_or_default();
    if let Err(error) = write_json(&path, &serde_json::json!({ "sha": sha, "binary": binary })) {
        tracing::warn!(path = %path.display(), %error, "could not record the server build");
    }
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
}
