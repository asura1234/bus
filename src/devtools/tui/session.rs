//! Where a driver session lives, and the guards that keep it away from every
//! Bus the driver did not start itself.
//!
//! A session is always a fresh Bus: its own generated data directory, its own
//! server and its own hidden terminal. Nothing here attaches to an existing
//! session; the guards refuse a target that a live Bus already owns and scrub
//! every inherited Bus variable from the spawned instance.
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Concurrent driver sessions on one machine; each one is a full Bus.
pub(super) const MAX_SESSIONS: usize = 4;
pub(super) const DEFAULT_NAME: &str = "s1";
/// Lets the integration tests drive the cargo-built `target/debug/bus`.
pub(super) const ALLOW_TARGET_DEBUG_ENV: &str = "BUS_TUI_ALLOW_TARGET_DEBUG";
/// Marks a directory as created by the driver, so stale cleanup never
/// removes anything else.
const MARKER: &str = "bus-tui-session";

/// Variables that name another Bus or its sockets. They are never passed to
/// the spawned instance, and refused outright when they point at the target.
const LOCATOR_VARS: &[&str] = &[
    "BUS_DATA_DIR",
    "BUS_CALLBACK_DIR",
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_CONFIG_PATH",
];

/// The only inherited variables the scratch Bus sees; everything else,
/// including every `BUS_*`, `HERDR_*`, `CLAUDE*` and terminal identity, is dropped.
const INHERITED_VARS: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "LANG",
    "TZ",
    "TMPDIR",
    "SYSTEMROOT",
    "SystemRoot",
    "WINDIR",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "PATHEXT",
    "COMSPEC",
    "TEMP",
    "TMP",
    "PROGRAMDATA",
    "ProgramFiles",
    "ProgramFiles(x86)",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct SessionInfo {
    pub name: String,
    pub host_pid: u32,
    pub bus_pid: Option<u32>,
    pub data_dir: PathBuf,
    pub run_dir: PathBuf,
    pub binary: PathBuf,
    pub size: (u16, u16),
}

#[derive(Debug, Clone)]
pub(super) struct SessionPaths {
    pub name: String,
    pub dir: PathBuf,
}

impl SessionPaths {
    pub fn new(root: &Path, name: &str) -> Self {
        Self {
            name: name.to_owned(),
            dir: root.join(name),
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.dir.join("data")
    }

    pub fn work_dir(&self) -> PathBuf {
        self.dir.join("work")
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.dir.join("bin")
    }

    pub fn xdg_dir(&self) -> PathBuf {
        self.dir.join("xdg")
    }

    pub fn socket(&self) -> PathBuf {
        self.dir.join("d.sock")
    }

    pub fn info_file(&self) -> PathBuf {
        self.dir.join("session.json")
    }

    pub fn host_log(&self) -> PathBuf {
        self.dir.join("host.log")
    }

    pub fn read_info(&self) -> Option<SessionInfo> {
        let text = std::fs::read_to_string(self.info_file()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn write_info(&self, info: &SessionInfo) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(info).map_err(std::io::Error::other)?;
        let tmp = self.dir.join("session.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, self.info_file())
    }

    pub fn is_driver_dir(&self) -> bool {
        self.dir.join(MARKER).is_file()
    }

    /// Creates the private session directory and its marker.
    pub fn create(&self) -> std::io::Result<()> {
        create_private_dir(&self.dir)?;
        std::fs::write(self.dir.join(MARKER), b"")?;
        for dir in [
            self.data_dir(),
            self.work_dir(),
            self.bin_dir(),
            self.xdg_dir(),
        ] {
            create_private_dir(&dir)?;
        }
        Ok(())
    }

    /// Removes a session directory the driver created. Refuses anything without
    /// the marker, so a mistaken path can never delete foreign data.
    pub fn remove(&self) -> std::io::Result<()> {
        if !self.is_driver_dir() {
            return Ok(());
        }
        std::fs::remove_dir_all(&self.dir)
    }
}

/// The fixed driver root. Unix uses `/tmp` because macOS `$TMPDIR` is long
/// enough to push Bus's sockets past the `sockaddr_un` limit.
pub(super) fn driver_root() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/tmp/bus-tui")
    }
    #[cfg(not(unix))]
    {
        std::env::temp_dir().join("bus-tui")
    }
}

pub(super) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(super) fn validate_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 16
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !name.starts_with('-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid session name {name:?}: use 1-16 of a-z, 0-9 and '-', not starting with '-'"
        ))
    }
}

