use crate::agents::Agent;
use crate::layout::PaneId;
use crate::terminal::emulator::GhosttyPaneTerminal;
use crate::terminal::emulator::PaneTerminal;
use crate::terminal::events::AppEvent;
use crate::terminal::pty::actor::PtyIoActor;
use crate::terminal::pty::actor::PtyIoActorConfig;
#[cfg(unix)]
use crate::terminal::pty::fd;
use crate::terminal::runtime::compression::TerminalCompressionTask;
use crate::terminal::runtime::detection_task::spawn_detection_task;
use crate::terminal::runtime::io::PaneRuntimeIo;
use crate::terminal::runtime::read::{pty_read_callback, PtyReadContext};
use crate::terminal::runtime::AgentDetection;
use crate::terminal::runtime::TerminalRuntime;
use crate::utils::render::signal::RenderSignal;
use bytes::Bytes;
use portable_pty::native_pty_system;
use portable_pty::Child;
use portable_pty::CommandBuilder;
#[cfg(windows)]
use portable_pty::MasterPty;
use portable_pty::PtySize;
use std::cell::Cell;
use std::io;
#[cfg(unix)]
use std::os::fd::FromRawFd;
#[cfg(unix)]
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tracing::error;

pub(super) const PANE_TERM: &str = "xterm-256color";

pub(super) const PANE_COLORTERM: &str = "truecolor";

pub(super) fn apply_pane_terminal_env(cmd: &mut CommandBuilder) {
    // Each pane is rendered by herdr's own terminal layer, not the outer terminal
    // that launched the app. Advertising the inherited TERM leaks the host terminal
    // identity into shells and across SSH, which breaks redraw and cursor movement
    // when the remote side lacks matching terminfo entries.
    cmd.env("TERM", PANE_TERM);
    cmd.env("COLORTERM", PANE_COLORTERM);
    // The server may be launched by a noninteractive tool with NO_COLOR set.
    // A pane is its own interactive terminal; launch_env can still explicitly
    // opt out of colors after these defaults are applied.
    cmd.env_remove("NO_COLOR");
    cmd.env_remove("WT_SESSION");
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PaneLaunchEnv {
    pub(super) extra: Vec<(String, String)>,
    pub(super) identity: PaneLaunchIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum PaneLaunchIdentity {
    #[default]
    Inherit,
    Managed {
        workspace_id: String,
        tab_id: String,
        pane_id: String,
    },
}

impl PaneLaunchEnv {
    pub(crate) fn from_extra(extra: Vec<(String, String)>) -> Self {
        Self {
            extra,
            identity: PaneLaunchIdentity::Inherit,
        }
    }

    pub(crate) fn with_identity(
        mut self,
        workspace_id: String,
        tab_id: String,
        pane_id: String,
    ) -> Self {
        self.identity = PaneLaunchIdentity::Managed {
            workspace_id,
            tab_id,
            pane_id,
        };
        self
    }
}

pub(super) fn apply_pane_launch_env(cmd: &mut CommandBuilder, launch_env: &PaneLaunchEnv) {
    for (key, value) in &launch_env.extra {
        cmd.env(key, value);
    }
    cmd.env(crate::HERDR_ENV_VAR, crate::HERDR_ENV_VALUE);
    apply_pane_base_env(cmd);
    crate::platform::apply_pane_runtime_marker(cmd);
    match &launch_env.identity {
        PaneLaunchIdentity::Inherit => {}
        PaneLaunchIdentity::Managed {
            workspace_id,
            tab_id,
            pane_id,
        } => {
            cmd.env(HERDR_WORKSPACE_ID_ENV_VAR, workspace_id);
            cmd.env(HERDR_TAB_ID_ENV_VAR, tab_id);
            cmd.env(HERDR_PANE_ID_ENV_VAR, pane_id);
        }
    }
    // New panes and cold resumes are independent provider sessions, even when
    // Bus itself was started from an agent's tool. Strip the parent's identity,
    // child/transcript flags and tool IPC after overrides have been applied.
    // Keep configuration (CLAUDE_CONFIG_DIR, ANTHROPIC_*, CODEX_HOME, etc.);
    // these prefixes also contain user settings, so do not remove them wholesale.
    for key in [
        "CLAUDECODE",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_SESSION_ATTENDED",
        "CLAUDE_CODE_SSE_PORT",
        "CLAUDE_CODE_MESSAGING_SOCKET",
        "CLAUDE_CODE_MESSAGING_TOKEN",
        "CLAUDE_CODE_SANDBOXED",
        "CLAUDE_PID",
        "CLAUDE_JOB_DIR",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED",
        "CODEX_PERMISSION_PROFILE",
        "CODEX_ESCALATE_SOCKET",
        "CODEX_EXEC_SERVER_NOISE_AUTH_TOKEN",
    ] {
        cmd.env_remove(key);
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct SpawnInitialState<'a> {
    pub(super) detected_agent: Option<Agent>,
    pub(super) history_ansi: Option<&'a str>,
    pub(super) windows_powershell_prompt_cwd_reporting: bool,
}

pub(super) fn pane_shell(configured_shell: &str) -> String {
    pane_shell_from(configured_shell, std::env::var("SHELL").ok())
}

pub(super) fn pane_shell_from(configured_shell: &str, env_shell: Option<String>) -> String {
    let configured_shell = configured_shell.trim();
    if !configured_shell.is_empty() {
        return configured_shell.to_string();
    }

    #[cfg(windows)]
    {
        let _ = env_shell;
        default_pane_shell()
    }

    #[cfg(not(windows))]
    env_shell
        .map(|shell| shell.trim().to_string())
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(default_pane_shell)
}

#[cfg(windows)]
pub(super) fn default_pane_shell() -> String {
    "powershell.exe".into()
}

#[cfg(not(windows))]
pub(super) fn default_pane_shell() -> String {
    "/bin/sh".into()
}

#[derive(Clone, Copy)]
pub(crate) struct PaneShellConfig<'a> {
    pub(crate) default_shell: &'a str,
    pub(crate) mode: crate::utils::config::ShellModeConfig,
}

impl<'a> PaneShellConfig<'a> {
    pub(crate) fn new(default_shell: &'a str, mode: crate::utils::config::ShellModeConfig) -> Self {
        Self {
            default_shell,
            mode,
        }
    }
}

/// Target platform for shell launch policy. Parameterized (instead of raw
/// `cfg!` checks at each decision point) so every branch stays testable on
/// every host platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ShellLaunchTarget {
    Windows,
    Macos,
    OtherUnix,
}

impl ShellLaunchTarget {
    pub(super) fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::OtherUnix
        }
    }
}

