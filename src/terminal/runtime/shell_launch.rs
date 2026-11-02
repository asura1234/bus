//! Every provider launch is an owned, verified interactive-shell input transaction.
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

    fn run(
        &self,
        probe: &EditorProbe,
        terminal: &PaneTerminal,
        command: &str,
        prompt_floor: &AtomicU64,
    ) -> io::Result<()> {
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
        prompt_floor.store(
            terminal.launch_shell_prompt_generation(&probe.token),
            Ordering::Release,
        );
        Ok(())
    }
}

impl TerminalRuntime {
    #[cfg(test)]
    pub(crate) fn test_launch_input_submitted(&self) -> bool {
        self.launch_input_submitted.load(Ordering::Acquire)
    }

    pub(crate) fn launch_input_error(&self) -> Option<String> {
        self.launch_input_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .map(|(error, _)| error.clone())
    }

    pub(crate) fn definite_launch_input_error(&self) -> Option<String> {
        self.launch_input_error
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|(error, enter_attempted)| (!enter_attempted).then(|| error.clone()))
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
        self.launch_input_submitted.store(false, Ordering::Release);
        #[cfg(test)]
        if self.child_pid().is_none() {
            let command = crate::platform::interactive_shell_command(argv, "sh")
                .ok_or_else(|| io::Error::other("Invalid shell command"))?;
            let mut bytes = self.paste_payload(command).to_vec();
            bytes.push(b'\r');
            let result = self.try_send_bytes(Bytes::from(bytes));
            if result.is_ok() {
                self.launch_input_submitted.store(true, Ordering::Release);
            }
            return result.map_err(io::Error::other);
        }
        let integration = self.shell_integration.as_ref().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported,
            "This pane has no Bus shell integration; no launch input was sent. Create a new Bus pane with zsh, bash, fish or PowerShell and launch there"))?;
        let probe = integration.editor_probe(self.child_pid());
        let command = crate::platform::interactive_shell_command(argv, &probe.shell)
            .ok_or_else(|| io::Error::other("Invalid shell command"))?;
        *self
            .launch_input_error
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        let input_error = self.launch_input_error.clone();
        self.invalidate_shell_prompt();
        // The initial prompt of a cold restore is readiness, not provider exit.
        self.launch_prompt_generation
            .store(u64::MAX, Ordering::Release);
        let prompt_floor = self.launch_prompt_generation.clone();
        let writer = ShellWriter {
            io: self.io.clone(),
            scope: self.io.input_scope(),
            deadline: Instant::now() + timeout,
        };
        let terminal = self.terminal.clone();
        let draft_input = self.draft_input.clone();
        let pane_id = self.pane_id.raw();
        #[cfg(test)]
        let submitted = self.launch_input_submitted.clone();
        std::thread::spawn(move || {
            let mut enter_attempted = false;
            let result = writer
                .wait_ready(&probe, &terminal)
                .and_then(|()| {
                    super::draft_input::DraftInput::acquire_launch(&draft_input, &writer.io)
                })
                .and_then(|lease| {
                    let result = writer
                        .run(&probe, &terminal, &command, &prompt_floor)
                        .and_then(|()| {
                            enter_attempted = true;
                            writer.write(Bytes::from_static(b"\r"))
                        });
                    if result.is_ok() {
                        lease.restored();
                    }
                    result
                });
            #[cfg(test)]
            if result.is_ok() {
                submitted.store(true, Ordering::Release);
            }
            if let Err(error) = result {
                *input_error
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some((error.to_string(), enter_attempted));
                tracing::warn!(event="bus.agent.launch_input_failed",pane=pane_id,%error,"Launch input was not confirmed; retain the shell for inspection");
            }
        });
        Ok(())
    }
}
