//! Every provider launch is an owned, verified interactive-shell input transaction.
use super::launch_transaction::{LaunchPhase, LaunchTransaction};
use super::{io::PaneRuntimeIo, TerminalRuntime};
use crate::terminal::{emulator::PaneTerminal, pty::actor::InputScope};
use bytes::Bytes;
use std::{
    io,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT_PROBE: AtomicU64 = AtomicU64::new(0);

pub(super) struct EditorProbe {
    pub(super) directory: PathBuf,
    pub(super) token: String,
    pub(super) shell: String,
    pub(super) floor: u64,
    pub(super) pid: Option<u32>,
    pub(super) process: Option<crate::platform::ProcessInstance>,
    pub(super) image: Option<PathBuf>,
    pub(super) windows: bool,
}

impl EditorProbe {
    fn ready(&self, terminal: &PaneTerminal) -> bool {
        self.pid.is_some_and(|pid| {
            self.process.is_some()
                && crate::platform::process_instance(pid) == self.process
                && if self.windows {
                    crate::platform::available_pane_shell(pid)
                        .is_some_and(|name| super::spawn::is_powershell_shell(&name))
                } else {
                    self.image.is_some() && crate::platform::process_image(pid) == self.image
                }
        }) && terminal.launch_shell_prompt_generation(&self.token) > self.floor
    }

    pub(super) fn key(&self, action: char) -> Bytes {
        if self.shell == "fish" || super::spawn::is_powershell_shell(&self.shell) {
            Bytes::from(format!("\x1bz{action}"))
        } else {
            Bytes::from(format!("\x1b[{}{action}~", self.token))
        }
    }

    fn attempt_command(&self, id: u64, command: &str) -> String {
        if super::spawn::is_powershell_shell(&self.shell) {
            format!("$global:__bus_launch_attempt={id}; {command}")
        } else {
            // Bash's capture macro completes the first command on the line.
            // Keep a single simple command so completion still calls our probe.
            format!("__bus_launch {id} {command}")
        }
    }

    fn capture(&self, writer: &ShellWriter) -> io::Result<Vec<u8>> {
        let challenge = NEXT_PROBE.fetch_add(1, Ordering::Relaxed).to_string();
        std::fs::write(self.directory.join("request"), format!("{challenge}\n"))?;
        writer.write(self.key('v'))?;
        let prefix = format!("{challenge}\n");
        loop {
            writer.check_deadline()?;
            if let Ok(bytes) = std::fs::read(self.directory.join("capture")) {
                if let Some(line) = bytes.strip_prefix(prefix.as_bytes()) {
                    // fish's commandline builtin terminates its raw buffer
                    // with one newline. Capture directly so shell substitution
                    // and field splitting cannot change the witness bytes.
                    let line = if self.shell == "fish" {
                        line.strip_suffix(b"\n")
                    } else {
                        Some(line)
                    };
                    if let Some(line) = line {
                        return Ok(line.to_vec());
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

struct ShellWriter {
    io: PaneRuntimeIo,
    scope: InputScope,
    deadline: Instant,
}

impl ShellWriter {
    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut,
                "Shell editor did not confirm the exact launch line; Enter was not sent. Inspect the retained pane before resuming."));
        }
        Ok(())
    }

    fn write(&self, bytes: Bytes) -> io::Result<()> {
        self.check_deadline()?;
        self.io
            .queue_input_write_scoped(
                bytes,
                Bytes::new(),
                Duration::ZERO,
                Some(self.deadline),
                &self.scope,
            )?
            .wait_for_api(self.deadline.saturating_duration_since(Instant::now()))
    }

    fn wait_ready(&self, probe: &EditorProbe, terminal: &PaneTerminal) -> io::Result<()> {
        while !probe.ready(terminal) {
            self.check_deadline()?;
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn run(&self, probe: &EditorProbe, terminal: &PaneTerminal, command: &str) -> io::Result<()> {
        // A marker proves primary editor readiness; the private widget then
        // proves the actual editable buffer, rather than trusting kernel echo.
        if !probe.capture(self)?.is_empty() {
            return Err(io::Error::other(
                "Shell editor already has input; launch was not typed and Enter was not sent",
            ));
        }
        let text = crate::platform::prepare_paste_text_for_pty(command.to_owned());
        let paste =
            crate::terminal::driver::input::paste(&text, terminal.bracketed_paste_enabled());
        self.write(Bytes::from(paste))?;
        if probe.capture(self)? != command.as_bytes() {
            // The first paste was transformed (e.g. a ZLE bracketed-paste
            // widget dropped a byte). Clear only our initially empty buffer,
            // retype without that paste widget, and verify again before Enter.
            self.write(probe.key('c'))?;
            if !probe.capture(self)?.is_empty() {
                return Err(io::Error::other(
                    "Could not clear the unsubmitted launch line; Enter was not sent",
                ));
            }
            self.write(Bytes::copy_from_slice(command.as_bytes()))?;
            if probe.capture(self)? != command.as_bytes() {
                self.write(probe.key('c'))?;
                return Err(io::Error::other("Shell transformed the launch line twice; Enter was not sent. Inspect the retained pane"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct LaunchTestBarrier {
    reached: std::sync::atomic::AtomicBool,
    pre_enter: Option<std::sync::Arc<LaunchTestBarrier>>,
    released: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
}

#[cfg(test)]
impl LaunchTestBarrier {
    fn hold(&self) {
        self.reached.store(true, Ordering::Release);
        let guard = self.released.lock().unwrap();
        drop(self.wake.wait_while(guard, |released| !*released).unwrap());
    }

    fn before_enter(&self) {
        self.pre_enter.as_deref().unwrap_or(self).hold();
    }

    pub(crate) fn verification_reached(&self) -> bool {
        self.pre_enter.as_deref().unwrap_or(self).reached()
    }

    pub(crate) fn release_verification(&self) {
        self.pre_enter.as_ref().unwrap().release();
    }

    pub(crate) fn reached(&self) -> bool {
        self.reached.load(Ordering::Acquire)
    }

    pub(crate) fn release(&self) {
        if let Some(pre) = &self.pre_enter {
            pre.release();
        }
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

impl TerminalRuntime {
    #[cfg(test)]
    pub(crate) fn test_hold_launch_input(
        &self,
        after_enter: bool,
    ) -> std::sync::Arc<LaunchTestBarrier> {
        let barrier = std::sync::Arc::new(LaunchTestBarrier {
            pre_enter: after_enter.then(|| std::sync::Arc::new(LaunchTestBarrier::default())),
            ..Default::default()
        });
        *self.launch_test_barrier.lock().unwrap() = Some((after_enter, barrier.clone()));
        barrier
    }

    #[cfg(test)]
    pub(crate) fn test_shell_prompt_marker(&self) -> Vec<u8> {
        let token = self.shell_integration.as_ref().unwrap().token();
        let (cycle, attempt) = self.terminal.launch_shell_prompt_witness(token).unwrap();
        format!("\x1b]133;A;bus={token};cycle={cycle};attempt={attempt}\x07").into_bytes()
    }

    #[cfg(test)]
    pub(crate) fn test_launch_input_submitted(&self) -> bool {
        self.launch_transaction.input_confirmed()
    }

    pub(crate) fn launch_input_error(&self) -> Option<String> {
        self.launch_transaction.error(false)
    }

    pub(crate) fn definite_launch_input_error(&self) -> Option<String> {
        self.launch_transaction.error(true)
    }

    #[cfg(test)]
    pub(crate) fn test_capture_shell_editor(&self) -> io::Result<Vec<u8>> {
        let probe = self
            .shell_integration
            .as_ref()
            .ok_or_else(|| io::Error::other("missing editor"))?
            .editor_probe(self.child_pid());
        probe.capture(&ShellWriter {
            io: self.io.clone(),
            scope: self.io.input_scope(),
            deadline: Instant::now() + Duration::from_secs(3),
        })
    }

    pub(crate) fn queue_shell_command(&self, argv: &[String], timeout: Duration) -> io::Result<()> {
        if argv.is_empty() || argv.iter().any(|arg| arg.chars().any(char::is_control)) {
            return Err(io::Error::other("Invalid shell launch arguments"));
        }
        #[cfg(test)]
        if self.child_pid().is_none() {
            let id = self
                .launch_transaction
                .begin(self.pane_id.raw(), self.shell_prompt_generation());
            let cycle = self.shell_prompt_generation();
            let command = crate::platform::interactive_shell_command(argv, "sh")
                .ok_or_else(|| io::Error::other("Invalid shell command"))?;
            let mut bytes = self.paste_payload(command).to_vec();
            bytes.push(b'\r');
            self.launch_transaction
                .transition(id, LaunchPhase::Submitting, cycle);
            let result = self.try_send_bytes(Bytes::from(bytes));
            if result.is_ok() {
                self.launch_transaction
                    .transition(id, LaunchPhase::Submitted, cycle);
            }
            return result.map_err(io::Error::other);
        }
        let integration = self.shell_integration.as_ref().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported,
            "This pane has no Bus shell integration; no launch input was sent. Create a new Bus pane with zsh, bash, fish or PowerShell and launch there"))?;
        let probe = integration.editor_probe(self.child_pid());
        let command = crate::platform::interactive_shell_command(argv, &probe.shell)
            .ok_or_else(|| io::Error::other("Invalid shell command"))?;
        let id = self
            .launch_transaction
            .begin(self.pane_id.raw(), self.shell_prompt_generation());
        let command = probe.attempt_command(id, &command);
        self.invalidate_shell_prompt();
        let writer = ShellWriter {
            io: self.io.clone(),
            scope: self.io.input_scope(),
            deadline: Instant::now() + timeout,
        };
        let task = ShellLaunch {
            writer,
            probe,
            command,
            terminal: self.terminal.clone(),
            draft_input: self.draft_input.clone(),
            transaction: self.launch_transaction.clone(),
            id,
            #[cfg(test)]
            barrier: self.launch_test_barrier.lock().unwrap().take(),
        };
        std::thread::spawn(move || task.run());
        Ok(())
    }
}

struct ShellLaunch {
    writer: ShellWriter,
    probe: EditorProbe,
    command: String,
    terminal: std::sync::Arc<PaneTerminal>,
    draft_input: super::draft_input::SharedDraftInput,
    transaction: LaunchTransaction,
    id: u64,
    #[cfg(test)]
    barrier: Option<(bool, std::sync::Arc<LaunchTestBarrier>)>,
}

impl ShellLaunch {
    fn run(self) {
        if let Err(error) = self.submit() {
            self.transaction.fail(
                self.id,
                error.to_string(),
                self.terminal.launch_shell_prompt_witness(&self.probe.token),
            );
        }
    }

    fn submit(&self) -> io::Result<()> {
        self.writer.wait_ready(&self.probe, &self.terminal)?;
        let cycle = self
            .terminal
            .launch_shell_prompt_generation(&self.probe.token);
        self.phase(LaunchPhase::Verifying, cycle)?;
        let lease =
            super::draft_input::DraftInput::acquire_launch(&self.draft_input, &self.writer.io)?;
        self.writer
            .run(&self.probe, &self.terminal, &self.command)?;
        let cycle = self
            .terminal
            .launch_shell_prompt_generation(&self.probe.token);
        self.phase(LaunchPhase::Verifying, cycle)?;
        #[cfg(test)]
        if let Some((_, barrier)) = &self.barrier {
            barrier.before_enter();
        }
        self.phase(LaunchPhase::Submitting, cycle)?;
        self.writer.write(Bytes::from_static(b"\r"))?;
        #[cfg(test)]
        if let Some((true, barrier)) = &self.barrier {
            barrier.hold();
        }
        lease.restored();
        self.phase(LaunchPhase::Submitted, cycle)?;
        Ok(())
    }

    fn phase(&self, phase: LaunchPhase, cycle: u64) -> io::Result<()> {
        if self.transaction.transition(self.id, phase, cycle) {
            Ok(())
        } else {
            Err(io::Error::other(
                "Shell launch attempt was replaced; no further input sent",
            ))
        }
    }
}