pub(super) fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 checks existence without delivering anything.
        let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let filter = format!("PID eq {pid}");
        std::process::Command::new("tasklist")
            .args(["/FI", &filter, "/NH"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// A session is live while its host process runs.
pub(super) fn session_alive(paths: &SessionPaths) -> bool {
    paths
        .read_info()
        .is_some_and(|info| process_alive(info.host_pid))
}

pub(super) fn live_sessions(root: &Path) -> Vec<SessionInfo> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut sessions: Vec<SessionInfo> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let paths = SessionPaths::new(root, &name);
            let info = paths.read_info()?;
            process_alive(info.host_pid).then_some(info)
        })
        .collect();
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    sessions
}

/// Processes other than this one with a file open under `dir`. Used after
/// `bus stop` so a session never leaves a server or agent behind.
pub(super) fn pids_holding(dir: &Path) -> Vec<u32> {
    #[cfg(unix)]
    {
        let Ok(output) = std::process::Command::new("lsof")
            .args(["-t", "+D"])
            .arg(dir)
            .stderr(std::process::Stdio::null())
            .output()
        else {
            return Vec::new();
        };
        let me = std::process::id();
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|pid| pid.parse().ok())
            .filter(|pid| *pid != me)
            .collect()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Vec::new()
    }
}

pub(super) fn terminate(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
}

/// Sockets a running Bus serves from its data directory.
fn bus_socket_candidates(data_dir: &Path) -> Vec<PathBuf> {
    vec![
        data_dir.join("control.sock"),
        data_dir.join("dev-control.sock"),
        data_dir.join("herdr-config").join("herdr.sock"),
        data_dir.join("herdr-config").join("herdr-client.sock"),
    ]
}

fn socket_answers(path: &Path) -> bool {
    path.exists()
        && crate::platform::ipc::connect_local_stream_timeout(path, Duration::from_millis(300))
            .is_ok()
}

/// Refuses a target directory that a live Bus already serves.
pub(super) fn ensure_not_live_bus(data_dir: &Path) -> Result<(), String> {
    match bus_socket_candidates(data_dir)
        .into_iter()
        .find(|socket| socket_answers(socket))
    {
        Some(socket) => Err(format!(
            "refusing to use {}: a live Bus session answers on {}; the driver only controls Bus instances it starts itself",
            data_dir.display(),
            socket.display()
        )),
        None => Ok(()),
    }
}

/// Refuses inherited locator variables that point inside the target, which
/// would make the scratch Bus and the inherited session the same one.
pub(super) fn ensure_env_does_not_point_at(
    target: &Path,
    vars: &[(String, OsString)],
) -> Result<(), String> {
    for (key, value) in vars {
        if !LOCATOR_VARS.contains(&key.as_str()) || value.is_empty() {
            continue;
        }
        let path = PathBuf::from(value);
        if path.starts_with(target) || target.starts_with(&path) {
            return Err(format!(
                "refusing to start: inherited {key}={} points at the driver's target {}",
                path.display(),
                target.display()
            ));
        }
    }
    Ok(())
}

/// Refuses a target inside a real Bus home (the session registry or the
/// platform config directory), even though the driver never generates one.
pub(super) fn ensure_outside_bus_homes(target: &Path, homes: &[PathBuf]) -> Result<(), String> {
    for home in homes {
        if target.starts_with(home) || home.starts_with(target) {
            return Err(format!(
                "refusing to use {}: it overlaps the Bus home {}",
                target.display(),
                home.display()
            ));
        }
    }
    Ok(())
}

