//! Live terminal runtime: construction, IO, reads and background task ownership.
mod compression;
mod detection_process;
mod detection_task;
mod dialog;
mod io;
mod read;
mod shell_integration;
mod shutdown;
mod submission;
mod submission_clock;
pub(crate) use submission::{InputObservation, PromptNotShown};
#[cfg(test)]
pub(crate) use submission_clock::{SubmissionClock, NATIVE_TEST_TIMEOUT};
pub(crate) mod spawn;

#[cfg(all(test, unix))]
pub(crate) mod env {
    pub(crate) use super::tests::env_lock;
}
#[cfg(test)]
use crate::agents::AgentKind;
#[cfg(test)]
use crate::agents::AgentState;
#[cfg(test)]
use crate::terminal::emulator::GhosttyPaneTerminal;
use crate::terminal::emulator::PaneTerminal;
#[cfg(test)]
use crate::terminal::events::TerminalEvent;
use crate::terminal::runtime::compression::TerminalCompressionTask;
use crate::terminal::runtime::io::PaneRuntimeIo;
use crate::utils::ids::PaneId;
#[cfg(all(test, unix))]
use crate::utils::render::signal::RenderSignal;
#[cfg(test)]
use bytes::Bytes;
#[cfg(test)]
use compression::spawn_blocking_with_compression_permit;
#[cfg(test)]
#[cfg(unix)]
use detection_process::absolute_process_cwd;
#[cfg(test)]
use detection_process::agent_hint_for_foreground_job_members;
#[cfg(test)]
use detection_process::clear_osc_evidence_for_agent_transition;
#[cfg(test)]
use detection_process::foreground_shell_agent_action;
#[cfg(test)]
use detection_process::probe_foreground_process_from_jobs;
#[cfg(test)]
use detection_process::process_group_for_change_tracking;
#[cfg(test)]
#[cfg(any(windows, test))]
use detection_process::should_observe_foreground_process_group;
#[cfg(test)]
use detection_process::should_probe_foreground_job;
#[cfg(test)]
use detection_process::sync_content_change_acquisition;
#[cfg(test)]
use detection_process::track_agent_leader;
#[cfg(test)]
use detection_process::AgentDetectionPresence;
#[cfg(test)]
use detection_process::ForegroundShellAgentAction;
#[cfg(test)]
use detection_process::ProcessProbeInput;
#[cfg(test)]
use detection_process::AGENT_MISS_CONFIRMATION_ATTEMPTS;
#[cfg(test)]
use detection_process::PROCESS_ACQUISITION_FAST_RECHECK;
#[cfg(test)]
use detection_process::PROCESS_ACQUISITION_IDLE_RESET;
#[cfg(test)]
use detection_process::PROCESS_ACQUISITION_SLOW_RECHECK;
#[cfg(test)]
use detection_process::PROCESS_ACQUISITION_WINDOW;
#[cfg(test)]
use detection_process::PROCESS_RECHECK_IDENTIFIED;
#[cfg(test)]
use detection_process::PROCESS_RECHECK_MISSING_FOREGROUND_GROUP;
#[cfg(test)]
use detection_task::publish_state_changed_event;
pub(crate) use dialog::DialogChoice;
#[cfg(all(test, unix))]
use portable_pty::native_pty_system;
#[cfg(test)]
use portable_pty::CommandBuilder;
#[cfg(all(test, unix))]
use portable_pty::PtySize;
#[cfg(test)]
use read::publish_reported_cwd;
#[cfg(test)]
use shutdown::process_alive_for_shutdown;
#[cfg(test)]
use spawn::apply_pane_launch_env;
#[cfg(test)]
use spawn::apply_pane_terminal_env;
#[cfg(test)]
use spawn::default_pane_shell;
#[cfg(test)]
use spawn::pane_shell_command_builder;
#[cfg(test)]
use spawn::pane_shell_command_builder_for_target;
#[cfg(test)]
use spawn::pane_shell_from;
#[cfg(all(test, unix))]
use spawn::resolve_shell_for_login_mode;
#[cfg(test)]
use spawn::shell_mode_uses_login_shell;
#[cfg(windows)]
pub(crate) use spawn::uses_windows_powershell_pane_shell;
#[cfg(test)]
use spawn::uses_windows_powershell_pane_shell_for_target;
pub(crate) use spawn::PaneLaunchEnv;
pub(crate) use spawn::PaneShellConfig;
#[cfg(test)]
use spawn::ShellLaunchTarget;
#[cfg(test)]
pub(crate) use spawn::WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND;
use std::cell::Cell;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(test)]
use tokio::sync::mpsc;
#[cfg(test)]
use tokio::sync::watch;
#[cfg(all(test, unix))]
use tokio::sync::Notify;

mod detection_policy;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentDetection {
    Enabled,
    #[cfg(all(test, unix))]
    Disabled,
}

/// PTY runtime for a pane. Owns the terminal, I/O channels, and background tasks.
/// Dropping this aborts async tasks and closes the PTY. An already-running bounded
/// compression step may finish before releasing its terminal reference.
pub struct TerminalRuntime {
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
    #[cfg(test)]
    dialog_wait_test: Mutex<tests::DialogWaitTest>,
    detection_content_seq: Arc<AtomicU64>,
    managed_launch_pending: Arc<AtomicBool>,
    preserve_processes_on_drop: bool,
    shell_integration: Option<shell_integration::ShellIntegration>,
    launch_prompt_generation: std::cell::Cell<u64>,
    // Task handles for deterministic shutdown
    compression: TerminalCompressionTask,
    detect_handle: Option<tokio::task::AbortHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelRouting {
    HostScroll,
    MouseReport,
    AlternateScroll,
}

#[cfg(test)]
#[path = "tests/support_test.rs"]
mod tests;

impl TerminalRuntime {
    pub(crate) fn waiting_for_shell_prompt(&self) -> bool {
        if self.shell_integration.is_none() || self.terminal.shell_prompt_generation() > 0 {
            return false;
        }
        self.child_pid()
            .and_then(crate::agents::foreground_job)
            .is_none_or(|job| self.shell_prompt_required_for_job(&job))
    }

    pub(crate) fn shell_prompt_required_for_job(
        &self,
        job: &crate::platform::ForegroundJob,
    ) -> bool {
        self.terminal.shell_prompt_generation() == 0
            && self
                .shell_integration
                .as_ref()
                .is_some_and(|integration| integration.shell_still_foreground(job))
    }

    pub(crate) fn mark_agent_launch(&self) {
        self.launch_prompt_generation
            .set(self.terminal.shell_prompt_generation());
    }

    pub(crate) fn launch_returned_to_prompt(&self) -> bool {
        self.terminal.shell_prompt_generation() > self.launch_prompt_generation.get()
    }

    pub fn apply_host_terminal_theme(&self, theme: crate::utils::theme::color::TerminalTheme) {
        self.terminal.apply_host_terminal_theme(theme);
    }

    pub fn apply_host_terminal_appearance(
        &self,
        appearance: Option<crate::utils::theme::color::HostAppearance>,
    ) {
        self.io
            .write_terminal_response(|| self.terminal.apply_host_terminal_appearance(appearance));
    }

    pub(crate) fn current_size(&self) -> (u16, u16) {
        let (rows, cols, _, _) = self.current_size.get();
        (rows, cols)
    }

    pub(crate) fn managed_launch_pending_signal(&self) -> Arc<AtomicBool> {
        self.managed_launch_pending.clone()
    }

    pub(crate) fn content_seq(&self) -> u64 {
        self.content_seq.load(Ordering::Acquire)
    }
}