pub(super) fn shell_mode_uses_login_shell(
    mode: crate::utils::config::ShellModeConfig,
    target: ShellLaunchTarget,
) -> bool {
    match mode {
        crate::utils::config::ShellModeConfig::Auto => target == ShellLaunchTarget::Macos,
        crate::utils::config::ShellModeConfig::Login => true,
        crate::utils::config::ShellModeConfig::NonLogin => false,
    }
}

pub(super) fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(super) fn resolve_shell_for_login_mode(shell: &str) -> io::Result<String> {
    if shell.contains(std::path::MAIN_SEPARATOR) {
        let path = Path::new(shell);
        return is_executable_file(path)
            .then(|| shell.to_string())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("login shell {shell:?} is not executable"),
                )
            });
    }

    std::env::var_os("PATH")
        .and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(shell))
                .find(|candidate| is_executable_file(candidate))
        })
        .and_then(|path| path.into_os_string().into_string().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("login shell {shell:?} was not found on PATH"),
            )
        })
}

/// Sourced via `-NoExit -Command` when launching PowerShell on Windows. It
/// wraps whatever `prompt` function the user's profile left behind so each
/// prompt render appends the cwd as OSC 9;9 — the sequence Windows Terminal
/// and ConEmu standardized for shell integration. PowerShell never updates
/// its Win32 process cwd on `Set-Location`, so the prompt hook updates it when
/// possible before reporting the cwd.
///
/// The snippet must not contain double quotes: powershell.exe parses its
/// command line with its own rules that disagree with the ArgvQuote escaping
/// portable-pty applies, and embedded `\"` sequences get corrupted in
/// transit. Single-quoted strings and `[char]` codes keep the round-trip
/// byte-exact, and the OSC 9;9 payload is emitted unquoted (the original
/// ConEmu form, which the cwd tracker accepts).
///
/// The original prompt must be invoked before any other statement in the
/// wrapper: anything that runs first resets `$?`, so a status-aware user
/// prompt would show success after a failed command (verified on 5.1).
pub(crate) const WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND: &str = r"if ($null -eq $global:__HerdrOriginalPrompt) { $global:__HerdrOriginalPrompt = $function:prompt; function global:prompt { $out = @(& $global:__HerdrOriginalPrompt) -join ' '; $loc = $ExecutionContext.SessionState.Path.CurrentLocation; if ($loc.Provider.Name -eq 'FileSystem') { try { [Environment]::CurrentDirectory = $loc.ProviderPath } catch {}; $esc = [string][char]27; $out += $esc + ']9;9;' + $loc.ProviderPath + $esc + '\' }; $out } }";