pub(super) fn bus_homes() -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Ok(base) = crate::messaging::storage::sessions::default_base_dir() {
        homes.push(base);
    }
    if let Some(home) = std::env::home_dir() {
        homes.push(home.join(".config").join("bus"));
        homes.push(home.join(".config").join("bus-dev"));
        homes.push(home.join(".local").join("state").join("bus"));
    }
    homes
}

/// Refuses to drive the developer's `target/debug/bus`, which `./run dev`
/// rebuilds and runs; `./run tui` builds into `target/tui-driver` instead.
pub(super) fn ensure_binary_allowed(binary: &Path, allow_target_debug: bool) -> Result<(), String> {
    let parts: Vec<String> = binary
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let n = parts.len();
    let is_target_debug = n >= 3
        && parts[n - 3] == "target"
        && parts[n - 2] == "debug"
        && (parts[n - 1] == "bus" || parts[n - 1] == "bus.exe");
    if is_target_debug && !allow_target_debug {
        return Err(format!(
            "refusing to drive {}: target/debug/bus is the developer's ./run dev binary; use ./run tui, which builds target/tui-driver/bus",
            binary.display()
        ));
    }
    Ok(())
}

/// The complete environment of the spawned Bus: an allow-list of inherited
/// basics plus the session's own locations. No inherited Bus variable survives.
pub(super) fn scratch_env(
    paths: &SessionPaths,
    inherited: &[(String, OsString)],
) -> Vec<(String, OsString)> {
    let mut env: Vec<(String, OsString)> = inherited
        .iter()
        .filter(|(key, _)| INHERITED_VARS.contains(&key.as_str()) || key.starts_with("LC_"))
        .cloned()
        .collect();
    if let Some((_, path)) = env.iter_mut().find(|(key, _)| key == "PATH") {
        let mut joined = OsString::from(paths.bin_dir());
        joined.push(if cfg!(windows) { ";" } else { ":" });
        joined.push(path.clone());
        *path = joined;
    } else {
        env.push(("PATH".into(), paths.bin_dir().into()));
    }
    let xdg = paths.xdg_dir();
    env.extend([
        ("BUS_DATA_DIR".into(), paths.data_dir().into()),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("XDG_CONFIG_HOME".into(), xdg.join("config").into()),
        ("XDG_STATE_HOME".into(), xdg.join("state").into()),
        ("XDG_RUNTIME_DIR".into(), xdg.join("runtime").into()),
        // Copies then go out as OSC 52, which the driver captures, instead of
        // reaching the developer's clipboard through pbcopy.
        ("SSH_TTY".into(), "/dev/null".into()),
        // Lets the provider names in `bin_dir` act as fake agents.
        (super::fake_agents::FAKE_AGENT_ENV.into(), "1".into()),
    ]);
    #[cfg(unix)]
    env.push(("SHELL".into(), "/bin/sh".into()));
    env
}

pub(super) fn inherited_env() -> Vec<(String, OsString)> {
    std::env::vars_os()
        .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
        .collect()
}

/// Where run artifacts go: the checkout's `temp/tui-driver/runs` when the
/// driver runs inside a Bus checkout, else the platform state directory.
pub(super) fn default_runs_root(cwd: &Path) -> PathBuf {
    if let Some(repo) = find_bus_checkout(cwd) {
        return repo.join("temp").join("tui-driver").join("runs");
    }
    state_home().join("bus").join("tui-runs")
}

pub(super) fn find_bus_checkout(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| {
            dir.join("src").join("devtools").is_dir()
                && std::fs::read_to_string(dir.join("Cargo.toml"))
                    .is_ok_and(|text| text.contains("name = \"bus\""))
        })
        .map(Path::to_path_buf)
}

fn state_home() -> PathBuf {
    #[cfg(windows)]
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::home_dir()
        .map(|home| home.join(".local").join("state"))
        .unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
#[path = "tests/session_test.rs"]
mod tests;
