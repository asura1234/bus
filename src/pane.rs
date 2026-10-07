use std::cell::Cell;
use std::io;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};

use bytes::Bytes;
use portable_pty::CommandBuilder;
#[cfg(all(test, unix))]
use portable_pty::{native_pty_system, PtySize};
use ratatui::{layout::Rect, Frame};
#[cfg(test)]
use tokio::sync::watch;
use tokio::sync::{mpsc, Notify};
use tracing::{error, info, warn};

use crate::detect::{Agent, AgentState};
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::pty::actor::{PtyIoActor, PtyIoActorConfig, PtyIoActorHandle, PtyReadResult};
use crate::render_signal::RenderSignal;

mod agent_detection;
mod cursor;
pub(crate) mod env;
mod input;
mod kitty_keyboard;
mod osc;
mod state;
mod terminal;
mod xtgettcap;

use self::agent_detection::{
    decide_detection_screen_read, decide_screen_detection_publish,
    detection_update_for_publish_with_osc, mark_detection_content_changed,
    observe_detection_content_change, DetectionPublishDecision, DetectionScreenReadDecision,
    DetectionScreenReadInput, PendingIdleConfirmation, ScreenDetectionPublishInput,
    AGENT_PENDING_IDLE_RECHECK, AGENT_STARTUP_GRACE_WINDOW,
};
use self::terminal::{GhosttyPaneTerminal, PaneTerminal};
pub(crate) use self::terminal::{
    TerminalCompressionStep, TerminalDirtyPatch, TerminalDirtyPatchOutcome, TerminalReadSnapshot,
    TerminalSearchDirection, TerminalSearchWindow, TerminalTextPoint, TerminalWordMotion,
};
pub use self::{
    state::PaneState,
    terminal::{ScrollMetrics, TerminalCursorState},
};

const TERMINAL_COMPRESSION_IDLE: std::time::Duration = std::time::Duration::from_millis(250);
const TERMINAL_COMPRESSION_STEP: std::time::Duration = std::time::Duration::from_millis(1);
const PANE_TERM: &str = "xterm-256color";
const PANE_COLORTERM: &str = "truecolor";

fn terminal_compression_permits() -> Arc<tokio::sync::Semaphore> {
    static PERMITS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    PERMITS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
        .clone()
}

fn spawn_blocking_with_compression_permit<T, F>(
    permit: tokio::sync::OwnedSemaphorePermit,
    operation: F,
) -> tokio::task::JoinHandle<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        operation()
    })
}