pub(super) fn pane_shell_command_builder_for_target(
    shell_config: PaneShellConfig<'_>,
    target: ShellLaunchTarget,
) -> io::Result<CommandBuilder> {
    let shell = pane_shell(shell_config.default_shell);
    if shell_mode_uses_login_shell(shell_config.mode, target) {
        let mut cmd = CommandBuilder::new_default_prog();
        cmd.env("SHELL", resolve_shell_for_login_mode(&shell)?);
        Ok(cmd)
    } else {
        let mut cmd = CommandBuilder::new(&shell);
        if uses_windows_powershell_pane_shell_for_target(shell_config, target) {
            cmd.args([
                "-NoExit",
                "-Command",
                WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND,
            ]);
        }
        Ok(cmd)
    }
}

pub(super) fn pane_shell_command_builder(
    shell_config: PaneShellConfig<'_>,
) -> io::Result<CommandBuilder> {
    pane_shell_command_builder_for_target(shell_config, ShellLaunchTarget::current())
}

/// True when panes launch an interactive PowerShell directly on Windows.
/// Gates the prompt-based cwd reporting pipeline and the agent-exit shell
/// respawn recovery.
pub(crate) fn uses_windows_powershell_pane_shell(shell_config: PaneShellConfig<'_>) -> bool {
    uses_windows_powershell_pane_shell_for_target(shell_config, ShellLaunchTarget::current())
}

pub(super) fn uses_windows_powershell_pane_shell_for_target(
    shell_config: PaneShellConfig<'_>,
    target: ShellLaunchTarget,
) -> bool {
    target == ShellLaunchTarget::Windows
        && !shell_mode_uses_login_shell(shell_config.mode, target)
        && is_powershell_shell(&pane_shell(shell_config.default_shell))
}

pub(super) fn is_powershell_shell(shell: &str) -> bool {
    // Split on both separators by hand: `Path::file_name` only treats `\` as
    // a separator on Windows hosts, and this predicate must evaluate Windows
    // shell paths correctly from tests on any host.
    // A missing separator leaves the entire name; empty/trailing components stay empty.
    let name = shell
        .rsplit_once(['/', '\\'])
        .map_or(shell, |(_, name)| name)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    )
}

pub(crate) const HERDR_PANE_ID_ENV_VAR: &str = "HERDR_PANE_ID";

pub(crate) const HERDR_TAB_ID_ENV_VAR: &str = "HERDR_TAB_ID";

pub(crate) const HERDR_WORKSPACE_ID_ENV_VAR: &str = "HERDR_WORKSPACE_ID";

pub(crate) fn apply_pane_base_env(cmd: &mut CommandBuilder) {
    cmd.env(
        crate::protocol::api::SOCKET_PATH_ENV_VAR,
        crate::protocol::api::socket_path(),
    );
    if let Ok(executable) = std::env::current_exe() {
        cmd.env("HERDR_BIN_PATH", executable);
    }
}

#[cfg(unix)]
pub(crate) struct SpawnedPty {
    pub master_fd: OwnedFd,
    pub child: Box<dyn Child + Send + Sync>,
}

#[cfg(unix)]
pub(crate) fn spawn_with_portable_pty(
    rows: u16,
    cols: u16,
    cmd: CommandBuilder,
) -> std::io::Result<SpawnedPty> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let master_fd = pair
        .master
        .as_raw_fd()
        .ok_or_else(|| std::io::Error::other("pty master fd is unavailable"))?;
    let actor_fd = fd::duplicate_cloexec_fd(master_fd)?;
    let actor_fd = unsafe { OwnedFd::from_raw_fd(actor_fd) };
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    drop(pair);

    Ok(SpawnedPty {
        master_fd: actor_fd,
        child,
    })
}

#[cfg(windows)]
pub(crate) struct SpawnedPty {
    pub master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

#[cfg(windows)]
pub(crate) fn spawn_with_portable_pty(
    rows: u16,
    cols: u16,
    cmd: CommandBuilder,
) -> std::io::Result<SpawnedPty> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|err| std::io::Error::other(err.to_string()))?;

    Ok(SpawnedPty {
        master: pair.master,
        child,
    })
}

