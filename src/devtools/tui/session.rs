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
/// removes anything else. It holds the session's [`Claim`].
const MARKER: &str = "bus-tui-session";
/// Held locked by the session's host for its whole life.
const OWNER_LOCK: &str = "owner.lock";
/// Serializes claiming names under the driver root.
const ROOT_LOCK: &str = ".start.lock";
/// Carries the session's claim token into every process the scratch Bus
/// starts, so teardown signals only those.
pub(super) const OWNER_ENV: &str = "BUS_TUI_OWNER";
/// Moves the driver root, so test suites never share the cap with other
/// drivers on the machine. Must be absolute and short (sockets live under it).
pub(super) const ROOT_ENV: &str = "BUS_TUI_ROOT";

/// Variables that name another Bus or its sockets. They are never passed to
/// the spawned instance, and refused outright when they point at the target.
const LOCATOR_VARS: &[&str] = &[
    "BUS_DATA_DIR",
    "BUS_CALLBACK_DIR",
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_CONFIG_PATH",
];

/// The only inherited variables the scratch Bus sees, with the locale's
/// `LC_*`; everything else, including `HOME`, every `BUS_*`, `HERDR_*`,
/// `CLAUDE*` and terminal identity, is dropped.
const INHERITED_VARS: &[&str] = &[
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
    // Coverage runs: instrumented processes write profiles where the gate collects
    // them instead of into their working directory.
    "LLVM_PROFILE_FILE",
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
    #[serde(default)]
    pub home: PathBuf,
    #[serde(default)]
    pub graphics: Graphics,
    #[serde(default)]
    pub yolo: bool,
}

/// Who reserved a session directory: the token its host must present, and
/// the `start` process that waits for that host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct Claim {
    pub token: String,
    pub claimant: u32,
}