fn apply_pane_terminal_env(cmd: &mut CommandBuilder) {
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

/// Gap between selection moves and the Enter that confirms them.
const DIALOG_CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// The outcome of answering a choice dialog under the content lock.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DialogChoice {
    /// The keys were queued for the dialog the caller observed.
    Sent(Vec<String>),
    /// The visible dialog no longer matches the caller's observation.
    Stale,
    /// The option does not exist or the current selection is not visible.
    Unreachable,
    /// Text answering is only available on a focused free-text question.
    NotQuestion,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PaneLaunchEnv {
    extra: Vec<(String, String)>,
    identity: PaneLaunchIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum PaneLaunchIdentity {
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

fn apply_pane_launch_env(cmd: &mut CommandBuilder, launch_env: &PaneLaunchEnv) {
    for (key, value) in &launch_env.extra {
        cmd.env(key, value);
    }
    cmd.env(crate::HERDR_ENV_VAR, crate::HERDR_ENV_VALUE);
    self::env::apply_pane_base_env(cmd);
    crate::platform::apply_pane_runtime_marker(cmd);
    match &launch_env.identity {
        PaneLaunchIdentity::Inherit => {}
        PaneLaunchIdentity::Managed {
            workspace_id,
            tab_id,
            pane_id,
        } => {
            cmd.env(self::env::HERDR_WORKSPACE_ID_ENV_VAR, workspace_id);
            cmd.env(self::env::HERDR_TAB_ID_ENV_VAR, tab_id);
            cmd.env(self::env::HERDR_PANE_ID_ENV_VAR, pane_id);
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
struct SpawnInitialState<'a> {
    detected_agent: Option<Agent>,
    history_ansi: Option<&'a str>,
    windows_powershell_prompt_cwd_reporting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentDetection {
    Enabled,
    #[cfg(all(test, unix))]
    Disabled,
}

async fn publish_state_changed_event(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Option<Agent>,
    state: AgentState,
    visible_blocker: bool,
    process_exited: bool,
    observed_at: std::time::Instant,
) {
    // This runs on the async detector task, not the PTY reader thread.
    // Waiting for queue space here preserves correctness-critical state transitions
    // without blocking pane I/O.
    if let Err(e) = state_events
        .send(AppEvent::StateChanged {
            pane_id,
            agent,
            state,
            visible_blocker,
            process_exited,
            observed_at,
        })
        .await
    {
        warn!(
            pane = pane_id.raw(),
            err = %e,
            "failed to deliver StateChanged event"
        );
    }
}

async fn publish_agent_process_detected_event(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Agent,
    observed_at: std::time::Instant,
) {
    if let Err(e) = state_events
        .send(AppEvent::AgentProcessDetected {
            pane_id,
            agent,
            observed_at,
        })
        .await
    {
        warn!(
            pane = pane_id.raw(),
            err = %e,
            "failed to deliver AgentProcessDetected event"
        );
    }
}

#[derive(Debug, Clone, Copy)]
struct AgentDetectionPublishUpdate {
    state: AgentState,
    visible_idle: bool,
    visible_blocker: bool,
    visible_working: bool,
    process_exited: bool,
}

async fn apply_agent_detection_publish_update(
    state_events: mpsc::Sender<AppEvent>,
    pane_id: PaneId,
    agent: Option<Agent>,
    update: AgentDetectionPublishUpdate,
    observed_at: std::time::Instant,
    state: &mut AgentState,
    last_visible_idle: &mut bool,
    last_visible_blocker: &mut bool,
    last_visible_working: &mut bool,
    last_visible_signal_refresh: &mut Option<std::time::Instant>,
    foreground_shell_exit_reported: &mut bool,
) {
    *state = update.state;
    *last_visible_idle = update.visible_idle;
    *last_visible_blocker = update.visible_blocker;
    *last_visible_working = update.visible_working;
    *last_visible_signal_refresh = if update.visible_blocker || update.visible_working {
        Some(observed_at)
    } else {
        None
    };
    if update.process_exited {
        *foreground_shell_exit_reported = true;
    }
    publish_state_changed_event(
        state_events,
        pane_id,
        agent,
        update.state,
        update.visible_blocker,
        update.process_exited,
        observed_at,
    )
    .await;
}

const AGENT_MISS_CONFIRMATION_ATTEMPTS: u8 = 6;
const PROCESS_RECHECK_IDENTIFIED: std::time::Duration = std::time::Duration::from_secs(5);
const PROCESS_RECHECK_MISSING_FOREGROUND_GROUP: std::time::Duration =
    std::time::Duration::from_secs(30);
const PROCESS_ACQUISITION_WINDOW: std::time::Duration = std::time::Duration::from_secs(8);
const PROCESS_ACQUISITION_FAST_WINDOW: std::time::Duration = std::time::Duration::from_millis(1500);
const PROCESS_ACQUISITION_FAST_RECHECK: std::time::Duration = std::time::Duration::from_millis(500);
const PROCESS_ACQUISITION_SLOW_RECHECK: std::time::Duration = std::time::Duration::from_secs(2);
const PROCESS_ACQUISITION_IDLE_RESET: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone, Copy)]
struct AgentDetectionPresence {
    current_agent: Option<Agent>,
    consecutive_misses: u8,
}

#[cfg(unix)]
fn absolute_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    crate::platform::process_cwd(pid).filter(|cwd| cwd.is_absolute())
}

#[cfg(unix)]
fn usable_process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    absolute_process_cwd(pid).filter(|cwd| cwd.is_dir())
}

#[cfg(unix)]
fn foreground_member_cwd_different_from_shell(
    shell_pid: u32,
    shell_cwd: Option<&std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    let job = crate::detect::foreground_job(shell_pid)?;
    for process in job.processes {
        if process.pid == shell_pid {
            continue;
        }
        let Some(cwd) = absolute_process_cwd(process.pid) else {
            continue;
        };
        if shell_cwd != Some(&cwd) {
            return Some(cwd);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForegroundShellAgentAction {
    ObserveProbe,
    ReportProcessExit,
    ReportReplacementProcess,
    ClearAgent,
}

fn foreground_shell_agent_action(
    previous_agent: Option<Agent>,
    new_agent: Option<Agent>,
    foreground_is_pane_shell: bool,
    process_exit_reported: bool,
) -> ForegroundShellAgentAction {
    let Some(previous_agent) = previous_agent else {
        return ForegroundShellAgentAction::ObserveProbe;
    };
    if process_exit_reported {
        return if new_agent == Some(previous_agent) {
            ForegroundShellAgentAction::ReportReplacementProcess
        } else if new_agent.is_none() {
            ForegroundShellAgentAction::ClearAgent
        } else {
            ForegroundShellAgentAction::ObserveProbe
        };
    }
    if new_agent.is_some() {
        return ForegroundShellAgentAction::ObserveProbe;
    }

    if foreground_is_pane_shell {
        // Do not clear identity immediately. First publish an idle process-exit
        // transition for the previous agent so notifications and wait-agent callers
        // observe completion before the pane becomes unknown.
        return ForegroundShellAgentAction::ReportProcessExit;
    }

    ForegroundShellAgentAction::ObserveProbe
}

/// Drops retained OSC evidence when changing away from an identified agent.
/// First acquisition keeps bytes that the newly identified process may have
/// emitted before the process probe recognized it.
fn clear_osc_evidence_for_agent_transition(terminal: &PaneTerminal, previous_agent: Option<Agent>) {
    if previous_agent.is_some() {
        terminal.clear_agent_osc_state();
    }
}

fn apply_foreground_shell_agent_action(
    agent_presence: &mut AgentDetectionPresence,
    action: ForegroundShellAgentAction,
    previous_agent: Option<Agent>,
    new_agent: Option<Agent>,
    pending_foreground_shell_clear: &mut bool,
    foreground_shell_exit_reported: &mut bool,
) -> bool {
    match action {
        ForegroundShellAgentAction::ReportReplacementProcess => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.observe_process_probe(previous_agent);
            true
        }
        ForegroundShellAgentAction::ReportProcessExit => {
            *pending_foreground_shell_clear = true;
            false
        }
        ForegroundShellAgentAction::ClearAgent => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.clear_current_agent()
        }
        ForegroundShellAgentAction::ObserveProbe => {
            *pending_foreground_shell_clear = false;
            *foreground_shell_exit_reported = false;
            agent_presence.observe_process_probe(new_agent)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ProcessProbeInput {
    current_agent: Option<Agent>,
    foreground_pgid: Option<u32>,
    last_foreground_pgid: Option<u32>,
    has_process_probe: bool,
    acquisition_age: Option<std::time::Duration>,
    pending_foreground_shell_clear: bool,
    pending_restore_probe: bool,
    elapsed_since_process_check: std::time::Duration,
}

fn foreground_group_changed(
    foreground_pgid: Option<u32>,
    last_foreground_pgid: Option<u32>,
) -> bool {
    foreground_pgid != last_foreground_pgid
}

// Only kernel-observed foreground groups drive change detection. Remembering an
// inferred group would look like a change on every tick while the kernel stays silent.
fn process_group_for_change_tracking(
    observed_foreground_pgid: Option<u32>,
    probed_process_group_id: Option<u32>,
) -> Option<u32> {
    observed_foreground_pgid?;
    probed_process_group_id.or(observed_foreground_pgid)
}

#[cfg(any(windows, test))]
fn should_observe_foreground_process_group(
    content_changed: bool,
    input: ProcessProbeInput,
) -> bool {
    !input.has_process_probe
        || input.current_agent.is_none()
        || input.pending_foreground_shell_clear
        || input.pending_restore_probe
        || content_changed
        || input.elapsed_since_process_check >= PROCESS_RECHECK_IDENTIFIED
}

fn should_probe_foreground_job(input: ProcessProbeInput) -> bool {
    if input.pending_foreground_shell_clear || input.pending_restore_probe {
        return true;
    }

    let foreground_group_changed =
        foreground_group_changed(input.foreground_pgid, input.last_foreground_pgid);

    if let Some(acquisition_age) = input.acquisition_age {
        let acquisition_interval = if acquisition_age <= PROCESS_ACQUISITION_FAST_WINDOW {
            PROCESS_ACQUISITION_FAST_RECHECK
        } else {
            PROCESS_ACQUISITION_SLOW_RECHECK
        };
        if acquisition_age <= PROCESS_ACQUISITION_WINDOW
            && input.elapsed_since_process_check >= acquisition_interval
        {
            return true;
        }
    }

    if input.current_agent.is_none() {
        return !input.has_process_probe
            || foreground_group_changed
            || (input.foreground_pgid.is_none()
                && input.elapsed_since_process_check >= PROCESS_RECHECK_MISSING_FOREGROUND_GROUP);
    }

    foreground_group_changed || input.elapsed_since_process_check >= PROCESS_RECHECK_IDENTIFIED
}

fn sync_content_change_acquisition(
    current_agent: Option<Agent>,
    process_group_changed: bool,
    content_changed: bool,
    now: std::time::Instant,
    acquisition_started_at: &mut Option<std::time::Instant>,
    last_content_change_at: &mut Option<std::time::Instant>,
) {
    if current_agent.is_some() || process_group_changed {
        return;
    }

    if content_changed {
        let should_start = acquisition_started_at.is_none_or(|started| {
            now.duration_since(started) > PROCESS_ACQUISITION_WINDOW
                && last_content_change_at.is_none_or(|last_change| {
                    now.duration_since(last_change) >= PROCESS_ACQUISITION_IDLE_RESET
                })
        });
        if should_start {
            *acquisition_started_at = Some(now);
        }
        *last_content_change_at = Some(now);
        return;
    }

    let Some(acquisition_started) = *acquisition_started_at else {
        return;
    };
    let Some(last_content_change) = *last_content_change_at else {
        return;
    };

    if now.duration_since(acquisition_started) > PROCESS_ACQUISITION_WINDOW
        && now.duration_since(last_content_change) >= PROCESS_ACQUISITION_IDLE_RESET
    {
        *acquisition_started_at = None;
        *last_content_change_at = None;
    }
}

#[derive(Debug, Clone)]
struct ProcessProbeResult {
    process_group_id: Option<u32>,
    foreground_is_pane_shell: bool,
    agent: Option<Agent>,
    process_name: Option<String>,
}

fn agent_hint_for_foreground_job_members(
    job: &crate::platform::ForegroundJob,
    read_hint: impl Fn(u32) -> Option<Agent>,
) -> Option<Agent> {
    read_hint(job.process_group_id)
        .or_else(|| agent_hint_for_non_leader_foreground_job_members(job, read_hint))
}

fn agent_hint_for_non_leader_foreground_job_members(
    job: &crate::platform::ForegroundJob,
    read_hint: impl Fn(u32) -> Option<Agent>,
) -> Option<Agent> {
    job.processes
        .iter()
        .filter(|process| process.pid != job.process_group_id)
        .find_map(|process| read_hint(process.pid))
}

fn identify_process_group_leader_in_job(
    job: &crate::platform::ForegroundJob,
) -> Option<(Agent, String)> {
    let leader = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)?;
    let leader_job = crate::platform::ForegroundJob {
        process_group_id: job.process_group_id,
        processes: vec![leader.clone()],
    };
    crate::detect::identify_agent_in_job(&leader_job)
}

fn process_probe_result(
    job: &crate::platform::ForegroundJob,
    pid: u32,
    agent: Agent,
    process_name: String,
) -> ProcessProbeResult {
    ProcessProbeResult {
        process_group_id: Some(job.process_group_id),
        foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
        agent: Some(agent),
        process_name: Some(process_name),
    }
}

fn hinted_process_probe_result(
    job: &crate::platform::ForegroundJob,
    pid: u32,
    read_hint: impl Fn(u32) -> Option<Agent>,
) -> Option<ProcessProbeResult> {
    let agent = agent_hint_for_foreground_job_members(job, read_hint)?;
    Some(process_probe_result(
        job,
        pid,
        agent,
        crate::detect::agent_label(agent).to_string(),
    ))
}

fn probe_foreground_process_from_jobs(
    pid: u32,
    foreground_pgid: Option<u32>,
    leader_job: Option<crate::platform::ForegroundJob>,
    foreground_job: impl FnOnce() -> Option<crate::platform::ForegroundJob>,
    read_hint: impl Fn(u32) -> Option<Agent> + Copy,
) -> ProcessProbeResult {
    if let Some(job) = leader_job.as_ref() {
        if let Some(hinted) = hinted_process_probe_result(job, pid, read_hint) {
            return hinted;
        }
        if let Some((agent, process_name)) = crate::detect::identify_agent_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
    }

    let foreground_job = foreground_job();
    if let Some(job) = foreground_job.as_ref() {
        if let Some(agent) = read_hint(job.process_group_id) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::detect::agent_label(agent).to_string(),
            );
        }
        if let Some((agent, process_name)) = identify_process_group_leader_in_job(job) {
            return process_probe_result(job, pid, agent, process_name);
        }
        if let Some(agent) = agent_hint_for_non_leader_foreground_job_members(job, read_hint) {
            return process_probe_result(
                job,
                pid,
                agent,
                crate::detect::agent_label(agent).to_string(),
            );
        }

        let identified = crate::detect::identify_agent_in_job(job);
        return ProcessProbeResult {
            process_group_id: Some(job.process_group_id),
            foreground_is_pane_shell: job.processes.iter().any(|process| process.pid == pid),
            agent: identified.as_ref().map(|(agent, _)| *agent),
            process_name: identified.map(|(_, process_name)| process_name),
        };
    }

    ProcessProbeResult {
        process_group_id: foreground_pgid,
        foreground_is_pane_shell: false,
        agent: None,
        process_name: None,
    }
}

fn probe_foreground_process(pid: u32, foreground_pgid: Option<u32>) -> ProcessProbeResult {
    probe_foreground_process_from_jobs(
        pid,
        foreground_pgid,
        foreground_pgid.and_then(crate::detect::foreground_group_leader_job),
        || crate::detect::foreground_job(pid),
        crate::platform::process_agent_hint,
    )
}

impl AgentDetectionPresence {
    fn from_agent(current_agent: Option<Agent>) -> Self {
        Self {
            current_agent,
            consecutive_misses: 0,
        }
    }

    fn current_agent(&self) -> Option<Agent> {
        self.current_agent
    }

    fn clear_current_agent(&mut self) -> bool {
        if self.current_agent.is_none() {
            self.consecutive_misses = 0;
            return false;
        }
        self.current_agent = None;
        self.consecutive_misses = 0;
        true
    }

    fn observe_process_probe(&mut self, identified_agent: Option<Agent>) -> bool {
        match identified_agent {
            Some(agent) => {
                self.consecutive_misses = 0;
                if Some(agent) == self.current_agent {
                    return false;
                }
                self.current_agent = Some(agent);
                true
            }
            None => {
                if self.current_agent.is_none() {
                    self.consecutive_misses = 0;
                    return false;
                }
                self.consecutive_misses = self.consecutive_misses.saturating_add(1);
                if self.consecutive_misses < AGENT_MISS_CONFIRMATION_ATTEMPTS {
                    return false;
                }
                self.current_agent = None;
                self.consecutive_misses = 0;
                true
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PaneRuntime — PTY, parser, channels, background tasks
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct TerminalCompressionWake {
    notify: Arc<Notify>,
    generation: Arc<AtomicU64>,
}

impl TerminalCompressionWake {
    fn wake(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.notify.notify_one();
    }
}

/// Drives libghostty-vt's caller-owned compression after terminal activity settles.
struct TerminalCompressionTask {
    wake: TerminalCompressionWake,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    completed_passes: Arc<AtomicU64>,
    handle: tokio::task::AbortHandle,
}

impl Drop for TerminalCompressionTask {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl TerminalCompressionTask {
    fn spawn(pane_id: PaneId, terminal: Arc<PaneTerminal>) -> Self {
        let wake = TerminalCompressionWake {
            notify: Arc::new(Notify::new()),
            generation: Arc::new(AtomicU64::new(0)),
        };
        let task_notify = wake.notify.clone();
        let task_generation = wake.generation.clone();
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let completed_passes = Arc::new(AtomicU64::new(0));
        #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
        let task_completed_passes = completed_passes.clone();
        let handle = tokio::spawn(async move {
            run_terminal_compression_task(
                pane_id,
                terminal,
                task_notify,
                task_generation,
                #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                task_completed_passes,
            )
            .await;
        })
        .abort_handle();
        Self {
            wake,
            #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
            completed_passes,
            handle,
        }
    }

    fn wake(&self) {
        self.wake.wake();
    }

    fn notifier(&self) -> TerminalCompressionWake {
        self.wake.clone()
    }

    fn abort(&self) {
        self.handle.abort();
    }

    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
    fn completed_passes(&self) -> u64 {
        self.completed_passes.load(Ordering::Acquire)
    }
}

async fn run_terminal_compression_task(
    pane_id: PaneId,
    terminal: Arc<PaneTerminal>,
    notify: Arc<Notify>,
    generation: Arc<AtomicU64>,
    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))] completed_passes: Arc<
        AtomicU64,
    >,
) {
    let mut observed_generation = generation.load(Ordering::Acquire);
    let mut activity = loop {
        match terminal.try_compression_activity() {
            Ok(Some(activity)) => break activity,
            Ok(None) => tokio::time::sleep(TERMINAL_COMPRESSION_IDLE).await,
            Err(err) => {
                warn!(pane = pane_id.raw(), err = %err, "failed to read terminal compression activity");
                return;
            }
        }
    };

    'schedule: loop {
        loop {
            tokio::time::sleep(TERMINAL_COMPRESSION_IDLE).await;
            let current = match terminal.try_compression_activity() {
                Ok(Some(current)) => current,
                Ok(None) => continue,
                Err(err) => {
                    warn!(pane = pane_id.raw(), err = %err, "failed to read terminal compression activity");
                    return;
                }
            };
            let current_generation = generation.load(Ordering::Acquire);
            if activity == current && observed_generation == current_generation {
                break;
            }
            activity = current;
            observed_generation = current_generation;
        }

        loop {
            let current_generation = generation.load(Ordering::Acquire);
            if observed_generation != current_generation {
                observed_generation = current_generation;
                continue 'schedule;
            }

            let permit = match terminal_compression_permits().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => return,
            };
            let current_generation = generation.load(Ordering::Acquire);
            if observed_generation != current_generation {
                observed_generation = current_generation;
                continue 'schedule;
            }

            let terminal_for_step = terminal.clone();
            let step = spawn_blocking_with_compression_permit(permit, move || {
                terminal_for_step.try_compress_incremental_if_activity(activity)
            })
            .await;
            let step = match step {
                Ok(Ok(step)) => step,
                Ok(Err(err)) => {
                    warn!(pane = pane_id.raw(), err = %err, "failed to compress terminal scrollback");
                    return;
                }
                Err(err) => {
                    warn!(pane = pane_id.raw(), err = %err, "terminal compression worker failed");
                    return;
                }
            };

            match step {
                TerminalCompressionStep::Busy => continue 'schedule,
                TerminalCompressionStep::ActivityChanged(current) => {
                    activity = current;
                    observed_generation = generation.load(Ordering::Acquire);
                    continue 'schedule;
                }
                TerminalCompressionStep::Compressed(
                    crate::ghostty::TerminalCompressionResult::Unsupported,
                ) => return,
                TerminalCompressionStep::Compressed(
                    crate::ghostty::TerminalCompressionResult::Pending,
                ) => tokio::time::sleep(TERMINAL_COMPRESSION_STEP).await,
                TerminalCompressionStep::Compressed(
                    crate::ghostty::TerminalCompressionResult::Complete,
                ) => {
                    #[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
                    completed_passes.fetch_add(1, Ordering::Release);
                    loop {
                        notify.notified().await;
                        let current_generation = generation.load(Ordering::Acquire);
                        if observed_generation != current_generation {
                            observed_generation = current_generation;
                            continue 'schedule;
                        }
                    }
                }
            }
        }
    }
}

/// PTY runtime for a pane. Owns the terminal, I/O channels, and background tasks.
/// Dropping this aborts async tasks and closes the PTY. An already-running bounded
/// compression step may finish before releasing its terminal reference.
pub struct PaneRuntime {
    pane_id: PaneId,
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    current_size: Cell<(u16, u16, u32, u32)>,
    child_pid: Arc<AtomicU32>,
    reported_cwd: Arc<Mutex<Option<std::path::PathBuf>>>,
    child_wait_completed: Option<Arc<AtomicBool>>,
    kitty_keyboard_flags: Arc<AtomicU16>,
    content_seq: Arc<AtomicU64>,
    content_write_lock: Arc<Mutex<()>>,
    detection_content_seq: Arc<AtomicU64>,
    preserve_processes_on_drop: bool,
    // Task handles for deterministic shutdown
    compression: TerminalCompressionTask,
    detect_handle: Option<tokio::task::AbortHandle>,
}

enum PaneRuntimeIo {
    Actor(PtyIoActorHandle),
    #[cfg(test)]
    TestChannel {
        sender: mpsc::Sender<Bytes>,
        resize_tx: watch::Sender<(u16, u16, u32, u32)>,
    },
}

impl PaneRuntimeIo {
    fn shutdown(&self) {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.shutdown(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => {}
        }
    }

    #[cfg(unix)]
    fn foreground_process_group_id(&self) -> Option<u32> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.foreground_process_group_id(),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { .. } => None,
        }
    }

    fn resize(
        &self,
        rows: u16,
        cols: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        terminal_responses: Vec<Bytes>,
    ) {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                actor.resize(
                    rows,
                    cols,
                    cell_width_px,
                    cell_height_px,
                    terminal_responses,
                );
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { resize_tx, .. } => {
                let _ = resize_tx.send((rows, cols, cell_width_px, cell_height_px));
            }
        }
    }

    fn try_send_bytes(&self, bytes: Bytes) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.try_write_user_input(bytes),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => sender.try_send(bytes),
        }
    }

    fn write_terminal_response(&self, response: impl FnOnce() -> Option<Bytes>) {
        match self {
            PaneRuntimeIo::Actor(actor) => actor.write_terminal_response(response),
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => {
                if let Some(bytes) = response() {
                    let _ = sender.try_send(bytes);
                }
            }
        }
    }

    fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        match self {
            PaneRuntimeIo::Actor(actor) => {
                #[cfg(windows)]
                return actor.queue_user_input_submission(text, enter, delay, deadline);
                #[cfg(unix)]
                {
                    let _ = deadline;
                    actor.queue_user_input_submission(text, enter, delay)
                }
            }
            #[cfg(test)]
            PaneRuntimeIo::TestChannel { sender, .. } => {
                let _ = deadline;
                let sender = sender.clone();
                let (reply_tx, reply_rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let result = sender
                        .try_send(text)
                        .map_err(std::io::Error::other)
                        .and_then(|()| {
                            std::thread::sleep(delay);
                            sender.try_send(enter).map_err(std::io::Error::other)
                        });
                    let _ = reply_tx.send(result);
                });
                Ok(reply_rx)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelRouting {
    HostScroll,
    MouseReport,
    AlternateScroll,
}

impl Drop for PaneRuntime {
    fn drop(&mut self) {
        // Abort detection task immediately and terminate the owned session.
        // The PTY actor shuts down before the process/session policy runs.
        if let Some(handle) = &self.detect_handle {
            handle.abort();
        }
        self.compression.abort();
        self.io.shutdown();
        if !self.preserve_processes_on_drop {
            shutdown_pane_processes(
                self.pane_id,
                self.child_pid.load(Ordering::Acquire),
                self.child_wait_completed.as_deref(),
            );
        }
    }
}

fn process_alive_for_shutdown(
    pid: u32,
    child_pid: u32,
    child_wait_completed: bool,
    process_exists: impl FnOnce(u32) -> bool,
) -> bool {
    if pid == child_pid && child_wait_completed {
        return false;
    }
    process_exists(pid)
}

fn wait_for_processes_to_exit(
    pids: &[u32],
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
    timeout: std::time::Duration,
) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let child_wait_completed =
            child_wait_completed.is_some_and(|flag| flag.load(Ordering::Acquire));
        if pids.iter().all(|pid| {
            !process_alive_for_shutdown(
                *pid,
                child_pid,
                child_wait_completed,
                crate::platform::process_exists,
            )
        }) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn shutdown_pane_processes(
    pane_id: PaneId,
    child_pid: u32,
    child_wait_completed: Option<&AtomicBool>,
) -> bool {
    if child_pid == 0 {
        return true;
    }

    let mut pids = crate::platform::session_processes(child_pid);
    if pids.is_empty() {
        pids.push(child_pid);
    }
    pids.sort_unstable();
    pids.dedup();

    for (signal, grace) in [
        (
            crate::platform::Signal::Hangup,
            std::time::Duration::from_millis(250),
        ),
        (
            crate::platform::Signal::Terminate,
            std::time::Duration::from_millis(250),
        ),
        (
            crate::platform::Signal::Kill,
            std::time::Duration::from_millis(250),
        ),
    ] {
        crate::platform::signal_processes(&pids, signal);
        if wait_for_processes_to_exit(&pids, child_pid, child_wait_completed, grace) {
            info!(
                pane = pane_id.raw(),
                pid = child_pid,
                ?signal,
                "pane session terminated"
            );
            return true;
        }
    }

    warn!(
        pane = pane_id.raw(),
        pid = child_pid,
        pids = ?pids,
        "pane session still alive after forced shutdown"
    );
    false
}

fn pane_shell(configured_shell: &str) -> String {
    pane_shell_from(configured_shell, std::env::var("SHELL").ok())
}

fn pane_shell_from(configured_shell: &str, env_shell: Option<String>) -> String {
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
fn default_pane_shell() -> String {
    "powershell.exe".into()
}

#[cfg(not(windows))]
fn default_pane_shell() -> String {
    "/bin/sh".into()
}

#[derive(Clone, Copy)]
pub(crate) struct PaneShellConfig<'a> {
    pub(crate) default_shell: &'a str,
    pub(crate) mode: crate::config::ShellModeConfig,
}

impl<'a> PaneShellConfig<'a> {
    pub(crate) fn new(default_shell: &'a str, mode: crate::config::ShellModeConfig) -> Self {
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
enum ShellLaunchTarget {
    Windows,
    Macos,
    OtherUnix,
}

impl ShellLaunchTarget {
    fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::OtherUnix
        }
    }
}

fn shell_mode_uses_login_shell(
    mode: crate::config::ShellModeConfig,
    target: ShellLaunchTarget,
) -> bool {
    match mode {
        crate::config::ShellModeConfig::Auto => target == ShellLaunchTarget::Macos,
        crate::config::ShellModeConfig::Login => true,
        crate::config::ShellModeConfig::NonLogin => false,
    }
}

fn is_executable_file(path: &Path) -> bool {
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

fn resolve_shell_for_login_mode(shell: &str) -> io::Result<String> {
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

fn pane_shell_command_builder_for_target(
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

fn pane_shell_command_builder(shell_config: PaneShellConfig<'_>) -> io::Result<CommandBuilder> {
    pane_shell_command_builder_for_target(shell_config, ShellLaunchTarget::current())
}

/// True when panes launch an interactive PowerShell directly on Windows.
/// Gates the prompt-based cwd reporting pipeline and the agent-exit shell
/// respawn recovery.
pub(crate) fn uses_windows_powershell_pane_shell(shell_config: PaneShellConfig<'_>) -> bool {
    uses_windows_powershell_pane_shell_for_target(shell_config, ShellLaunchTarget::current())
}

fn uses_windows_powershell_pane_shell_for_target(
    shell_config: PaneShellConfig<'_>,
    target: ShellLaunchTarget,
) -> bool {
    target == ShellLaunchTarget::Windows
        && !shell_mode_uses_login_shell(shell_config.mode, target)
        && is_powershell_shell(&pane_shell(shell_config.default_shell))
}

fn is_powershell_shell(shell: &str) -> bool {
    // Split on both separators by hand: `Path::file_name` only treats `\` as
    // a separator on Windows hosts, and this predicate must evaluate Windows
    // shell paths correctly from tests on any host.
    let name = shell
        .rsplit(['/', '\\'])
        .next()
        .unwrap()
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "powershell" | "powershell.exe" | "pwsh" | "pwsh.exe"
    )
}

fn usable_reported_cwd(cwd: std::path::PathBuf) -> Option<std::path::PathBuf> {
    (cwd.is_absolute() && cwd.is_dir()).then_some(cwd)
}

fn publish_terminal_bells(pane_id: PaneId, count: u16, events: &mpsc::Sender<AppEvent>) {
    if count == 0 {
        return;
    }
    if let Err(err) = events.try_send(AppEvent::TerminalBell { pane_id, count }) {
        warn!(
            pane = pane_id.raw(),
            count,
            err = %err,
            "failed to queue terminal bell"
        );
    }
}

fn publish_reported_cwd(
    pane_id: PaneId,
    cwd: std::path::PathBuf,
    reported_cwd: &Arc<Mutex<Option<std::path::PathBuf>>>,
    events: &mpsc::Sender<AppEvent>,
) {
    let Some(cwd) = usable_reported_cwd(cwd) else {
        return;
    };
    if let Ok(mut current) = reported_cwd.lock() {
        if current.as_ref() == Some(&cwd) {
            return;
        }
        *current = Some(cwd.clone());
    }
    if let Err(err) = events.try_send(AppEvent::TerminalCwdReported { pane_id, cwd }) {
        warn!(
            pane = pane_id.raw(),
            err = %err,
            "failed to send terminal cwd report"
        );
    }
}

impl PaneRuntime {
    pub fn shutdown(mut self) {
        if let Some(handle) = self.detect_handle.take() {
            handle.abort();
        }
        self.compression.abort();
        self.io.shutdown();
        shutdown_pane_processes(
            self.pane_id,
            self.child_pid.load(Ordering::Acquire),
            self.child_wait_completed.as_deref(),
        );
        self.preserve_processes_on_drop = true;
    }

    /// Checked shutdown for destructive API operations. Preserve the runtime
    /// for retry if a native process remains alive after the normal escalation.
    pub fn stop_session_for_close(&self) -> bool {
        let stopped = shutdown_pane_processes(
            self.pane_id,
            self.child_pid.load(Ordering::Acquire),
            self.child_wait_completed.as_deref(),
        );
        if stopped {
            // The subsequent normal close must not signal a recycled process id.
            self.child_pid.store(0, Ordering::Release);
        }
        stopped
    }

    pub fn apply_host_terminal_theme(&self, theme: crate::terminal_theme::TerminalTheme) {
        self.terminal.apply_host_terminal_theme(theme);
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::terminal_theme::HostAppearance>,
    ) {
        self.io
            .write_terminal_response(|| self.terminal.apply_host_terminal_appearance(appearance));
    }

    // Runtime construction threads PTY geometry, host context, launch policy, and render hooks.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
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
    pub(crate) fn spawn_with_initial_history(
        pane_id: PaneId,
        rows: u16,
        cols: u16,
        cwd: std::path::PathBuf,
        scrollback_limit_bytes: usize,
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
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
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
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
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
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
        host_terminal_theme: crate::terminal_theme::TerminalTheme,
        host_terminal_appearance: Option<crate::terminal_theme::HostAppearance>,
        events: mpsc::Sender<AppEvent>,
        render_notify: Arc<Notify>,
        render_dirty: Arc<RenderSignal>,
        cmd: CommandBuilder,
        spawn_error_message: &'static str,
        initial_state: SpawnInitialState<'_>,
        agent_detection: AgentDetection,
    ) -> std::io::Result<Self> {
        crate::logging::pane_spawn_started(pane_id.raw(), rows, cols, scrollback_limit_bytes);

        let (response_tx, _response_rx) = mpsc::channel::<Bytes>(1);
        let mut terminal = crate::ghostty::Terminal::new(cols, rows, scrollback_limit_bytes)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        if crate::kitty_graphics::is_enabled() {
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
        let terminal = Arc::new(PaneTerminal::new(pane_terminal));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let kitty_keyboard_flags = Arc::new(AtomicU16::new(0));
        let content_write_lock = Arc::new(Mutex::new(()));

        let spawned = crate::pty::backend::spawn_with_portable_pty(rows, cols, cmd)
            .inspect_err(|err| error!(pane = pane_id.raw(), err = %err, "{spawn_error_message}"))?;

        // --- Child watcher task ---
        let child_pid = Arc::new(AtomicU32::new(0));
        let reported_cwd = Arc::new(Mutex::new(None));
        let child_wait_completed = Arc::new(AtomicBool::new(false));
        let content_seq = Arc::new(AtomicU64::new(0));
        let detection_content_seq = Arc::new(AtomicU64::new(0));
        {
            let child_pid = child_pid.clone();
            let child_wait_completed = child_wait_completed.clone();
            let events = events.clone();
            let rt = tokio::runtime::Handle::current();
            let mut child = spawned.child;
            if let Some(pid) = child.process_id() {
                child_pid.store(pid, Ordering::Release);
                crate::logging::pane_spawned(pane_id.raw(), pid);
            }
            tokio::task::spawn_blocking(move || {
                let exit_reason = match child.wait() {
                    Ok(status) => {
                        let exit_reason = crate::platform::classify_child_exit(&status);
                        let status_text = format!("{status:?}");
                        crate::logging::pane_exited(pane_id.raw(), &status_text);
                        exit_reason
                    }
                    Err(e) => {
                        crate::logging::pane_exit_failed(pane_id.raw(), &e.to_string());
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
            let on_read = Box::new(move |bytes: &[u8]| {
                let _content_write_guard = match content_write_lock.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                content_seq.fetch_add(1, Ordering::AcqRel);
                let shell_pid = child_pid.load(Ordering::Acquire);
                let result =
                    terminal.process_pty_bytes(pane_id, shell_pid, bytes, &response_writer);
                content_seq.fetch_add(1, Ordering::Release);
                drop(_content_write_guard);
                compression_wake.wake();
                publish_terminal_bells(pane_id, result.terminal_bells, &events);
                if agent_detection == AgentDetection::Enabled {
                    observe_detection_content_change(bytes, &detection_content_seq);
                }
                let title_requested =
                    result.terminal_title_changed && render_dirty.request_terminal_title(pane_id);
                let render_requested = result.request_render && render_dirty.request_pty(pane_id);
                if title_requested || render_requested {
                    render_notify.notify_one();
                }
                if let Some(delay) = result.render_delay {
                    let render_notify = render_notify.clone();
                    let render_dirty = render_dirty.clone();
                    rt.spawn(async move {
                        tokio::time::sleep(delay).await;
                        if render_dirty.request_pty(pane_id) {
                            render_notify.notify_one();
                        }
                    });
                }
                if let Some(cwd) = result.reported_cwd.clone() {
                    publish_reported_cwd(pane_id, cwd, &reported_cwd, &events);
                }
                for content in result.clipboard_writes {
                    if let Err(err) = events.try_send(AppEvent::ClipboardWrite { content }) {
                        warn!(
                            pane = pane_id.raw(),
                            err = %err,
                            "failed to send OSC 52 clipboard write"
                        );
                    }
                }
                PtyReadResult {
                    terminal_responses: result.terminal_responses,
                }
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

        // --- Detection task ---
        let detect_handle = if agent_detection == AgentDetection::Enabled {
            use crate::detect;
            use std::time::{Duration, Instant};

            const TICK_UNIDENTIFIED: Duration = Duration::from_millis(500);
            const TICK_IDENTIFIED: Duration = Duration::from_millis(300);
            const TICK_TRANSIENT_COLOR_OVERRIDE: Duration = Duration::from_millis(50);

            let child_pid = child_pid.clone();
            let terminal = terminal.clone();
            let state_events = events.clone();
            let detection_content_seq = detection_content_seq.clone();
            let render_notify = render_notify.clone();
            let render_dirty = render_dirty.clone();

            let handle = tokio::spawn(async move {
                let mut agent_presence =
                    AgentDetectionPresence::from_agent(initial_state.detected_agent);
                let mut state = AgentState::Idle;
                let mut last_visible_idle = initial_state.detected_agent.is_some();
                let mut last_process_check = Instant::now();
                #[cfg(windows)]
                let mut last_observation = (Instant::now(), Some(0));
                let mut last_foreground_pgid = None;
                let mut has_process_probe = false;
                let mut acquisition_started_at = None;
                let mut last_content_change_at = None;
                let mut pending_foreground_shell_clear = false;
                let mut foreground_shell_exit_reported = false;
                let mut pending_restore_probe = initial_state.detected_agent.is_some();
                let mut last_visible_blocker = false;
                let mut last_visible_working = false;
                let mut last_visible_signal_refresh = None;
                let mut last_detection_text = String::new();
                let mut last_screen_scan_detection_content_seq = None;
                let mut agent_startup_grace_until = None;
                let mut pending_idle = PendingIdleConfirmation::default();

                tokio::time::sleep(Duration::from_millis(50)).await;

                loop {
                    let tick = if terminal.has_transient_default_color_override() {
                        TICK_TRANSIENT_COLOR_OVERRIDE
                    } else if pending_idle.active() {
                        AGENT_PENDING_IDLE_RECHECK
                    } else if agent_presence.current_agent().is_none() {
                        TICK_UNIDENTIFIED
                    } else {
                        TICK_IDENTIFIED
                    };
                    tokio::time::sleep(tick).await;

                    let now = Instant::now();
                    let pid = child_pid.load(Ordering::Acquire);
                    let mut agent = agent_presence.current_agent();
                    let process_probe_input = ProcessProbeInput {
                        current_agent: agent,
                        foreground_pgid: last_foreground_pgid,
                        last_foreground_pgid,
                        has_process_probe,
                        acquisition_age: acquisition_started_at
                            .map(|started| now.duration_since(started)),
                        pending_foreground_shell_clear,
                        pending_restore_probe,
                        elapsed_since_process_check: now.duration_since(last_process_check),
                    };
                    #[cfg(windows)]
                    let content_seq = detection_content_seq.load(Ordering::Relaxed);
                    #[cfg(windows)]
                    let last_content_seq = last_observation.1;
                    #[cfg(windows)]
                    let foreground_observation_due = should_observe_foreground_process_group(
                        last_content_seq != Some(content_seq)
                            && (last_content_seq.is_some()
                                || now.duration_since(last_observation.0) >= TICK_IDENTIFIED),
                        process_probe_input,
                    );
                    #[cfg(not(windows))]
                    let foreground_observation_due = true;
                    let foreground_pgid = match (pid, foreground_observation_due) {
                        (0, _) => None,
                        (_, true) => detect::foreground_process_group_id(pid),
                        _ => last_foreground_pgid,
                    };
                    #[cfg(windows)]
                    if pid > 0 && foreground_observation_due {
                        let retry =
                            last_content_seq.is_some() && last_content_seq != Some(content_seq);
                        last_observation = (now, (!retry).then_some(content_seq));
                    }
                    let process_group_changed =
                        foreground_group_changed(foreground_pgid, last_foreground_pgid);
                    let should_check_process = pid > 0 && {
                        let process_probe_input = ProcessProbeInput {
                            foreground_pgid,
                            ..process_probe_input
                        };
                        should_probe_foreground_job(process_probe_input)
                    };

                    let mut agent_changed = false;
                    if should_check_process {
                        last_process_check = now;
                        let had_process_probe = has_process_probe;
                        has_process_probe = true;
                        if pid > 0 {
                            let probe = probe_foreground_process(pid, foreground_pgid);
                            let process_name = probe.process_name;
                            let process_group_id = probe.process_group_id;
                            let tracked_process_group_id = process_group_for_change_tracking(
                                foreground_pgid,
                                process_group_id,
                            );
                            let foreground_is_pane_shell = probe.foreground_is_pane_shell;
                            let new_agent = probe.agent;

                            let previous_agent = agent_presence.current_agent();
                            let foreground_action = foreground_shell_agent_action(
                                previous_agent,
                                new_agent,
                                foreground_is_pane_shell,
                                foreground_shell_exit_reported,
                            );
                            let changed = apply_foreground_shell_agent_action(
                                &mut agent_presence,
                                foreground_action,
                                previous_agent,
                                new_agent,
                                &mut pending_foreground_shell_clear,
                                &mut foreground_shell_exit_reported,
                            );
                            last_foreground_pgid = tracked_process_group_id;
                            if new_agent.is_some() {
                                acquisition_started_at = None;
                                last_content_change_at = None;
                            } else if agent_presence.current_agent().is_none()
                                && had_process_probe
                                && process_group_changed
                            {
                                acquisition_started_at = Some(now);
                            }
                            pending_restore_probe = false;
                            if changed {
                                agent = agent_presence.current_agent();
                                if agent != previous_agent
                                    || foreground_action
                                        == ForegroundShellAgentAction::ReportReplacementProcess
                                {
                                    pending_idle.clear();
                                    last_screen_scan_detection_content_seq = None;
                                    // A replacement agent must not inherit OSC
                                    // evidence from the previous process; a first
                                    // acquisition keeps the evidence its own
                                    // process already emitted.
                                    clear_osc_evidence_for_agent_transition(
                                        &terminal,
                                        previous_agent,
                                    );
                                    if let Some(agent) = agent {
                                        agent_startup_grace_until =
                                            Some(now + AGENT_STARTUP_GRACE_WINDOW);
                                        state = AgentState::Unknown;
                                        last_visible_idle = false;
                                        last_visible_blocker = false;
                                        last_visible_working = false;
                                        last_visible_signal_refresh = None;
                                        publish_agent_process_detected_event(
                                            state_events.clone(),
                                            pane_id,
                                            agent,
                                            now,
                                        )
                                        .await;
                                    } else {
                                        agent_startup_grace_until = None;
                                    }
                                }
                                if let Some(process_name) = process_name {
                                    info!(
                                        pane = pane_id.raw(),
                                        previous_agent = ?previous_agent,
                                        ?agent,
                                        process = %process_name,
                                        pgid = ?process_group_id,
                                        "agent changed"
                                    );
                                } else {
                                    info!(
                                        pane = pane_id.raw(),
                                        previous_agent = ?previous_agent,
                                        ?agent,
                                        pgid = ?process_group_id,
                                        "agent changed"
                                    );
                                }
                                agent_changed = true;
                            }
                        }
                    }

                    let pid = child_pid.load(Ordering::Acquire);
                    // Keep the terminal restore side effect separate from render notification state.
                    #[allow(clippy::collapsible_if)]
                    if pid > 0 && terminal.maybe_restore_host_terminal_theme(pane_id, pid) {
                        if render_dirty.request_pty(pane_id) {
                            render_notify.notify_one();
                        }
                    }

                    let process_exited = pending_foreground_shell_clear
                        && agent.is_some()
                        && !foreground_shell_exit_reported;

                    if let Some(until) = agent_startup_grace_until {
                        if process_exited {
                            agent_startup_grace_until = None;
                            last_screen_scan_detection_content_seq = None;
                            pending_idle.clear();
                        } else {
                            if now < until {
                                pending_idle.clear();
                                continue;
                            }
                            agent_startup_grace_until = None;
                            pending_idle.clear();
                            continue;
                        }
                    }

                    let current_detection_content_seq = if agent.is_some() {
                        Some(detection_content_seq.load(Ordering::Relaxed))
                    } else {
                        None
                    };
                    match decide_detection_screen_read(DetectionScreenReadInput {
                        state,
                        agent,
                        pending_idle_active: pending_idle.active(),
                        agent_changed,
                        process_exited,
                        current_detection_content_seq,
                        last_screen_scan_detection_content_seq,
                    }) {
                        DetectionScreenReadDecision::Read => {}
                        DetectionScreenReadDecision::Skip => continue,
                    }

                    let content = terminal.detection_text();
                    last_screen_scan_detection_content_seq = current_detection_content_seq;
                    let content_changed = content != last_detection_text;
                    last_detection_text.clone_from(&content);
                    if detect::should_skip_state_update(agent, &content) {
                        pending_idle.clear();
                        continue;
                    }
                    sync_content_change_acquisition(
                        agent_presence.current_agent(),
                        process_group_changed,
                        content_changed,
                        now,
                        &mut acquisition_started_at,
                        &mut last_content_change_at,
                    );

                    let osc_title = terminal.agent_osc_title();
                    let osc_progress = terminal.agent_osc_progress();
                    let Some(screen_detection) = detection_update_for_publish_with_osc(
                        agent,
                        &content,
                        &osc_title,
                        &osc_progress,
                        process_exited,
                    ) else {
                        pending_idle.clear();
                        continue;
                    };
                    match decide_screen_detection_publish(
                        ScreenDetectionPublishInput {
                            screen_detection,
                            current_state: state,
                            last_visible_idle,
                            last_visible_blocker,
                            last_visible_working,
                            last_visible_signal_refresh,
                            process_exited,
                            agent_changed,
                            now,
                        },
                        &mut pending_idle,
                    ) {
                        DetectionPublishDecision::NoPublish => {}
                        DetectionPublishDecision::Publish {
                            state: new_state,
                            visible_idle,
                            visible_blocker,
                            visible_working,
                            process_exited: publish_process_exited,
                        } => {
                            apply_agent_detection_publish_update(
                                state_events.clone(),
                                pane_id,
                                agent,
                                AgentDetectionPublishUpdate {
                                    state: new_state,
                                    visible_idle,
                                    visible_blocker,
                                    visible_working,
                                    process_exited: publish_process_exited,
                                },
                                now,
                                &mut state,
                                &mut last_visible_idle,
                                &mut last_visible_blocker,
                                &mut last_visible_working,
                                &mut last_visible_signal_refresh,
                                &mut foreground_shell_exit_reported,
                            )
                            .await;
                        }
                    }
                }
            });
            Some(handle.abort_handle())
        } else {
            None
        };

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

    pub(crate) fn current_size(&self) -> (u16, u16) {
        let (rows, cols, _, _) = self.current_size.get();
        (rows, cols)
    }

    pub(crate) fn content_seq(&self) -> u64 {
        self.content_seq.load(Ordering::Acquire)
    }

    /// Resize if the dimensions actually changed.
    pub fn resize(&self, rows: u16, cols: u16, cell_width_px: u32, cell_height_px: u32) {
        let rows = rows.max(2);
        let cols = cols.max(4);
        let size = (rows, cols, cell_width_px, cell_height_px);
        if self.current_size.get() == size {
            return;
        }
        self.current_size.set(size);
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.content_seq.fetch_add(1, Ordering::AcqRel);
        let terminal_responses = self
            .terminal
            .resize(rows, cols, cell_width_px, cell_height_px);
        self.content_seq.fetch_add(1, Ordering::Release);
        drop(_content_write_guard);
        self.compression.wake();
        mark_detection_content_changed(&self.detection_content_seq);
        self.io.resize(
            rows,
            cols,
            cell_width_px,
            cell_height_px,
            terminal_responses,
        );
    }

    /// Scroll up by N lines (into scrollback history).
    pub fn scroll_up(&self, lines: usize) {
        self.terminal.scroll_up(lines);
        self.compression.wake();
    }

    /// Scroll down by N lines (toward live output).
    pub fn scroll_down(&self, lines: usize) {
        self.terminal.scroll_down(lines);
        self.compression.wake();
    }

    /// Reset scroll to live view (offset = 0).
    pub fn scroll_reset(&self) {
        self.terminal.scroll_reset();
        self.compression.wake();
    }

    /// Set scrollback offset measured from the live bottom of the terminal.
    pub fn set_scroll_offset_from_bottom(&self, lines: usize) {
        self.terminal.set_scroll_offset_from_bottom(lines);
        self.compression.wake();
    }

    pub fn scroll_metrics(&self) -> Option<ScrollMetrics> {
        self.terminal.scroll_metrics()
    }

    pub(crate) fn search_text_window(
        &self,
        query: &str,
        case_sensitive: bool,
        direction: crate::pane::TerminalSearchDirection,
        cursor: crate::pane::TerminalTextPoint,
        previous: Option<(
            crate::pane::TerminalTextPoint,
            crate::pane::TerminalTextPoint,
        )>,
        limit: usize,
    ) -> crate::pane::TerminalSearchWindow {
        let result = self.terminal.search_text_window(
            query,
            case_sensitive,
            direction,
            cursor,
            previous,
            limit,
        );
        self.compression.wake();
        result
    }

    pub(crate) fn word_motion_target(
        &self,
        row: u32,
        col: u16,
        motion: crate::pane::TerminalWordMotion,
    ) -> Option<crate::pane::TerminalTextPoint> {
        let result = self.terminal.word_motion_target(row, col, motion);
        self.compression.wake();
        result
    }

    pub(crate) fn terminal_dimensions(&self) -> Option<(u16, u16)> {
        self.terminal.dimensions()
    }

    pub(crate) fn paragraph_motion_target(
        &self,
        row: u32,
        direction: i8,
    ) -> Option<crate::pane::TerminalTextPoint> {
        let result = self.terminal.paragraph_motion_target(row, direction);
        self.compression.wake();
        result
    }

    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal.bracketed_paste_enabled()
    }

    pub fn focus_reporting_enabled(&self) -> bool {
        self.terminal.focus_reporting_enabled()
    }

    pub fn mouse_reporting_enabled(&self) -> bool {
        self.terminal.mouse_reporting_enabled()
    }

    pub fn sgr_pixel_mouse_enabled(&self) -> bool {
        self.terminal.sgr_pixel_mouse_enabled()
    }

    pub fn plain_page_keys_use_host_scrollback(&self) -> Option<bool> {
        self.terminal.plain_page_keys_use_host_scrollback()
    }

    pub fn alternate_screen_active(&self) -> bool {
        self.terminal.alternate_screen_active()
    }

    pub fn cursor_state(&self, area: Rect, show_cursor: bool) -> Option<TerminalCursorState> {
        if !show_cursor {
            return None;
        }
        let cursor = self.terminal.cursor_state()?;
        if cursor.x >= area.width || cursor.y >= area.height {
            return None;
        }
        Some(TerminalCursorState {
            x: area.x + cursor.x,
            y: area.y + cursor.y,
            visible: cursor.visible,
            shape: cursor.shape,
        })
    }

    pub fn synchronized_output_active(&self) -> bool {
        self.terminal.synchronized_output_active()
    }

    pub fn visible_text(&self) -> String {
        self.terminal.visible_text()
    }

    /// The visible screen with ANSI styling, as dialog detection reads it.
    pub(crate) fn visible_ansi_snapshot_with_seq(&self) -> Option<(String, u64)> {
        for _ in 0..3 {
            let before = self.content_seq.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                continue;
            }
            let text = self.terminal.visible_ansi();
            let after = self.content_seq.load(Ordering::Acquire);
            if before == after {
                return Some((text, after));
            }
        }
        None
    }

    pub(crate) fn visible_text_snapshot_with_dimensions(&self) -> Option<(String, u16, u16, u64)> {
        for _ in 0..3 {
            let before = self.content_seq.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                continue;
            }
            let (rows, columns) = self.current_size();
            let text = self.terminal.visible_text();
            let after = self.content_seq.load(Ordering::Acquire);
            if before == after {
                return Some((text, rows, columns, after));
            }
        }
        None
    }

    pub fn visible_ansi(&self) -> String {
        self.terminal.visible_ansi()
    }

    pub fn detection_text(&self) -> String {
        self.terminal.detection_text()
    }

    pub fn terminal_title(&self) -> Option<String> {
        self.terminal.terminal_title()
    }

    pub(crate) fn recent_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_text_snapshot(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_ansi_snapshot(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_unwrapped_text_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_unwrapped_text_snapshot(lines);
        self.compression.wake();
        result
    }

    pub fn recent_unwrapped_ansi(&self, lines: usize) -> String {
        let result = self.terminal.recent_unwrapped_ansi(lines);
        self.compression.wake();
        result
    }

    pub(crate) fn recent_unwrapped_ansi_snapshot(&self, lines: usize) -> TerminalReadSnapshot {
        let result = self.terminal.recent_unwrapped_ansi_snapshot(lines);
        self.compression.wake();
        result
    }

    pub fn snapshot_history(&self) -> Option<String> {
        let ansi = self.recent_unwrapped_ansi(usize::MAX);
        (!ansi.trim().is_empty()).then_some(ansi)
    }

    pub fn extract_selection(&self, selection: &crate::selection::Selection) -> Option<String> {
        let result = self.terminal.extract_selection(selection);
        self.compression.wake();
        result
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, show_cursor: bool) {
        self.terminal.render(frame, area, show_cursor);
    }

    pub(crate) fn collect_dirty_patch(
        &self,
        area_width: u16,
        area_height: u16,
    ) -> TerminalDirtyPatchOutcome {
        self.terminal.collect_dirty_patch(area_width, area_height)
    }

    pub fn visible_hyperlinks(&self, area: Rect) -> Vec<((u16, u16), String, String)> {
        self.terminal.visible_hyperlinks(area)
    }

    pub(crate) fn kitty_graphics_may_have_placements(&self) -> bool {
        self.terminal.kitty_graphics_may_have_placements()
    }

    pub fn kitty_image_placements_with_data_filter<F>(
        &self,
        needs_data: F,
    ) -> Vec<crate::ghostty::KittyImagePlacement>
    where
        F: FnMut(crate::ghostty::KittyImageDescriptor) -> bool,
    {
        self.terminal
            .kitty_image_placements_with_data_filter(needs_data)
    }

    pub fn keyboard_protocol(&self) -> crate::input::KeyboardProtocol {
        let fallback = crate::input::KeyboardProtocol::from_kitty_flags(
            self.kitty_keyboard_flags.load(Ordering::Relaxed),
        );
        self.terminal.keyboard_protocol(fallback)
    }

    pub fn modify_other_keys_level(&self) -> u8 {
        self.terminal.modify_other_keys_level()
    }

    pub fn encode_terminal_key(&self, key: crate::input::TerminalKey) -> Vec<u8> {
        self.terminal
            .encode_terminal_key(key, self.keyboard_protocol())
    }

    pub fn try_send_bytes(&self, bytes: Bytes) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.io.try_send_bytes(bytes)
    }

    /// Re-parse the live choice dialog and send the keys that choose `option`
    /// while holding the lock that serializes terminal content updates, so the
    /// keys only ever reach the exact dialog the caller observed.
    pub(crate) fn try_choose_dialog_option(
        &self,
        expected_digest: &str,
        option: u32,
    ) -> Result<DialogChoice, String> {
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(dialog) = crate::detect::dialog::parse(&self.terminal.visible_ansi())
            .filter(|dialog| dialog.digest() == expected_digest)
        else {
            return Ok(DialogChoice::Stale);
        };
        let Some(keys) = dialog.keys_for(option) else {
            return Ok(DialogChoice::Unreachable);
        };
        let encode = |key: &str| {
            let code = match key {
                "up" => crossterm::event::KeyCode::Up,
                "down" => crossterm::event::KeyCode::Down,
                "enter" => crossterm::event::KeyCode::Enter,
                digit => crossterm::event::KeyCode::Char(digit.chars().next().unwrap_or('0')),
            };
            self.encode_terminal_key(
                crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE).into(),
            )
        };
        match keys.split_last() {
            // Moves then Enter: confirm only after the TUI redrew the selection.
            Some((enter, moves)) if !moves.is_empty() => {
                let moves: Vec<u8> = moves.iter().flat_map(|key| encode(key)).collect();
                self.io
                    .queue_user_input_submission(
                        Bytes::from(moves),
                        Bytes::from(encode(enter)),
                        DIALOG_CONFIRM_DELAY,
                        None,
                    )
                    .map_err(|error| error.to_string())?;
            }
            _ => self
                .io
                .try_send_bytes(Bytes::from(
                    keys.iter().flat_map(|key| encode(key)).collect::<Vec<_>>(),
                ))
                .map_err(|error| error.to_string())?,
        }
        Ok(DialogChoice::Sent(
            keys.into_iter().map(str::to_owned).collect(),
        ))
    }

    pub(crate) fn try_answer_dialog(
        &self,
        expected_digest: &str,
        text: Option<String>,
        skip: bool,
    ) -> Result<DialogChoice, String> {
        crate::api::schema::AgentDialogAnswerParams::validate_answer(text.as_deref(), skip)?;
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let Some(dialog) = crate::detect::dialog::parse(&self.terminal.visible_ansi())
            .filter(|dialog| dialog.digest() == expected_digest)
        else {
            return Ok(DialogChoice::Stale);
        };
        let Some(input) = dialog.input else {
            return Ok(DialogChoice::NotQuestion);
        };
        let (code, modifiers, key) = if skip && input.skip_key == "ctrl+]" {
            (
                crossterm::event::KeyCode::Char(']'),
                crossterm::event::KeyModifiers::CONTROL,
                "ctrl+]",
            )
        } else if skip {
            (
                crossterm::event::KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
                "esc",
            )
        } else {
            (
                crossterm::event::KeyCode::Enter,
                crossterm::event::KeyModifiers::NONE,
                "enter",
            )
        };
        let key_bytes = Bytes::from(
            self.encode_terminal_key(crossterm::event::KeyEvent::new(code, modifiers).into()),
        );
        if let Some(text) = text {
            if !self.bracketed_paste_enabled() && text.contains(['\n', '\t']) {
                return Err("Multiline answers require bracketed paste support".into());
            }
            self.io
                .queue_user_input_submission(
                    self.paste_payload(text),
                    key_bytes,
                    DIALOG_CONFIRM_DELAY,
                    None,
                )
                .map_err(|error| error.to_string())?;
        } else {
            self.io
                .try_send_bytes(key_bytes)
                .map_err(|error| error.to_string())?;
        }
        Ok(DialogChoice::Sent(vec![key.into()]))
    }

    pub fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: std::time::Duration,
        deadline: Option<std::time::Instant>,
    ) -> std::io::Result<std::sync::mpsc::Receiver<std::io::Result<()>>> {
        self.io
            .queue_user_input_submission(text, enter, delay, deadline)
    }

    pub fn try_send_paste(&self, text: String) -> Result<(), mpsc::error::TrySendError<Bytes>> {
        self.try_send_bytes(self.paste_payload(text))
    }

    fn paste_payload(&self, text: String) -> Bytes {
        let text = crate::platform::prepare_paste_text_for_pty(text);
        let bracketed = self.bracketed_paste_enabled();
        let payload = if bracketed {
            format!("\x1b[200~{text}\x1b[201~")
        } else {
            text
        };
        Bytes::from(payload)
    }

    pub fn try_send_focus_event(&self, event: crate::ghostty::FocusEvent) -> bool {
        if !self.focus_reporting_enabled() {
            return false;
        }

        let Ok(bytes) = crate::ghostty::encode_focus(event) else {
            return false;
        };
        if let Err(err) = self.try_send_bytes(Bytes::from(bytes)) {
            warn!(err = %err, ?event, "failed to forward pane focus event");
        }
        true
    }

    pub fn wheel_routing(&self) -> Option<WheelRouting> {
        self.terminal.wheel_routing()
    }

    pub(crate) fn screen_text_snapshot(
        &self,
    ) -> Option<(
        crate::ghostty::ActiveScreen,
        u16,
        Vec<crate::ghostty::ScreenTextRow>,
    )> {
        let result = self.terminal.screen_text_snapshot();
        self.compression.wake();
        result
    }

    pub fn encode_mouse_button(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        if !self.mouse_reporting_enabled() {
            return None;
        }
        self.terminal.encode_mouse_button(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_motion(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        self.terminal.encode_mouse_motion(kind, position, modifiers)
    }

    pub(crate) fn encode_mouse_wheel(
        &self,
        kind: crossterm::event::MouseEventKind,
        position: crate::input::mouse::Position,
        modifiers: crossterm::event::KeyModifiers,
    ) -> Option<Vec<u8>> {
        if self.wheel_routing()? != WheelRouting::MouseReport {
            return None;
        }
        self.terminal.encode_mouse_wheel(kind, position, modifiers)
    }

    pub(crate) fn pixel_size(&self) -> Option<(u32, u32)> {
        let (rows, cols, cell_width_px, cell_height_px) = self.current_size.get();
        let width = u32::from(cols).checked_mul(cell_width_px)?;
        let height = u32::from(rows).checked_mul(cell_height_px)?;
        (width > 0 && height > 0).then_some((width, height))
    }

    pub fn encode_alternate_scroll(
        &self,
        kind: crossterm::event::MouseEventKind,
    ) -> Option<Vec<u8>> {
        if self.wheel_routing()? != WheelRouting::AlternateScroll {
            return None;
        }
        let key = match kind {
            crossterm::event::MouseEventKind::ScrollUp => crossterm::event::KeyCode::Up,
            crossterm::event::MouseEventKind::ScrollDown => crossterm::event::KeyCode::Down,
            _ => return None,
        };
        Some(self.encode_terminal_key(crate::input::TerminalKey::new(
            key,
            crossterm::event::KeyModifiers::empty(),
        )))
    }

    /// Get the current working directory of the child shell process.
    pub fn cwd(&self) -> Option<std::path::PathBuf> {
        if let Some(cwd) = self
            .reported_cwd
            .lock()
            .ok()
            .and_then(|reported_cwd| reported_cwd.clone())
        {
            return Some(cwd);
        }

        let pid = self.child_pid.load(Ordering::Relaxed);
        crate::platform::process_cwd(pid)
    }

    pub fn child_pid(&self) -> Option<u32> {
        let pid = self.child_pid.load(Ordering::Acquire);
        (pid > 0).then_some(pid)
    }

    pub fn follow_cwd(&self) -> Option<std::path::PathBuf> {
        #[cfg(unix)]
        {
            let leader_cwd = self
                .io
                .foreground_process_group_id()
                .and_then(usable_process_cwd);
            leader_cwd.or_else(|| self.cwd())
        }

        #[cfg(not(unix))]
        {
            self.cwd()
        }
    }

    /// Get the current working directory of the process group controlling the pane PTY.
    pub fn foreground_cwd(&self) -> Option<std::path::PathBuf> {
        #[cfg(unix)]
        {
            let pid = self.child_pid.load(Ordering::Acquire);
            let shell_cwd = absolute_process_cwd(pid);
            let foreground_pgid = self
                .io
                .foreground_process_group_id()
                .or_else(|| crate::platform::foreground_process_group_id(pid));
            let leader_cwd = foreground_pgid.and_then(absolute_process_cwd);

            // The group leader's cwd is authoritative (issue #3270): a helper
            // process that chdirs elsewhere inside the same foreground group
            // must not override it. Scan other members only when the leader's
            // cwd cannot be read at all.
            leader_cwd
                .or_else(|| foreground_member_cwd_different_from_shell(pid, shell_cwd.as_ref()))
        }

        #[cfg(not(unix))]
        {
            None
        }
    }
}

#[cfg(test)]
impl PaneRuntime {
    pub(crate) fn test_with_channel(cols: u16, rows: u16) -> (Self, mpsc::Receiver<Bytes>) {
        Self::test_with_channel_and_scrollback_bytes(cols, rows, 0, &[], 4)
    }

    pub(crate) fn test_with_channel_capacity(
        cols: u16,
        rows: u16,
        capacity: usize,
    ) -> (Self, mpsc::Receiver<Bytes>) {
        Self::test_with_channel_and_scrollback_bytes(cols, rows, 0, &[], capacity)
    }

    pub(crate) fn test_with_screen_bytes(cols: u16, rows: u16, bytes: &[u8]) -> Self {
        Self::test_with_scrollback_bytes(cols, rows, 0, bytes)
    }

    pub(crate) fn test_process_pty_bytes(&self, bytes: &[u8]) {
        let _content_write_guard = match self.content_write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.content_seq.fetch_add(1, Ordering::AcqRel);
        let (tx, _rx) = mpsc::channel(1);
        let _ = self.terminal.process_pty_bytes(self.pane_id, 0, bytes, &tx);
        self.content_seq.fetch_add(1, Ordering::Release);
        self.compression.wake();
    }

    pub(crate) fn test_with_scrollback_bytes(
        cols: u16,
        rows: u16,
        scrollback_limit_bytes: usize,
        bytes: &[u8],
    ) -> Self {
        Self::test_with_channel_and_scrollback_bytes(cols, rows, scrollback_limit_bytes, bytes, 4).0
    }

    pub(crate) fn test_with_channel_and_scrollback_bytes(
        cols: u16,
        rows: u16,
        scrollback_limit_bytes: usize,
        bytes: &[u8],
        channel_capacity: usize,
    ) -> (Self, mpsc::Receiver<Bytes>) {
        let (tx, rx) = mpsc::channel(channel_capacity);
        let (resize_tx, _resize_rx) = watch::channel((rows, cols, 0, 0));
        let mut terminal =
            crate::ghostty::Terminal::new(cols, rows, scrollback_limit_bytes).unwrap();
        terminal.write(bytes);
        let pane_id = PaneId::from_raw(0);
        let terminal = Arc::new(PaneTerminal::new(
            GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
        ));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());

        (
            Self {
                pane_id,
                terminal,
                io: PaneRuntimeIo::TestChannel {
                    sender: tx,
                    resize_tx,
                },
                current_size: Cell::new((rows, cols, 0, 0)),
                child_pid: Arc::new(AtomicU32::new(0)),
                reported_cwd: Arc::new(Mutex::new(None)),
                child_wait_completed: None,
                kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
                content_seq: Arc::new(AtomicU64::new(0)),
                content_write_lock: Arc::new(Mutex::new(())),
                detection_content_seq: Arc::new(AtomicU64::new(0)),
                preserve_processes_on_drop: true,
                compression,
                detect_handle: Some(tokio::spawn(async {}).abort_handle()),
            },
            rx,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_launch_env_removes_outer_codex_thread_id() {
        let mut cmd = CommandBuilder::new("shell");
        cmd.env("CODEX_THREAD_ID", "outer-session");

        apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::default());

        assert!(cmd.get_env("CODEX_THREAD_ID").is_none());
    }

    #[test]
    fn pane_launch_env_isolates_agent_sessions_without_losing_configuration() {
        // Session context exported by Claude's Bash tool and Codex's exec tool.
        let session = [
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_CHILD_SESSION", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("CLAUDE_CODE_SESSION_ID", "outer-claude"),
            ("CLAUDE_CODE_SESSION_ATTENDED", "0"),
            ("CLAUDE_CODE_SSE_PORT", "12345"),
            ("CLAUDE_CODE_MESSAGING_SOCKET", "/outer/message.sock"),
            ("CLAUDE_CODE_MESSAGING_TOKEN", "outer-message-token"),
            ("CLAUDE_CODE_SANDBOXED", "1"),
            ("CLAUDE_PID", "123"),
            ("CLAUDE_JOB_DIR", "/outer/job"),
            ("CODEX_THREAD_ID", "outer-thread"),
            ("CODEX_SESSION_ID", "outer-codex"),
            ("CODEX_SANDBOX", "seatbelt"),
            ("CODEX_SANDBOX_NETWORK_DISABLED", "1"),
            ("CODEX_PERMISSION_PROFILE", ":workspace"),
            ("CODEX_ESCALATE_SOCKET", "/outer/escalate.sock"),
            ("CODEX_EXEC_SERVER_NOISE_AUTH_TOKEN", "outer-exec-token"),
        ];
        let config = [
            ("CLAUDE_CONFIG_DIR", "/user/claude"),
            ("CLAUDE_CODE_EXECPATH", "/user/bin/claude"),
            ("CLAUDE_CODE_SHELL", "/bin/zsh"),
            ("CLAUDE_CODE_SUBAGENT_MODEL", "haiku"),
            ("CLAUDE_CODE_USE_BEDROCK", "1"),
            ("CLAUDE_EFFORT", "medium"),
            ("ANTHROPIC_API_KEY", "user-api-key"),
            ("ANTHROPIC_BASE_URL", "https://provider.example"),
            ("CODEX_HOME", "/user/codex"),
            ("CODEX_API_KEY", "user-codex-key"),
            ("CODEX_MANAGED_BY_NPM", "1"),
            ("CODEX_CI", "1"),
            ("OPENAI_API_KEY", "user-openai-key"),
            ("HTTP_PROXY", "http://proxy.example"),
            ("PATH", "/user/bin"),
        ];
        // Both a new shell and a resumed agent reach the same pane-env boundary.
        // Explicit launch env must not put a parent's session context back.
        for restore_from_launch_env in [false, true] {
            let mut cmd = CommandBuilder::new("shell");
            cmd.env_clear();
            for (key, value) in session.into_iter().chain(config) {
                cmd.env(key, value);
            }
            let mut extra = vec![
                ("BUS_LAUNCH_ID".into(), "own-launch".into()),
                ("BUS_CALLBACK_DIR".into(), "/own/callbacks".into()),
            ];
            if restore_from_launch_env {
                extra.extend(session.map(|(key, value)| (key.into(), value.into())));
            }
            apply_pane_launch_env(&mut cmd, &PaneLaunchEnv::from_extra(extra));

            for (key, _) in session {
                assert!(
                    cmd.get_env(key).is_none(),
                    "inherited session marker: {key}"
                );
            }
            for (key, value) in config {
                assert_eq!(cmd.get_env(key), Some(std::ffi::OsStr::new(value)), "{key}");
            }
            for (key, value) in [
                ("BUS_LAUNCH_ID", "own-launch"),
                ("BUS_CALLBACK_DIR", "/own/callbacks"),
            ] {
                assert_eq!(cmd.get_env(key), Some(std::ffi::OsStr::new(value)), "{key}");
            }
        }
    }

    #[test]
    fn pane_terminal_identity_removes_outer_windows_terminal_session() {
        let mut cmd = CommandBuilder::new("shell");
        cmd.env("WT_SESSION", "outer-session");

        apply_pane_terminal_env(&mut cmd);

        assert!(cmd.get_env("WT_SESSION").is_none());
    }

    #[tokio::test]
    async fn cwd_returns_accepted_report_without_rechecking_filesystem() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        let cwd = std::env::temp_dir().join(format!(
            "herdr-reported-cwd-cache-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&cwd).expect("create reported cwd");

        let (runtime, _rx) = PaneRuntime::test_with_channel(80, 24);
        let (events, _event_rx) = mpsc::channel(1);
        publish_reported_cwd(runtime.pane_id, cwd.clone(), &runtime.reported_cwd, &events);
        assert_eq!(
            runtime.reported_cwd.lock().unwrap().as_ref(),
            Some(&cwd),
            "test setup must pass cache admission"
        );

        std::fs::remove_dir(&cwd).expect("remove reported cwd after admission");

        assert_eq!(runtime.cwd(), Some(cwd));
    }

    #[cfg(unix)]
    #[test]
    fn process_cwd_does_not_require_traversing_the_directory_path() {
        use std::os::unix::fs::PermissionsExt;

        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "herdr-process-cwd-no-stat-{}-{stamp}",
            std::process::id()
        ));
        let private = base.join("private");
        let cwd = private.join("cwd");
        std::fs::create_dir_all(&cwd).expect("create process cwd");

        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .current_dir(&cwd)
            .spawn()
            .expect("spawn process in cwd");
        let expected_cwd = crate::platform::process_cwd(child.id())
            .expect("resolve process cwd before restricting traversal");
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o000))
            .expect("make cwd path untraversable");

        let path_is_traversable = cwd.is_dir();
        let observed = (!path_is_traversable)
            .then(|| absolute_process_cwd(child.id()))
            .flatten();

        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755))
            .expect("restore cwd path permissions");
        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(&base).expect("remove process cwd");

        if path_is_traversable {
            eprintln!("skipping untraversable cwd assertion for privileged test process");
            return;
        }
        assert_eq!(observed, Some(expected_cwd));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn follow_cwd_falls_back_to_reported_pane_cwd_without_foreground_group() {
        let (runtime, _rx) = PaneRuntime::test_with_channel(80, 24);
        let cwd = std::env::temp_dir();
        *runtime.reported_cwd.lock().unwrap() = Some(cwd.clone());

        assert_eq!(runtime.follow_cwd(), Some(cwd));
    }

    #[test]
    fn shutdown_liveness_treats_reaped_direct_child_as_gone() {
        assert!(!process_alive_for_shutdown(42, 42, true, |_| true));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checked_close_stops_owned_native_session_and_clears_pid_before_retry() {
        let (events, _event_rx) = mpsc::channel(8);
        let runtime = PaneRuntime::spawn_shell_command(
            PaneId::from_raw(43),
            24,
            80,
            std::env::temp_dir(),
            "exec sleep 30",
            &PaneLaunchEnv::default(),
            AgentDetection::Disabled,
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
        )
        .unwrap();
        let pid = runtime.child_pid.load(Ordering::Acquire);
        assert_ne!(pid, 0);
        assert!(runtime.stop_session_for_close());
        assert_eq!(runtime.child_pid.load(Ordering::Acquire), 0);
        assert!(runtime.stop_session_for_close());
        runtime.shutdown();
    }

    #[test]
    fn shutdown_liveness_keeps_unreaped_direct_child_alive() {
        assert!(process_alive_for_shutdown(42, 42, false, |_| true));
    }

    #[test]
    fn shutdown_liveness_keeps_other_session_processes_alive() {
        assert!(process_alive_for_shutdown(43, 42, true, |_| true));
    }

    #[test]
    fn shutdown_liveness_treats_missing_process_as_gone() {
        assert!(!process_alive_for_shutdown(43, 42, false, |_| false));
    }

    #[cfg(unix)]
    fn capture_shell_output(command: &str, extra_env: &[(&str, &str)]) -> String {
        static CAPTURE_ID: AtomicU64 = AtomicU64::new(0);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let output_path = std::env::temp_dir().join(format!(
            "herdr-pane-term-test-{}-{}-{}.txt",
            std::process::id(),
            CAPTURE_ID.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.arg("-c");
        cmd.arg(format!("{command} > '{}'", output_path.display()));
        cmd.cwd(std::env::current_dir().unwrap());
        cmd.env("TERM", "xterm-ghostty");
        cmd.env("COLORTERM", "falsecolor");
        cmd.env("NO_COLOR", "1");
        apply_pane_terminal_env(&mut cmd);
        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        let mut child = pair.slave.spawn_command(cmd).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success(), "shell command failed: {status:?}");

        let output = std::fs::read_to_string(&output_path).unwrap();
        let _ = std::fs::remove_file(output_path);
        output
    }

    #[test]
    fn pane_shell_prefers_configured_shell() {
        assert_eq!(
            pane_shell_from("/usr/bin/nu", Some("/bin/bash".to_string())),
            "/usr/bin/nu"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn pane_shell_falls_back_to_shell_env() {
        assert_eq!(
            pane_shell_from("", Some("/bin/bash".to_string())),
            "/bin/bash"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pane_shell_ignores_shell_env_on_windows() {
        assert_eq!(
            pane_shell_from("", Some("c:\\windows\\system32\\cmd.exe".to_string())),
            default_pane_shell()
        );
    }

    #[test]
    fn pane_shell_ignores_empty_values() {
        assert_eq!(
            pane_shell_from("   ", Some("  ".to_string())),
            default_pane_shell()
        );
        assert_eq!(pane_shell_from("", None), default_pane_shell());
    }

    #[test]
    fn shell_mode_auto_uses_login_shell_only_on_macos() {
        assert!(shell_mode_uses_login_shell(
            crate::config::ShellModeConfig::Auto,
            ShellLaunchTarget::Macos
        ));
        assert!(!shell_mode_uses_login_shell(
            crate::config::ShellModeConfig::Auto,
            ShellLaunchTarget::OtherUnix
        ));
        assert!(!shell_mode_uses_login_shell(
            crate::config::ShellModeConfig::Auto,
            ShellLaunchTarget::Windows
        ));
        assert!(shell_mode_uses_login_shell(
            crate::config::ShellModeConfig::Login,
            ShellLaunchTarget::OtherUnix
        ));
        assert!(!shell_mode_uses_login_shell(
            crate::config::ShellModeConfig::NonLogin,
            ShellLaunchTarget::Macos
        ));
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_builder_uses_default_prog_with_resolved_shell_env() {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::Login),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap();
        assert!(cmd.is_default_prog());
        assert_eq!(
            cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
            Some("/bin/sh")
        );
    }

    #[cfg(unix)]
    #[test]
    fn auto_shell_builder_uses_login_shell_on_macos_target() {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::Auto),
            ShellLaunchTarget::Macos,
        )
        .unwrap();
        assert!(cmd.is_default_prog());
        assert_eq!(
            cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
            Some("/bin/sh")
        );
    }

    #[test]
    fn auto_shell_builder_keeps_direct_shell_on_non_macos_target() {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::Auto),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap();
        assert!(!cmd.is_default_prog());
        assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("/bin/sh")]);
    }

    #[test]
    fn windows_powershell_builder_injects_prompt_cwd_shell_integration() {
        for shell in [
            "powershell.exe",
            "pwsh.exe",
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
        ] {
            let cmd = pane_shell_command_builder_for_target(
                PaneShellConfig::new(shell, crate::config::ShellModeConfig::NonLogin),
                ShellLaunchTarget::Windows,
            )
            .unwrap();

            assert_eq!(
                cmd.get_argv(),
                &[
                    std::ffi::OsString::from(shell),
                    std::ffi::OsString::from("-NoExit"),
                    std::ffi::OsString::from("-Command"),
                    std::ffi::OsString::from(WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND),
                ]
            );
        }

        let script = WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND;
        let cwd_sync = script
            .find("[Environment]::CurrentDirectory = $loc.ProviderPath")
            .expect("wrapper must synchronize the Win32 process cwd");
        let osc_report = script.find("]9;9;").expect("wrapper must emit OSC 9;9");
        assert!(cwd_sync < osc_report, "cwd sync must precede OSC report");
        assert!(
            script.contains("$global:__HerdrOriginalPrompt = $function:prompt"),
            "must wrap the profile-defined prompt: {script}"
        );
        assert!(
            script.contains("$null -eq $global:__HerdrOriginalPrompt"),
            "wrap must be idempotent for nested sessions: {script}"
        );
        assert!(
            script.contains("'FileSystem'"),
            "must not report non-filesystem provider paths: {script}"
        );
        assert!(
            !script.contains('"'),
            "double quotes corrupt the powershell.exe command-line round-trip: {script}"
        );
        let invoke_original = script
            .find("@(& $global:__HerdrOriginalPrompt)")
            .expect("wrapper must invoke the original prompt");
        let cwd_lookup = script
            .find("$loc =")
            .expect("wrapper must look up the current location");
        assert!(
            invoke_original < cwd_lookup,
            "original prompt must run first or $? is reset before a status-aware prompt reads it: {script}"
        );
    }

    #[test]
    fn windows_non_powershell_builder_launches_plain_shell() {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("cmd.exe", crate::config::ShellModeConfig::NonLogin),
            ShellLaunchTarget::Windows,
        )
        .unwrap();

        assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("cmd.exe")]);
    }

    #[test]
    fn unix_powershell_builder_launches_plain_shell() {
        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("pwsh", crate::config::ShellModeConfig::NonLogin),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap();

        assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("pwsh")]);
    }

    #[test]
    fn windows_powershell_pane_shell_predicate_requires_windows_and_non_login() {
        let pwsh = PaneShellConfig::new("pwsh.exe", crate::config::ShellModeConfig::NonLogin);
        assert!(uses_windows_powershell_pane_shell_for_target(
            pwsh,
            ShellLaunchTarget::Windows
        ));
        assert!(!uses_windows_powershell_pane_shell_for_target(
            pwsh,
            ShellLaunchTarget::OtherUnix
        ));
        assert!(!uses_windows_powershell_pane_shell_for_target(
            pwsh,
            ShellLaunchTarget::Macos
        ));
        assert!(!uses_windows_powershell_pane_shell_for_target(
            PaneShellConfig::new("pwsh.exe", crate::config::ShellModeConfig::Login),
            ShellLaunchTarget::Windows
        ));
        assert!(!uses_windows_powershell_pane_shell_for_target(
            PaneShellConfig::new("cmd.exe", crate::config::ShellModeConfig::NonLogin),
            ShellLaunchTarget::Windows
        ));
    }

    #[test]
    fn login_shell_builder_rejects_missing_shell_instead_of_falling_back() {
        let err = pane_shell_command_builder_for_target(
            PaneShellConfig::new(
                "/__herdr_missing_shell__",
                crate::config::ShellModeConfig::Login,
            ),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_builder_resolves_bare_shell_names_from_path() {
        let _lock = self::env::env_lock();
        let base = std::env::temp_dir().join(format!(
            "herdr-login-shell-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = base.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let shell = bin.join("fake-shell");
        std::fs::write(&shell, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let original_path = std::env::var_os("PATH");
        std::env::set_var("PATH", &bin);

        let cmd = pane_shell_command_builder_for_target(
            PaneShellConfig::new("fake-shell", crate::config::ShellModeConfig::Login),
            ShellLaunchTarget::OtherUnix,
        )
        .unwrap();

        assert!(cmd.is_default_prog());
        assert_eq!(
            cmd.get_env("SHELL").and_then(std::ffi::OsStr::to_str),
            shell.to_str()
        );
        match original_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn login_shell_resolution_preserves_shell_paths() {
        assert_eq!(resolve_shell_for_login_mode("/bin/sh").unwrap(), "/bin/sh");
    }

    #[test]
    fn non_login_shell_builder_execs_resolved_shell_directly() {
        let cmd = pane_shell_command_builder(PaneShellConfig::new(
            "/bin/sh",
            crate::config::ShellModeConfig::NonLogin,
        ))
        .unwrap();
        assert!(!cmd.is_default_prog());
        assert_eq!(cmd.get_argv(), &[std::ffi::OsString::from("/bin/sh")]);
    }

    #[cfg(unix)]
    #[test]
    fn pane_terminal_identity_overrides_outer_terminal_env() {
        let output = capture_shell_output("printf '%s\\n%s\\n' \"$TERM\" \"$COLORTERM\"", &[]);
        assert_eq!(output, "xterm-256color\ntruecolor\n");
    }

    #[cfg(unix)]
    #[test]
    fn pane_terminal_identity_allows_explicit_override() {
        let output = capture_shell_output(
            "printf '%s\\n%s\\n' \"$TERM\" \"$COLORTERM\"",
            &[("TERM", "vt100"), ("COLORTERM", "24bit")],
        );
        assert_eq!(output, "vt100\n24bit\n");
    }

    #[cfg(unix)]
    #[test]
    fn pane_terminal_does_not_inherit_outer_no_color() {
        let output = capture_shell_output("printf '%s' \"${NO_COLOR-unset}\"", &[]);
        assert_eq!(output, "unset");
    }

    #[cfg(unix)]
    #[test]
    fn pane_terminal_allows_explicit_no_color_override() {
        let output =
            capture_shell_output("printf '%s' \"${NO_COLOR-unset}\"", &[("NO_COLOR", "1")]);
        assert_eq!(output, "1");
    }

    #[tokio::test]
    async fn compression_permit_survives_an_aborted_async_waiter() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let handle = spawn_blocking_with_compression_permit(permit, move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv();
        });
        started_rx.await.unwrap();

        handle.abort();
        assert!(semaphore.clone().try_acquire_owned().is_err());

        release_tx.send(()).unwrap();
        handle.await.unwrap();
        assert!(semaphore.try_acquire_owned().is_ok());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn compression_task_rechecks_history_after_a_read() {
        let suffix = "x".repeat(66);
        let history = (1..=2_000)
            .map(|line| format!("{line:05} {suffix}\r\n"))
            .collect::<String>();
        let runtime =
            PaneRuntime::test_with_scrollback_bytes(80, 24, 20_000_000, history.as_bytes());

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while runtime.compression.completed_passes() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let completed_before_read = runtime.compression.completed_passes();

        let snapshot = runtime.recent_unwrapped_text_snapshot(usize::MAX);
        assert!(snapshot.text.contains("00001 "));
        assert!(snapshot.text.contains("02000 "));

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while runtime.compression.completed_passes() == completed_before_read {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn compressed_scrollback_survives_shrink_and_grow_resize() {
        let suffix = "x".repeat(66);
        let history = (1..=2_000)
            .map(|line| format!("{line:05} {suffix}\r\n"))
            .collect::<String>();
        let runtime =
            PaneRuntime::test_with_scrollback_bytes(80, 45, 20_000_000, history.as_bytes());

        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while runtime.compression.completed_passes() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        runtime.resize(21, 80, 0, 0);
        let completed_after_shrink = runtime.compression.completed_passes();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while runtime.compression.completed_passes() == completed_after_shrink {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        runtime.resize(45, 80, 0, 0);

        assert_eq!(runtime.current_size(), (45, 80));
        assert_eq!(runtime.terminal_dimensions(), Some((80, 45)));
        assert_eq!(runtime.scroll_metrics().unwrap().viewport_rows, 45);
        let snapshot = runtime.recent_unwrapped_text_snapshot(usize::MAX);
        assert!(snapshot.text.contains("00001 "));
        assert!(snapshot.text.contains("02000 "));
    }

    #[tokio::test]
    async fn focus_events_are_forwarded_when_enabled() {
        let (tx, mut rx) = mpsc::channel(4);
        let (resize_tx, _resize_rx) = watch::channel((80, 24, 0, 0));
        let mut terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        terminal
            .mode_set(crate::ghostty::MODE_FOCUS_EVENT, true)
            .unwrap();
        let pane_id = PaneId::from_raw(0);
        let terminal = Arc::new(PaneTerminal::new(
            GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
        ));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let runtime = PaneRuntime {
            pane_id,
            terminal,
            io: PaneRuntimeIo::TestChannel {
                sender: tx,
                resize_tx,
            },
            current_size: Cell::new((80, 24, 0, 0)),
            child_pid: Arc::new(AtomicU32::new(0)),
            reported_cwd: Arc::new(Mutex::new(None)),
            child_wait_completed: None,
            kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
            content_seq: Arc::new(AtomicU64::new(0)),
            content_write_lock: Arc::new(Mutex::new(())),
            detection_content_seq: Arc::new(AtomicU64::new(0)),
            preserve_processes_on_drop: true,
            compression,
            detect_handle: Some(tokio::spawn(async {}).abort_handle()),
        };

        assert!(runtime.try_send_focus_event(crate::ghostty::FocusEvent::Gained));
        assert_eq!(rx.recv().await.unwrap(), Bytes::from_static(b"\x1b[I"));
    }

    #[tokio::test]
    async fn focus_events_are_suppressed_when_disabled() {
        let (tx, mut rx) = mpsc::channel(4);
        let (resize_tx, _resize_rx) = watch::channel((80, 24, 0, 0));
        let terminal = crate::ghostty::Terminal::new(80, 24, 0).unwrap();
        let pane_id = PaneId::from_raw(0);
        let terminal = Arc::new(PaneTerminal::new(
            GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
        ));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let runtime = PaneRuntime {
            pane_id,
            terminal,
            io: PaneRuntimeIo::TestChannel {
                sender: tx,
                resize_tx,
            },
            current_size: Cell::new((80, 24, 0, 0)),
            child_pid: Arc::new(AtomicU32::new(0)),
            reported_cwd: Arc::new(Mutex::new(None)),
            child_wait_completed: None,
            kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
            content_seq: Arc::new(AtomicU64::new(0)),
            content_write_lock: Arc::new(Mutex::new(())),
            detection_content_seq: Arc::new(AtomicU64::new(0)),
            preserve_processes_on_drop: true,
            compression,
            detect_handle: Some(tokio::spawn(async {}).abort_handle()),
        };

        assert!(!runtime.try_send_focus_event(crate::ghostty::FocusEvent::Gained));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn subscribed_idle_child_receives_color_scheme_transition() {
        let (runtime, mut rx) = PaneRuntime::test_with_channel(80, 24);
        runtime.apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Dark));
        runtime.test_process_pty_bytes(b"\x1b[?2031h");

        runtime.apply_host_terminal_appearance(Some(crate::terminal_theme::HostAppearance::Light));

        assert_eq!(rx.recv().await, Some(Bytes::from_static(b"\x1b[?997;2n")));
    }

    #[test]
    fn foreground_shell_reports_process_exit_before_clearing_agent() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Codex), None, true, false),
            ForegroundShellAgentAction::ReportProcessExit
        );
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Codex), None, true, true),
            ForegroundShellAgentAction::ClearAgent
        );
    }

    #[test]
    fn same_agent_after_reported_exit_is_a_replacement_process() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Pi), Some(Agent::Pi), false, true),
            ForegroundShellAgentAction::ReportReplacementProcess
        );
    }

    #[test]
    fn unknown_non_shell_foreground_job_is_not_immediate_clear_signal() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Claude), None, false, false),
            ForegroundShellAgentAction::ObserveProbe
        );
    }

    #[tokio::test]
    async fn first_agent_acquisition_keeps_osc_evidence_replacement_clears_it() {
        let runtime = PaneRuntime::test_with_screen_bytes(80, 24, b"");
        runtime.test_process_pty_bytes(b"\x1b]2;startup title\x1b\\\x1b]9;4;1;\x1b\\");

        clear_osc_evidence_for_agent_transition(&runtime.terminal, None);
        assert_eq!(runtime.terminal.agent_osc_title(), "startup title");
        assert_eq!(runtime.terminal.agent_osc_progress(), "4;1;");

        clear_osc_evidence_for_agent_transition(&runtime.terminal, Some(Agent::Claude));
        assert_eq!(runtime.terminal.agent_osc_title(), "");
        assert_eq!(runtime.terminal.agent_osc_progress(), "");
    }

    #[test]
    fn reported_process_exit_clears_before_unknown_foreground_probe() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Claude), None, false, true),
            ForegroundShellAgentAction::ClearAgent
        );
    }

    #[test]
    fn foreground_agent_job_is_not_clear_signal() {
        assert_eq!(
            foreground_shell_agent_action(Some(Agent::Claude), Some(Agent::OpenCode), true, false,),
            ForegroundShellAgentAction::ObserveProbe
        );
    }

    fn foreground_process(pid: u32, name: &str) -> crate::platform::ForegroundProcess {
        crate::platform::ForegroundProcess {
            pid,
            name: name.to_string(),
            argv0: None,
            argv: None,
            cmdline: None,
        }
    }

    #[test]
    fn foreground_agent_hint_accepts_pane_shell_environment() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 42,
            processes: vec![foreground_process(42, "bash")],
        };

        assert_eq!(
            agent_hint_for_foreground_job_members(&job, |pid| {
                (pid == 42).then_some(Agent::Claude)
            }),
            Some(Agent::Claude)
        );
    }

    #[test]
    fn foreground_agent_hint_accepts_non_leader_foreground_process_environment() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 99,
            processes: vec![
                foreground_process(99, "fence"),
                foreground_process(100, "pi"),
            ],
        };

        assert_eq!(
            agent_hint_for_foreground_job_members(&job, |pid| {
                (pid == 100).then_some(Agent::Codex)
            }),
            Some(Agent::Codex)
        );
    }

    #[test]
    fn foreground_agent_hint_wins_over_process_name_detection() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 99,
            processes: vec![foreground_process(99, "codex")],
        };

        let result = probe_foreground_process_from_jobs(
            42,
            Some(99),
            Some(job),
            || None,
            |pid| (pid == 99).then_some(Agent::Claude),
        );

        assert_eq!(result.agent, Some(Agent::Claude));
        assert_eq!(result.process_name.as_deref(), Some("claude"));
    }

    #[test]
    fn foreground_agent_hint_on_inherited_child_environment_is_authoritative() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 99,
            processes: vec![foreground_process(99, "vim")],
        };

        let result = probe_foreground_process_from_jobs(
            42,
            Some(99),
            None,
            || Some(job),
            |pid| (pid == 99).then_some(Agent::Claude),
        );

        assert_eq!(result.agent, Some(Agent::Claude));
        assert_eq!(result.process_name.as_deref(), Some("claude"));
    }

    #[test]
    fn non_leader_agent_hint_does_not_override_identifiable_leader() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 99,
            processes: vec![
                foreground_process(99, "codex"),
                foreground_process(100, "vim"),
            ],
        };

        let result = probe_foreground_process_from_jobs(
            42,
            Some(99),
            None,
            || Some(job),
            |pid| (pid == 100).then_some(Agent::Claude),
        );

        assert_eq!(result.agent, Some(Agent::Codex));
        assert_eq!(result.process_name.as_deref(), Some("codex"));
    }

    #[test]
    fn non_leader_agent_hint_wins_when_leader_is_unidentified() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 99,
            processes: vec![
                foreground_process(99, "some_vm"),
                foreground_process(100, "vim"),
            ],
        };

        let result = probe_foreground_process_from_jobs(
            42,
            Some(99),
            None,
            || Some(job),
            |pid| (pid == 100).then_some(Agent::Claude),
        );

        assert_eq!(result.agent, Some(Agent::Claude));
        assert_eq!(result.process_name.as_deref(), Some("claude"));
    }

    fn process_probe_input() -> ProcessProbeInput {
        ProcessProbeInput {
            current_agent: None,
            foreground_pgid: Some(42),
            last_foreground_pgid: Some(42),
            has_process_probe: true,
            acquisition_age: None,
            pending_foreground_shell_clear: false,
            pending_restore_probe: false,
            elapsed_since_process_check: std::time::Duration::from_secs(1),
        }
    }

    #[test]
    fn windows_foreground_observation_schedule_preserves_safety_checks() {
        let before_safety_bound = PROCESS_RECHECK_IDENTIFIED - std::time::Duration::from_millis(1);
        let quiet = ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            elapsed_since_process_check: before_safety_bound,
            ..process_probe_input()
        };
        let content_retry = std::time::Duration::from_millis(300);
        let content_due = |last: Option<u64>, current, elapsed| {
            last != Some(current) && (last.is_some() || elapsed >= content_retry)
        };

        assert!(!should_observe_foreground_process_group(false, quiet));
        assert!(should_observe_foreground_process_group(true, quiet));
        assert!(content_due(Some(0), 1, std::time::Duration::ZERO));
        assert!(!content_due(
            None,
            1,
            content_retry - std::time::Duration::from_millis(1)
        ));
        assert!(content_due(None, 1, content_retry));
        assert!(should_observe_foreground_process_group(
            false,
            ProcessProbeInput {
                elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED,
                ..quiet
            }
        ));

        for immediate in [
            ProcessProbeInput {
                has_process_probe: false,
                ..quiet
            },
            ProcessProbeInput {
                current_agent: None,
                acquisition_age: Some(std::time::Duration::ZERO),
                ..quiet
            },
            ProcessProbeInput {
                pending_restore_probe: true,
                ..quiet
            },
            ProcessProbeInput {
                pending_foreground_shell_clear: true,
                ..quiet
            },
        ] {
            assert!(should_observe_foreground_process_group(false, immediate));
        }
    }

    #[test]
    fn unchanged_unidentified_foreground_group_skips_full_process_probe() {
        assert!(!should_probe_foreground_job(process_probe_input()));
    }

    #[test]
    fn unidentified_foreground_group_change_runs_full_process_probe() {
        assert!(should_probe_foreground_job(ProcessProbeInput {
            foreground_pgid: Some(43),
            ..process_probe_input()
        }));
    }

    #[test]
    fn unidentified_pane_gets_initial_process_probe() {
        assert!(should_probe_foreground_job(ProcessProbeInput {
            has_process_probe: false,
            ..process_probe_input()
        }));
    }

    #[test]
    fn stable_unidentified_foreground_group_has_no_safety_process_probe() {
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            elapsed_since_process_check: PROCESS_RECHECK_MISSING_FOREGROUND_GROUP,
            ..process_probe_input()
        }));
    }

    #[test]
    fn unidentified_pane_without_foreground_group_uses_safety_process_probe() {
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            foreground_pgid: None,
            last_foreground_pgid: None,
            ..process_probe_input()
        }));
        assert!(should_probe_foreground_job(ProcessProbeInput {
            foreground_pgid: None,
            last_foreground_pgid: None,
            elapsed_since_process_check: PROCESS_RECHECK_MISSING_FOREGROUND_GROUP,
            ..process_probe_input()
        }));
    }

    #[test]
    fn unidentified_pane_probes_when_foreground_group_disappears() {
        assert!(should_probe_foreground_job(ProcessProbeInput {
            foreground_pgid: None,
            last_foreground_pgid: Some(42),
            ..process_probe_input()
        }));
    }

    #[test]
    fn inferred_group_does_not_trigger_a_probe_on_every_tick() {
        let tracked = process_group_for_change_tracking(None, Some(300));
        assert_eq!(tracked, None);
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Claude),
            foreground_pgid: None,
            last_foreground_pgid: tracked,
            elapsed_since_process_check: std::time::Duration::from_millis(300),
            ..process_probe_input()
        }));
    }

    #[test]
    fn pending_shell_clear_and_restore_force_process_probes() {
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            pending_foreground_shell_clear: true,
            ..process_probe_input()
        }));
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            pending_restore_probe: true,
            ..process_probe_input()
        }));
    }

    #[test]
    fn acquisition_window_catches_delayed_same_group_wrapper_startup() {
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            current_agent: None,
            acquisition_age: Some(std::time::Duration::from_millis(1250)),
            elapsed_since_process_check: PROCESS_ACQUISITION_FAST_RECHECK
                - std::time::Duration::from_millis(1),
            ..process_probe_input()
        }));
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: None,
            acquisition_age: Some(std::time::Duration::from_millis(1250)),
            elapsed_since_process_check: PROCESS_ACQUISITION_FAST_RECHECK,
            ..process_probe_input()
        }));
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: None,
            acquisition_age: Some(std::time::Duration::from_secs(5)),
            elapsed_since_process_check: PROCESS_ACQUISITION_SLOW_RECHECK,
            ..process_probe_input()
        }));
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            current_agent: None,
            acquisition_age: Some(PROCESS_ACQUISITION_WINDOW + std::time::Duration::from_millis(1),),
            elapsed_since_process_check: PROCESS_ACQUISITION_SLOW_RECHECK,
            ..process_probe_input()
        }));
    }

    #[test]
    fn content_change_starts_bounded_unidentified_acquisition_window() {
        let now = std::time::Instant::now();
        let mut acquisition_started_at = None;
        let mut last_content_change_at = None;

        sync_content_change_acquisition(
            None,
            false,
            true,
            now,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );
        assert_eq!(acquisition_started_at, Some(now));
        assert_eq!(last_content_change_at, Some(now));

        let later = now + std::time::Duration::from_secs(1);
        sync_content_change_acquisition(
            None,
            false,
            true,
            later,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );
        assert_eq!(
            acquisition_started_at,
            Some(now),
            "changed frames should not refresh the acquisition window"
        );
        assert_eq!(last_content_change_at, Some(later));

        let quiet_after_window =
            later + PROCESS_ACQUISITION_WINDOW + PROCESS_ACQUISITION_IDLE_RESET;
        sync_content_change_acquisition(
            None,
            false,
            false,
            quiet_after_window,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );
        assert_eq!(acquisition_started_at, None);
        assert_eq!(last_content_change_at, None);

        let next_burst = quiet_after_window + std::time::Duration::from_secs(1);
        sync_content_change_acquisition(
            None,
            false,
            true,
            next_burst,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );
        assert_eq!(acquisition_started_at, Some(next_burst));
        assert_eq!(last_content_change_at, Some(next_burst));
    }

    #[test]
    fn content_change_does_not_start_acquisition_when_process_probe_has_other_signal() {
        let now = std::time::Instant::now();
        let mut acquisition_started_at = None;
        let mut last_content_change_at = None;

        sync_content_change_acquisition(
            Some(Agent::Codex),
            false,
            true,
            now,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );
        assert_eq!(acquisition_started_at, None);
        assert_eq!(last_content_change_at, None);

        sync_content_change_acquisition(
            None,
            true,
            true,
            now,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );
        assert_eq!(acquisition_started_at, None);
        assert_eq!(last_content_change_at, None);
    }

    #[test]
    fn content_change_restarts_stale_process_group_acquisition_window() {
        let now = std::time::Instant::now();
        let stale_start = now - PROCESS_ACQUISITION_WINDOW - std::time::Duration::from_millis(1);
        let mut acquisition_started_at = Some(stale_start);
        let mut last_content_change_at = None;

        sync_content_change_acquisition(
            None,
            false,
            true,
            now,
            &mut acquisition_started_at,
            &mut last_content_change_at,
        );

        assert_eq!(acquisition_started_at, Some(now));
        assert_eq!(last_content_change_at, Some(now));
    }

    #[test]
    fn identified_agent_uses_shorter_safety_process_probe() {
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED
                - std::time::Duration::from_millis(1),
            ..process_probe_input()
        }));
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED,
            ..process_probe_input()
        }));
    }

    #[test]
    fn identified_agent_probes_when_foreground_group_disappears() {
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            foreground_pgid: None,
            last_foreground_pgid: Some(42),
            elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED
                - std::time::Duration::from_millis(1),
            ..process_probe_input()
        }));
    }

    #[test]
    fn stable_missing_foreground_group_uses_safety_process_probe() {
        assert!(!should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            foreground_pgid: None,
            last_foreground_pgid: None,
            elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED
                - std::time::Duration::from_millis(1),
            ..process_probe_input()
        }));
        assert!(should_probe_foreground_job(ProcessProbeInput {
            current_agent: Some(Agent::Codex),
            foreground_pgid: None,
            last_foreground_pgid: None,
            elapsed_since_process_check: PROCESS_RECHECK_IDENTIFIED,
            ..process_probe_input()
        }));
    }

    #[test]
    fn transient_process_miss_keeps_current_agent_detected() {
        let mut presence = AgentDetectionPresence::from_agent(Some(Agent::Pi));

        let changed = presence.observe_process_probe(None);

        assert!(!changed, "one miss should not clear the detected agent");
        assert_eq!(presence.current_agent(), Some(Agent::Pi));
    }

    #[test]
    fn agent_only_clears_after_confirmation_misses() {
        let mut presence = AgentDetectionPresence::from_agent(Some(Agent::Pi));

        for attempt in 1..AGENT_MISS_CONFIRMATION_ATTEMPTS {
            let changed = presence.observe_process_probe(None);
            assert!(
                !changed,
                "miss {attempt} should stay in the confirmation window"
            );
            assert_eq!(presence.current_agent(), Some(Agent::Pi));
        }

        let changed = presence.observe_process_probe(None);
        assert!(changed, "last confirmation miss should clear the agent");
        assert_eq!(presence.current_agent(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawned_pty_reader_aggregates_terminal_bells() {
        let (events, mut event_rx) = mpsc::channel(8);
        let pane_id = PaneId::from_raw(42);
        let runtime = PaneRuntime::spawn_shell_command(
            pane_id,
            24,
            80,
            std::env::temp_dir(),
            "printf '\\a\\a'; sleep 0.05",
            &PaneLaunchEnv::default(),
            AgentDetection::Disabled,
            0,
            crate::terminal_theme::TerminalTheme::default(),
            None,
            events,
            Arc::new(Notify::new()),
            Arc::new(RenderSignal::new()),
        )
        .unwrap();

        let bell = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Some(AppEvent::TerminalBell {
                    pane_id: delivered_pane,
                    count,
                }) = event_rx.recv().await
                {
                    break (delivered_pane, count);
                }
            }
        })
        .await
        .expect("PTY reader should publish terminal bells");

        assert_eq!(bell, (pane_id, 2));
        runtime.shutdown();
    }

    #[tokio::test]
    async fn state_changed_event_waits_for_queue_space_instead_of_dropping() {
        let (tx, mut rx) = mpsc::channel(1);
        let pane_id = PaneId::from_raw(42);

        tx.try_send(AppEvent::ClipboardWrite {
            content: Vec::new(),
        })
        .unwrap();

        let publish = publish_state_changed_event(
            tx.clone(),
            pane_id,
            Some(Agent::Pi),
            AgentState::Idle,
            false,
            false,
            std::time::Instant::now(),
        );
        tokio::pin!(publish);

        let blocked = tokio::time::timeout(std::time::Duration::from_millis(20), async {
            (&mut publish).await;
        })
        .await;
        assert!(
            blocked.is_err(),
            "publisher should wait for queue space instead of dropping StateChanged"
        );

        let first = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
            .await
            .expect("queue should yield first event")
            .expect("sender still alive");
        assert!(matches!(first, AppEvent::ClipboardWrite { .. }));

        tokio::time::timeout(std::time::Duration::from_millis(50), async {
            (&mut publish).await;
        })
        .await
        .expect("publisher should complete once queue space is available");

        let second = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
            .await
            .expect("queue should yield second event")
            .expect("sender still alive");
        assert!(matches!(
            second,
            AppEvent::StateChanged {
                pane_id: delivered_pane,
                agent: Some(Agent::Pi),
                state: AgentState::Idle,
                visible_blocker: false,
                process_exited: false,
                observed_at: _,
            } if delivered_pane == pane_id
        ));
    }
}