fn initialize_pane_terminal(
    cols: u16,
    rows: u16,
    scrollback_limit_bytes: usize,
    host_terminal_theme: crate::utils::theme::color::TerminalTheme,
    host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
    initial_state: &SpawnInitialState<'_>,
    response_tx: &mpsc::Sender<Bytes>,
) -> io::Result<Arc<PaneTerminal>> {
    let mut terminal = crate::terminal::vt::Terminal::new(cols, rows, scrollback_limit_bytes)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if crate::protocol::kitty::is_enabled() {
        terminal
            .enable_kitty_graphics()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    let pane_terminal = GhosttyPaneTerminal::new(terminal, response_tx.clone())?;
    pane_terminal.apply_host_terminal_theme(host_terminal_theme);
    let _ = pane_terminal.apply_host_terminal_appearance(host_terminal_appearance);
    pane_terminal.set_windows_powershell_prompt_cwd_reporting(
        initial_state.windows_powershell_prompt_cwd_reporting,
    );
    if let Some(ansi) = initial_state.history_ansi {
        pane_terminal.seed_history_ansi(ansi);
    }
    Ok(Arc::new(PaneTerminal::new(pane_terminal)))
}

fn spawn_child_watcher(
    pane_id: PaneId,
    mut child: Box<dyn Child + Send + Sync>,
    child_pid: Arc<AtomicU32>,
    child_wait_completed: Arc<AtomicBool>,
    events: mpsc::Sender<AppEvent>,
) {
    let rt = tokio::runtime::Handle::current();
    if let Some(pid) = child.process_id() {
        child_pid.store(pid, Ordering::Release);
        crate::utils::logging::pane_spawned(pane_id.raw(), pid);
    }
    tokio::task::spawn_blocking(move || {
        let exit_reason = match child.wait() {
            Ok(status) => {
                let exit_reason = crate::platform::classify_child_exit(&status);
                let status_text = format!("{status:?}");
                crate::utils::logging::pane_exited(pane_id.raw(), &status_text);
                exit_reason
            }
            Err(e) => {
                crate::utils::logging::pane_exit_failed(pane_id.raw(), &e.to_string());
                crate::platform::ChildExitReason::WaitFailed
            }
        };
        child_wait_completed.store(true, Ordering::Release);
        // Use blocking send — PaneDied is critical, must not be dropped
        if let Err(e) = rt.block_on(events.send(AppEvent::PaneDied {
            pane_id,
            exit_reason,
        })) {
            error!(pane = pane_id.raw(), err = %e, "failed to send PaneDied event");
        }
    });
}

impl TerminalRuntime {
    // Runtime construction threads PTY geometry, host context, launch policy, and render hooks.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::utils::theme::color::TerminalTheme,
        host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
        shell_config: PaneShellConfig<'_>,
        launch_env: &PaneLaunchEnv,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        Self::spawn_with_initial_history(
            pane_id,
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            shell_config,
            launch_env,
            None,
            events,
            render_notify,
            render_dirty,
        )
    }

    // Runtime construction needs to thread PTY size, environment, theme, and render hooks together.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with_initial_history(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::utils::theme::color::TerminalTheme,
        host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
        shell_config: PaneShellConfig<'_>,
        launch_env: &PaneLaunchEnv,
        initial_history_ansi: Option<&str>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let windows_powershell_prompt_cwd_reporting =
            uses_windows_powershell_pane_shell(shell_config);
        let mut cmd = pane_shell_command_builder(shell_config)?;
        cmd.cwd(cwd);
        apply_pane_terminal_env(&mut cmd);
        apply_pane_launch_env(&mut cmd, launch_env);
        Self::spawn_command_builder(
            pane_id,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
            cmd,
            "failed to spawn shell",
            SpawnInitialState {
                detected_agent: None,
                history_ansi: initial_history_ansi,
                windows_powershell_prompt_cwd_reporting,
            },
            AgentDetection::Enabled,
        )
    }

    /// Test helper: runs `command` through `/bin/sh -c` in a real PTY.
    #[cfg(all(test, unix))]
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_shell_command(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        command: &str,
        launch_env: &PaneLaunchEnv,
        agent_detection: AgentDetection,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::utils::theme::color::TerminalTheme,
        host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let mut cmd = portable_pty::CommandBuilder::from_argv(vec![
            "/bin/sh".into(),
            "-c".into(),
            command.into(),
        ]);
        cmd.cwd(cwd);
        apply_pane_terminal_env(&mut cmd);
        apply_pane_launch_env(&mut cmd, launch_env);
        Self::spawn_command_builder(
            pane_id,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
            cmd,
            "failed to spawn command pane",
            SpawnInitialState::default(),
            agent_detection,
        )
    }

    // Runtime construction needs to thread PTY size, environment, theme, render hooks, and detection policy together.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_argv_command(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        argv: &[String],
        launch_env: &PaneLaunchEnv,
        agent_detection: AgentDetection,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::utils::theme::color::TerminalTheme,
        host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
    ) -> std::io::Result<Self> {
        let Some((program, args)) = argv.split_first() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "argv must not be empty",
            ));
        };
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        apply_pane_terminal_env(&mut cmd);
        apply_pane_launch_env(&mut cmd, launch_env);
        Self::spawn_command_builder(
            pane_id,
            rows,
            cols,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            events,
            render_notify,
            render_dirty,
            cmd,
            "failed to spawn argv command pane",
            SpawnInitialState::default(),
            agent_detection,
        )
    }

    // Runtime construction needs to thread PTY size, environment, theme, render hooks, and detection policy together.
    #[allow(clippy::too_many_arguments)]
    fn spawn_command_builder(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::utils::theme::color::TerminalTheme,
        host_terminal_appearance: Option<crate::utils::theme::color::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
        cmd: CommandBuilder,
        spawn_error_message: &'static str,
        initial_state: SpawnInitialState<'_>,
        agent_detection: AgentDetection,
    ) -> std::io::Result<Self> {
        crate::utils::logging::pane_spawn_started(
            pane_id.raw(),
            rows,
            cols,
            scrollback_limit_bytes,
        );

        let (response_tx, _response_rx) = mpsc::channel::<Bytes>(1);
        let terminal = initialize_pane_terminal(
            cols,
            rows,
            scrollback_limit_bytes,
            host_terminal_theme,
            host_terminal_appearance,
            &initial_state,
            &response_tx,
        )?;
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let kitty_keyboard_flags = Arc::new(AtomicU16::new(0));
        let content_write_lock = Arc::new(Mutex::new(()));

        let spawned = spawn_with_portable_pty(rows, cols, cmd)
            .inspect_err(|err| error!(pane = pane_id.raw(), err = %err, "{spawn_error_message}"))?;

        // --- Child watcher task ---
        let child_pid = Arc::new(AtomicU32::new(0));
        let reported_cwd = Arc::new(Mutex::new(None));
        let child_wait_completed = Arc::new(AtomicBool::new(false));
        let content_seq = Arc::new(AtomicU64::new(0));
        let detection_content_seq = Arc::new(AtomicU64::new(0));
        spawn_child_watcher(
            pane_id,
            spawned.child,
            child_pid.clone(),
            child_wait_completed.clone(),
            events.clone(),
        );

        let io = {
            let terminal = terminal.clone();
            let response_writer = response_tx.clone();
            let render_notify = render_notify.clone();
            let render_dirty = render_dirty.clone();
            let content_seq = content_seq.clone();
            let content_write_lock = content_write_lock.clone();
            let detection_content_seq = detection_content_seq.clone();
            let child_pid = child_pid.clone();
            let events = events.clone();
            let reported_cwd = reported_cwd.clone();
            let compression_wake = compression.notifier();
            let rt = tokio::runtime::Handle::current();
            let on_read = pty_read_callback(PtyReadContext {
                pane_id,
                terminal,
                response_writer,
                render_notify,
                render_dirty,
                content_seq,
                content_write_lock,
                detection_content_seq,
                child_pid,
                events,
                reported_cwd,
                compression_wake,
                rt,
                agent_detection,
            });

            PaneRuntimeIo::Actor(PtyIoActor::spawn(PtyIoActorConfig {
                pane_id: pane_id.raw(),
                #[cfg(unix)]
                master_fd: spawned.master_fd,
                #[cfg(windows)]
                master: spawned.master,
                initially_quiesced: false,
                on_read,
            })?)
        };

        let detect_handle = spawn_detection_task(
            pane_id,
            agent_detection,
            initial_state.detected_agent,
            child_pid.clone(),
            terminal.clone(),
            events.clone(),
            detection_content_seq.clone(),
            render_notify.clone(),
            render_dirty.clone(),
        );

        Ok(Self {
            pane_id,
            terminal,
            io,
            current_size: Cell::new((rows, cols, 0, 0)),
            child_pid,
            reported_cwd,
            child_wait_completed: Some(child_wait_completed),
            kitty_keyboard_flags,
            content_seq,
            content_write_lock,
            detection_content_seq,
            preserve_processes_on_drop: false,
            compression,
            detect_handle,
        })
    }
}
