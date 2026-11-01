use super::*;

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
        "bus-pane-term-test-{}-{}-{}.txt",
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

fn foreground_process(pid: u32, name: &str) -> crate::platform::ForegroundProcess {
    crate::platform::ForegroundProcess {
        pid,
        name: name.to_string(),
        argv0: None,
        argv: None,
        cmdline: None,
    }
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

#[path = "compression_test.rs"]
mod compression;
#[path = "detection_test.rs"]
mod detection;
#[path = "io_test.rs"]
mod io;
#[path = "kimi_command_screen_test.rs"]
mod kimi_command_screen;
#[path = "shutdown_test.rs"]
mod shutdown;
#[path = "spawn_test.rs"]
mod spawn;

#[derive(Default)]
pub(super) struct DialogWaitTest {
    timeout: Option<std::time::Duration>,
    observed: Option<std::sync::mpsc::Sender<()>>,
}

#[cfg(test)]
impl TerminalRuntime {
    pub(crate) fn test_kimi_command_detection(&self) -> Option<crate::agents::AgentDetection> {
        let content = self.terminal.detection_text();
        let screen = crate::agents::detect_agent_with_osc(Some(AgentKind::Kimi), &content, "", "");
        let home = self.kimi_code_home.lock().unwrap().clone();
        super::detection_task::classify_kimi_command_screen(
            &self.terminal,
            home.as_deref(),
            &mut Default::default(),
            &content,
            screen,
        )
    }

    /// Arm a one-shot signal for the first unfinished frame the guard sees.
    /// Only this runtime's tests receive the generous completion budget.
    pub(crate) fn test_observe_dialog_redraw(
        &self,
        timeout: std::time::Duration,
    ) -> std::sync::mpsc::Receiver<()> {
        let (sender, receiver) = std::sync::mpsc::channel();
        *self.dialog_wait_test.lock().unwrap() = DialogWaitTest {
            timeout: Some(timeout),
            observed: Some(sender),
        };
        receiver
    }

    pub(super) fn test_dialog_wait_timeout(
        &self,
        default: std::time::Duration,
    ) -> std::time::Duration {
        self.dialog_wait_test
            .lock()
            .unwrap()
            .timeout
            .unwrap_or(default)
    }

    pub(super) fn test_signal_dialog_redraw(&self) {
        let sender = self.dialog_wait_test.lock().unwrap().observed.take();
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
    }

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
        let _ = self
            .terminal
            .process_pty_bytes(self.pane_id, 0, bytes, &tx, |_| None);
        self.content_seq.fetch_add(1, Ordering::Release);
        self.compression.wake();
        if let Some(clock) = self.test_submission_clock() {
            clock.rendered();
        }
    }

    pub(crate) fn test_submission_clock(&self) -> Option<super::SubmissionClock> {
        match &self.io {
            PaneRuntimeIo::TestChannel { clock, .. } => Some(clock.clone()),
            PaneRuntimeIo::Actor(_) => None,
        }
    }

    /// A detached PTY chunk writer for tests that finish a frame while the
    /// server's guarded write is waiting. Uses the same content lock and seq.
    pub(crate) fn test_pty_writer(&self) -> impl Fn(&[u8]) + Send + 'static {
        let terminal = self.terminal.clone();
        let lock = self.content_write_lock.clone();
        let seq = self.content_seq.clone();
        let clock = self.test_submission_clock();
        let pane = self.pane_id;
        move |bytes| {
            let _guard = lock.lock().unwrap_or_else(|error| error.into_inner());
            seq.fetch_add(1, Ordering::AcqRel);
            let (tx, _rx) = mpsc::channel(1);
            let _ = terminal.process_pty_bytes(pane, 0, bytes, &tx, |_| None);
            seq.fetch_add(1, Ordering::Release);
            if let Some(clock) = &clock {
                clock.rendered();
            }
        }
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
            crate::terminal::vt::Terminal::new(cols, rows, scrollback_limit_bytes).unwrap();
        terminal.write(bytes);
        let pane_id = PaneId::from_raw(0);
        let terminal = Arc::new(PaneTerminal::new(
            GhosttyPaneTerminal::new(terminal, tx.clone()).unwrap(),
        ));
        let compression = TerminalCompressionTask::spawn(pane_id, terminal.clone());
        let (draft_input, startup_suggestion_open) =
            super::draft_input::DraftInput::shared_with_launch_window();

        (
            Self {
                pane_id,
                terminal,
                io: PaneRuntimeIo::TestChannel {
                    sender: tx,
                    resize_tx,
                    clock: super::SubmissionClock::default(),
                },
                current_size: Cell::new((rows, cols, 0, 0)),
                child_pid: Arc::new(AtomicU32::new(0)),
                reported_cwd: Arc::new(Mutex::new(None)),
                child_wait_completed: None,
                kitty_keyboard_flags: Arc::new(AtomicU16::new(0)),
                content_seq: Arc::new(AtomicU64::new(0)),
                content_write_lock: Arc::new(Mutex::new(())),
                draft_input,
                driver_state: Default::default(),
                dialog_wait_test: Mutex::default(),
                detection_content_seq: Arc::new(AtomicU64::new(0)),
                managed_launch_pending: Arc::new(AtomicBool::new(false)),
                detection_context: Default::default(),
                kimi_code_home: Arc::new(Mutex::new(None)),
                preserve_processes_on_drop: true,
                shell_integration: None,
                launch_prompt_generation: Arc::new(AtomicU64::new(0)),
                startup_suggestion_open,
                launch_input_error: Arc::new(Mutex::new(None)),
                launch_input_submitted: Arc::new(AtomicBool::new(false)),
                compression,
                detect_handle: Some(tokio::spawn(async {}).abort_handle()),
            },
            rx,
        )
    }
}

/// Serializes tests that mutate process environment variables such as HOME or APPDATA.
#[cfg(test)]
#[cfg(unix)]
pub(crate) use crate::utils::test_env::env_lock;