impl Claim {
    pub fn new(name: &str) -> Self {
        use sha2::Digest as _;
        let seed = format!(
            "{name}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        );
        let token = sha2::Sha256::digest(seed.as_bytes())[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Self {
            token,
            claimant: std::process::id(),
        }
    }
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

    /// The scratch Bus's `HOME`, so nothing it lists or writes under `~` is the
    /// developer's.
    pub fn home_dir(&self) -> PathBuf {
        self.dir.join("home")
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

    /// What the session's effect stand-ins recorded, one JSON line each.
    pub fn effects_log(&self) -> PathBuf {
        self.dir.join("effects.ndjson")
    }

    /// The image the clipboard stand-in hands to Bus's image paste.
    pub fn clipboard_image(&self) -> PathBuf {
        self.dir.join("clipboard.png")
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

    pub fn read_claim(&self) -> Option<Claim> {
        let text = std::fs::read_to_string(self.dir.join(MARKER)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Reserves the session: creates its private directory, which must not
    /// exist yet, and writes the claim into the marker.
    pub fn claim(&self, claim: &Claim) -> std::io::Result<()> {
        std::fs::create_dir(&self.dir)?;
        create_private_dir(&self.dir)?;
        let text = serde_json::to_string(claim).map_err(std::io::Error::other)?;
        std::fs::write(self.dir.join(MARKER), text)?;
        for dir in [
            self.data_dir(),
            self.work_dir(),
            self.bin_dir(),
            self.xdg_dir(),
            self.home_dir(),
        ] {
            create_private_dir(&dir)?;
        }
        Ok(())
    }

    /// True while someone owns the session: its `start` still waits for the
    /// host, or a host holds the owner lock. Only an unowned dir is stale.
    pub fn owned(&self) -> bool {
        if self
            .read_claim()
            .is_some_and(|claim| process_alive(claim.claimant))
        {
            return true;
        }
        let Ok(lock) = std::fs::File::open(self.dir.join(OWNER_LOCK)) else {
            return false;
        };
        match lock.try_lock() {
            Ok(()) => false,
            Err(std::fs::TryLockError::WouldBlock) => true,
            // Unknown: treat it as owned, so it is never cleared.
            Err(std::fs::TryLockError::Error(_)) => true,
        }
    }

    /// The host's side of the claim: refuses unless `token` matches the claim
    /// and no other host owns the session, before the host touches anything.
    /// The returned file keeps the owner lock until the host exits.
    pub fn take_ownership(&self, token: &str) -> Result<std::fs::File, String> {
        match self.read_claim() {
            Some(claim) if claim.token == token => {}
            _ => {
                return Err(format!(
                    "{} is not claimed for this host; sessions start only through `start`",
                    self.dir.display()
                ))
            }
        }
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join(OWNER_LOCK))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|_| format!("session {:?} already has a host", self.name))?;
        Ok(lock)
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

/// The driver root, `$BUS_TUI_ROOT` when set to an absolute path. Unix uses
/// `/tmp` because macOS `$TMPDIR` is long enough to push Bus's sockets past
/// the `sockaddr_un` limit.
pub(super) fn driver_root() -> PathBuf {
    driver_root_for(std::env::var_os(ROOT_ENV))
}

/// `driver_root` for a given `$BUS_TUI_ROOT` value, so tests can check the
/// default without depending on the caller's environment.
fn driver_root_for(value: Option<OsString>) -> PathBuf {
    // `bus --dev tui` refuses a malformed override before any verb runs.
    if let Ok(Some(root)) = parse_root_override(value) {
        return root;
    }
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

/// `$BUS_TUI_ROOT`, when set: refused unless absolute and free of `.` and
/// `..` components, so it names one directory however much of it exists.
pub(super) fn root_override() -> Result<Option<PathBuf>, String> {
    parse_root_override(std::env::var_os(ROOT_ENV))
}

fn parse_root_override(value: Option<OsString>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let root = PathBuf::from(&value);
    let dotted = value
        .to_string_lossy()
        .split(std::path::is_separator)
        .any(|part| part == "." || part == "..");
    if !root.is_absolute() || dotted {
        return Err(format!(
            "refusing {ROOT_ENV}={}: it must be an absolute path without . or .. components",
            root.display()
        ));
    }
    Ok(Some(root))
}

/// Refuses a driver root that is, contains or sits inside a Bus location
/// (an inherited locator, a Bus home, a live data dir), and an existing
/// `$BUS_TUI_ROOT` the driver did not create. Runs before `start` creates,
/// chmods or locks anything in the root.
pub(super) fn ensure_root_isolated(
    root: &Path,
    inherited: &[(String, OsString)],
    homes: &[PathBuf],
) -> Result<(), String> {
    ensure_env_does_not_point_at(root, inherited)?;
    ensure_outside_bus_homes(root, homes)?;
    // The root, or any directory above it, must not be a live Bus's data dir.
    for dir in resolve_existing(root)?.ancestors() {
        ensure_not_live_bus(dir)?;
    }
    let overridden = std::env::var_os(ROOT_ENV).is_some_and(|value| Path::new(&value) == root);
    let entries = std::fs::read_dir(root).map(|entries| {
        entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect::<Vec<_>>()
    });
    match entries {
        Ok(names) if overridden && !names.is_empty() && !names.iter().any(|n| n == ROOT_LOCK) => {
            Err(format!(
                "refusing {ROOT_ENV}={}: it already holds files the driver did not create; use a new or empty directory",
                root.display()
            ))
        }
        _ => Ok(()),
    }
}

/// Holds the root lock while a `start` checks and claims a name, so two
/// starts never both judge the same directory stale or free.
pub(super) fn lock_root(root: &Path) -> std::io::Result<std::fs::File> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(ROOT_LOCK))?;
    lock.lock()?;
    Ok(lock)
}

/// Names of every session someone owns, including ones still starting.
pub(super) fn owned_sessions(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            let paths = SessionPaths::new(root, name);
            paths.is_driver_dir() && paths.owned()
        })
        .collect();
    names.sort();
    names
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

/// Processes the session started (they carry its owner token) that still
/// hold something under `dir`. Used after `bus stop` so a session never
/// leaves a server or agent behind, and never signals anything else.
pub(super) fn pids_owned(dir: &Path, token: &str) -> Vec<u32> {
    let marker = format!("{OWNER_ENV}={token}");
    pids_holding(dir)
        .into_iter()
        .filter(|pid| {
            crate::platform::process_environment(*pid).is_some_and(|env| {
                env.split(|byte| *byte == 0)
                    .any(|record| record == marker.as_bytes())
            })
        })
        .collect()
}

/// Processes other than this one with a file open under `dir`.
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

/// Sockets a running Bus serves from its data directory. Bus runs its herdr
/// server as a named session (`--session bus`), whose sockets live under
/// `herdr-config/sessions/NAME`; the unnamed layout is checked too.
fn bus_socket_candidates(data_dir: &Path) -> Vec<PathBuf> {
    let herdr = data_dir.join("herdr-config");
    let mut herdr_dirs = vec![herdr.clone()];
    if let Ok(entries) = std::fs::read_dir(herdr.join("sessions")) {
        herdr_dirs.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
    }
    let mut candidates = vec![
        data_dir.join("control.sock"),
        data_dir.join("dev-control.sock"),
    ];
    for dir in herdr_dirs {
        candidates.push(dir.join("herdr.sock"));
        candidates.push(dir.join("herdr-client.sock"));
    }
    candidates
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

/// The path with symlinks and aliases (macOS `/tmp` is `/private/tmp`)
/// resolved: its deepest existing ancestor canonicalized, the missing rest
/// appended. Creates nothing. A path that cannot be resolved this way (no
/// existing ancestor, or `.`/`..` in its missing part) is an error, never
/// compared as spelled.
pub(super) fn resolve_existing(path: &Path) -> Result<PathBuf, String> {
    let unresolvable = || format!("cannot resolve {}", path.display());
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return Ok(missing
                .iter()
                .rev()
                .fold(resolved, |path, part| path.join(part)));
        }
        let name = match existing.components().next_back() {
            Some(std::path::Component::Normal(name)) => name,
            _ => return Err(unresolvable()),
        };
        missing.push(name.to_owned());
        existing = existing.parent().ok_or_else(unresolvable)?;
    }
}

/// Whether one path contains the other, however each is spelled.
fn overlaps(a: &Path, b: &Path) -> Result<bool, String> {
    let (a, b) = (resolve_existing(a)?, resolve_existing(b)?);
    Ok(a.starts_with(&b) || b.starts_with(&a))
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
        if overlaps(&path, target)? {
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
        if overlaps(home, target)? {
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
            "refusing to drive {}: target/debug/bus is the developer's ./run dev binary; use ./run tui, which builds into target/tui-driver",
            binary.display()
        ));
    }
    Ok(())
}

/// How the scratch Bus draws pictures in its terminal; never taken from the
/// runner's own terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(super) enum Graphics {
    /// Kitty graphics, which the driver's screen and `screenshot --real` show.
    #[default]
    Kitty,
    /// No picture protocol: Bus shows attachments as file names.
    None,
}

impl Graphics {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "kitty" => Ok(Self::Kitty),
            "none" => Ok(Self::None),
            other => Err(format!("unknown --graphics {other:?}; use kitty or none")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::None => "none",
        }
    }

