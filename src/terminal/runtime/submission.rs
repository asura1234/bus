//! Observe the composer between writes; retry only Enter for the owned draft.
use super::{io::PaneRuntimeIo, TerminalRuntime};
use crate::terminal::emulator::PaneTerminal;
use bytes::Bytes;
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputObservation {
    Empty,
    Pending,
    Other,
    Unavailable,
    Accepted,
}

/// The paste never rendered as Bus's draft, so no Enter was ever sent and the
/// provider cannot have taken it; the unrecognised draft was cleared. Unlike a
/// plain timeout this is a definite non-delivery, safe to try again.
#[derive(Debug)]
pub(crate) struct PromptNotShown;

impl std::fmt::Display for PromptNotShown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE)
    }
}

impl std::error::Error for PromptNotShown {}

impl PromptNotShown {
    pub(crate) fn is(error: &io::Error) -> bool {
        error
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<Self>().is_some())
    }
}

/// Time allowed after the deadline to clear a paste that never showed.
const WITHDRAW_GRACE: Duration = Duration::from_secs(1);

struct ObservedSubmission {
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    content_lock: Arc<Mutex<()>>,
    inspect: Box<dyn Fn(&str) -> InputObservation + Send>,
    enter: Bytes,
    clear: Vec<Bytes>,
    deadline: std::cell::Cell<Instant>,
}

impl TerminalRuntime {
    /// Completion means the owned composer cleared after Enter, or a turn
    /// visibly began. A PTY write alone cannot confirm provider acceptance.
    pub(crate) fn queue_observed_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        clear: Vec<Bytes>,
        delay: Duration,
        deadline: Option<Instant>,
        inspect: impl Fn(&str) -> InputObservation + Send + 'static,
    ) -> io::Result<std::sync::mpsc::Receiver<io::Result<()>>> {
        let submission = ObservedSubmission {
            terminal: self.terminal.clone(),
            io: self.io.clone(),
            content_lock: self.content_write_lock.clone(),
            inspect: Box::new(inspect),
            enter,
            clear,
            deadline: std::cell::Cell::new(
                deadline.unwrap_or_else(|| Instant::now() + delay + Duration::from_secs(2)),
            ),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(submission.run(text, delay));
        });
        Ok(rx)
    }
}

impl ObservedSubmission {
    fn observe(&self) -> InputObservation {
        (self.inspect)(&self.terminal.visible_ansi())
    }

    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline.get() {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Provider did not accept prompt before submission deadline",
            ))
        } else {
            Ok(())
        }
    }

    fn write(&self, bytes: Bytes, delay: Duration) -> io::Result<()> {
        self.check_deadline()?;
        self.io
            .queue_user_input_submission(bytes, Bytes::new(), delay, Some(self.deadline.get()))?
            .recv_timeout(
                self.deadline
                    .get()
                    .saturating_duration_since(Instant::now()),
            )
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?
    }

    fn clear_draft(&self) -> io::Result<()> {
        loop {
            for key in &self.clear {
                if self.clear_step(key.clone())? {
                    return Ok(());
                }
            }
        }
    }

    fn clear_step(&self, key: Bytes) -> io::Result<bool> {
        self.check_deadline()?;
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        match self.observe() {
            InputObservation::Empty => return Ok(true),
            InputObservation::Pending | InputObservation::Other => {}
            _ => {
                return Err(io::Error::other(
                    "Provider composer is not available; prompt was not typed",
                ))
            }
        }
        let completion = self.io.queue_user_input_submission(
            key,
            Bytes::new(),
            Duration::ZERO,
            Some(self.deadline.get()),
        )?;
        drop(guard);
        completion
            .recv_timeout(
                self.deadline
                    .get()
                    .saturating_duration_since(Instant::now()),
            )
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
        // Keep editing keys in separate input events for Ink providers. Check
        // for emptiness again before sending the next key or another line kill.
        std::thread::sleep(Duration::from_millis(25));
        Ok(false)
    }

    /// No Enter was sent. A draft the observer cannot attribute to Bus is our
    /// unrendered paste or someone's edit; neither may be submitted, and our
    /// paste must not linger for a later delivery or a person to send. Clear
    /// it and report a definite non-delivery only once the box is empty.
    fn withdraw_unshown(&self, timeout: io::Error) -> io::Result<()> {
        if self.observe() != InputObservation::Other {
            return Err(timeout);
        }
        self.deadline.set(Instant::now() + WITHDRAW_GRACE);
        self.clear_draft()?;
        Err(io::Error::other(PromptNotShown))
    }

    fn run(&self, text: Bytes, delay: Duration) -> io::Result<()> {
        self.clear_draft()?;
        self.write(text, delay)?;
        // Paste may be buffered by the provider after the PTY flush. Wait for
        // the owned draft to render before attempting the first Enter.
        while self.observe() != InputObservation::Pending {
            if let Err(error) = self.check_deadline() {
                return self.withdraw_unshown(error);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let mut last_enter = None;
        loop {
            self.check_deadline()?;
            let guard = self
                .content_lock
                .lock()
                .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
            let observation = self.observe();
            if last_enter.is_some()
                && matches!(
                    observation,
                    InputObservation::Empty | InputObservation::Accepted
                )
            {
                return Ok(());
            }
            let retry = observation == InputObservation::Pending
                && last_enter.is_none_or(|at: Instant| at.elapsed() >= Duration::from_millis(250));
            let completion = if retry {
                // Inspect and enqueue under the same lock as PTY rendering.
                // No paste retry: a lost response must never duplicate text.
                Some(self.io.queue_user_input_submission(
                    Bytes::new(),
                    self.enter.clone(),
                    Duration::ZERO,
                    Some(self.deadline.get()),
                )?)
            } else {
                None
            };
            drop(guard);
            if let Some(completion) = completion {
                completion
                    .recv_timeout(
                        self.deadline
                            .get()
                            .saturating_duration_since(Instant::now()),
                    )
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
                last_enter = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
