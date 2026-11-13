//! Live terminal runtime: construction, IO, reads and background task ownership.
mod cli_env;
pub(crate) use cli_env::prepare_bus_cli;
mod compression;
mod detection_process;
mod detection_task;
mod draft_input;
mod io;
mod launch_env;
mod launch_transaction;
mod read;
mod screen_detection;
mod shell_integration;
mod shell_launch;
#[cfg(test)]
pub(crate) use shell_launch::LaunchTestBarrier;
mod shutdown;
mod submission;
mod submission_clock;
#[cfg(test)]
pub(crate) use submission::tests::codex_clipped_composer;
pub(crate) use submission::{
    DraftPreservation, EditorSubmission, InputObservation, PromptNotShown, SubmissionInspector,
};
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

mod detection_context;
mod detection_policy;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentDetection {
    Enabled,
    #[cfg(all(test, unix))]
    Disabled,
}

#[cfg(unix)]
pub(crate) mod native_input;

/// PTY runtime for a pane. Owns the terminal, I/O channels, and background tasks.
/// Dropping this aborts async tasks and closes the PTY. An already-running bounded
/// compression step may finish before releasing its terminal reference.
pub struct TerminalRuntime {
    pane_id: PaneId,
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    current_size: Cell<(u16, u16, u32, u32)>,
    child_pid: Arc<AtomicU32>,
    child_instance: Option<crate::platform::ProcessInstance>,
    reported_cwd: Arc<Mutex<Option<std::path::PathBuf>>>,
    child_wait_completed: Option<Arc<AtomicBool>>,
    kitty_keyboard_flags: Arc<AtomicU16>,
    content_seq: Arc<AtomicU64>,
    content_write_lock: Arc<Mutex<()>>,
    draft_input: draft_input::SharedDraftInput,
    /// Semantic navigation state is owned exclusively by the terminal driver.
    driver_state: crate::terminal::driver::DriverState,
    detection_content_seq: Arc<AtomicU64>,
    screen_detection: Arc<screen_detection::ScreenDetectionCache>,
    managed_launch_pending: Arc<AtomicBool>,
    detection_context: Arc<Mutex<detection_context::DetectionContext>>,
    kimi_code_home: Arc<Mutex<Option<std::path::PathBuf>>>,
    /// Pane-owned Codex runtime. One `Option` created by `from_environment`.
    /// Admission clones this handle onto its worker so the app thread does not
    /// wait on the socket. Dropping the last handle closes the helper lifetime
    /// channel: immediately when no admission call is in flight, and after that
    /// call when one is.
    #[cfg(unix)]
    shared_codex: Arc<Mutex<Option<crate::agents::providers::codex::native::Runtime>>>,
    /// Test stand-in for the admission port. Empty in production tests, so
    /// admission still uses the pane runtime above.
    #[cfg(all(test, unix))]
    native_input_override: Mutex<Option<native_input::NativeInputHandle>>,
    preserve_processes_on_drop: bool,
    shell_integration: Option<shell_integration::ShellIntegration>,
    launch_transaction: launch_transaction::LaunchTransaction,
    /// True until developer or API bytes are queued to this pane, or Bus
    /// successfully pastes a prompt, since the last agent launch. A
    /// preservation buffer that is never queued leaves it set. Dialog
    /// automation, mouse, and focus do not clear it. A rejected send leaves
    /// it set. Only then is Claude's `Try "..."` shape a placeholder.
    startup_suggestion_open: Arc<AtomicBool>,
    #[cfg(test)]
    launch_test_barrier: Arc<Mutex<Option<Arc<shell_launch::LaunchTestBarrier>>>>,
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
    pub(super) fn driver_state(&self) -> &crate::terminal::driver::DriverState {
        &self.driver_state
    }

    /// Native launches into an existing shell must update the detector as well
    /// as the command's environment.
    pub(crate) fn bind_kimi_code_home(&self, home: Option<std::path::PathBuf>) {
        *self
            .kimi_code_home
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = home;
    }

    #[cfg(test)]
    pub(crate) fn shell_prompt_diagnostics(&self) -> String {
        format!(
            "integration={:?} current_image={:?} exited={:?} generation={}",
            self.shell_integration,
            self.child_pid().and_then(crate::platform::process_image),
            self.child_wait_completed
                .as_ref()
                .map(|done| done.load(std::sync::atomic::Ordering::Acquire)),
            self.shell_prompt_generation()
        ) + &self.launch_diagnostics()
    }