    /// The variables that make Bus pick this protocol. Bus reads
    /// `GHOSTTY_RESOURCES_DIR` only to choose Kitty pictures, unlike
    /// `TERM_PROGRAM`, which also changes keys and notifications.
    pub fn env(self, paths: &SessionPaths) -> Vec<(String, OsString)> {
        match self {
            Self::Kitty => vec![("GHOSTTY_RESOURCES_DIR".into(), paths.home_dir().into())],
            Self::None => Vec::new(),
        }
    }
}

/// The complete environment of the spawned Bus: an allow-list of inherited
/// basics plus the session's own locations. No inherited Bus variable survives.
pub(super) fn scratch_env(
    paths: &SessionPaths,
    token: &str,
    inherited: &[(String, OsString)],
) -> Vec<(String, OsString)> {
    let mut env: Vec<(String, OsString)> = inherited
        .iter()
        .filter(|(key, _)| {
            INHERITED_VARS.contains(&key.as_str())
                // iTerm2 names itself in LC_TERMINAL*, which would pick Bus's
                // picture protocol from the runner's terminal.
                || (key.starts_with("LC_") && !key.starts_with("LC_TERMINAL"))
        })
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
        ("HOME".into(), paths.home_dir().into()),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("XDG_CONFIG_HOME".into(), xdg.join("config").into()),
        ("XDG_STATE_HOME".into(), xdg.join("state").into()),
        ("XDG_RUNTIME_DIR".into(), xdg.join("runtime").into()),
        // Copies then go out as OSC 52, which the driver captures, instead of
        // reaching the developer's clipboard through pbcopy.
        ("SSH_TTY".into(), "/dev/null".into()),
        // Lets the provider and OS command names in `bin_dir` act as stand-ins.
        (super::fake_agents::FAKE_AGENT_ENV.into(), "1".into()),
        (OWNER_ENV.into(), token.into()),
        (
            super::effects::EFFECTS_ENV.into(),
            paths.effects_log().into(),
        ),
        (
            super::effects::CLIPBOARD_IMAGE_ENV.into(),
            paths.clipboard_image().into(),
        ),
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
