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

struct ObservedSubmission {
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    content_lock: Arc<Mutex<()>>,
    inspect: Box<dyn Fn(&str) -> InputObservation + Send>,
    enter: Bytes,
    clear: Vec<Bytes>,
    deadline: Instant,
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
            deadline: deadline.unwrap_or_else(|| Instant::now() + delay + Duration::from_secs(2)),
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
        if Instant::now() >= self.deadline {
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
            .queue_user_input_submission(bytes, Bytes::new(), delay, Some(self.deadline))?
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
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
            Some(self.deadline),
        )?;
        drop(guard);
        completion
            .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
        // Keep editing keys in separate input events for Ink providers. Check
        // for emptiness again before sending the next key or another line kill.
        std::thread::sleep(Duration::from_millis(25));
        Ok(false)
    }

    fn run(&self, text: Bytes, delay: Duration) -> io::Result<()> {
        self.clear_draft()?;
        self.write(text, delay)?;
        // Paste may be buffered by the provider after the PTY flush. Wait for
        // the owned draft to render before attempting the first Enter.
        while self.observe() != InputObservation::Pending {
            self.check_deadline()?;
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
                    Some(self.deadline),
                )?)
            } else {
                None
            };
            drop(guard);
            if let Some(completion) = completion {
                completion
                    .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
                last_enter = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