    pub(crate) fn has_launch_shell_integration(&self) -> bool {
        self.shell_integration.is_some()
    }

    pub(crate) fn waiting_for_shell_prompt(&self) -> bool {
        self.shell_integration.as_ref().is_some_and(|integration| {
            integration.waiting_for_prompt(self.child_pid(), self.shell_prompt_generation())
        })
    }

    pub(crate) fn shell_prompt_wait_expired(&self) -> bool {
        self.shell_integration
            .as_ref()
            .is_some_and(|integration| integration.wait_expired())
    }

    fn shell_prompt_generation(&self) -> u64 {
        self.shell_integration.as_ref().map_or_else(
            || self.terminal.shell_prompt_generation(),
            |integration| {
                self.terminal
                    .launch_shell_prompt_generation(integration.token())
            },
        )
    }

    fn invalidate_shell_prompt(&self) {
        if let Some(integration) = &self.shell_integration {
            integration.invalidate_prompt(self.shell_prompt_generation());
        }
    }

    pub(crate) fn mark_agent_launch(&self) {
        self.startup_suggestion_open.store(true, Ordering::Release);
        self.invalidate_submission_ownership();
    }

    fn launch_prompt_witness(&self) -> Option<(u64, u64)> {
        // Only the shell's report of this attempt is a return. A bare prompt
        // mark, including an editor redraw before the provider is acquired, is
        // not. Runtimes without integration do not invent a witness from the
        // prompt generation; that would end pending startup on a redraw.
        self.shell_integration.as_ref().and_then(|integration| {
            self.terminal
                .launch_shell_prompt_witness(integration.token())
        })
    }

    pub(crate) fn has_launch_return_witness(&self) -> bool {
        self.launch_transaction
            .has_return_witness(self.launch_prompt_witness())
    }

    pub(crate) fn reconcile_launch_attempt(&self, live_provider: bool) -> bool {
        self.launch_transaction
            .returned(self.launch_prompt_witness(), live_provider)
    }

    #[cfg(test)]
    pub(crate) fn launch_returned_to_prompt(&self) -> bool {
        self.reconcile_launch_attempt(false)
    }

    pub(crate) fn launch_diagnostics(&self) -> String {
        self.launch_transaction
            .diagnostics(self.launch_prompt_witness())
    }

    #[cfg(test)]
    pub(crate) fn test_prompt_observed(&self) -> bool {
        self.shell_prompt_generation() > 0
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

    /// Keep the runtime `from_environment` accepts. A later pane start is the
    /// only caller of `launch`; this does not admit prompts or interrupts.
    #[cfg(unix)]
    pub(crate) fn retain_shared_codex_from_env(&self, env: &[(String, String)]) {
        if let Some(runtime) =
            crate::agents::providers::codex::native::Runtime::from_environment(env)
        {
            *self
                .shared_codex
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(runtime);
        }
    }

    #[cfg(unix)]
    pub(crate) fn launch_shared_codex(&self, argv: Vec<String>) -> std::io::Result<Vec<String>> {
        match self
            .shared_codex
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
        {
            Some(runtime) => runtime.launch(&argv),
            None => Ok(argv),
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn shared_codex_session_open(&self) -> bool {
        self.shared_codex
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(|runtime| runtime.has_session())
    }

    /// Clone of the pane runtime handle. The worker may lock it; it is not a
    /// second runtime.
    #[cfg(unix)]
    pub(crate) fn shared_codex_handle(
        &self,
    ) -> Arc<Mutex<Option<crate::agents::providers::codex::native::Runtime>>> {
        Arc::clone(&self.shared_codex)
    }

    /// Admission port over the pane runtime. Always present so a busy lock can
    /// still move onto a worker; an empty slot reports no session.
    #[cfg(unix)]
    pub(crate) fn native_input(&self) -> Option<native_input::NativeInputHandle> {
        #[cfg(test)]
        if let Some(input) = self
            .native_input_override
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
        {
            return Some(input);
        }
        Some(Arc::new(native_input::PaneCodex::new(
            self.shared_codex_handle(),
        )))
    }

    /// Install a test admission port. This does not construct a runtime.
    #[cfg(all(test, unix))]
    pub(crate) fn bind_native_input(&self, input: native_input::NativeInputHandle) {
        *self
            .native_input_override
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(input);
    }
}
