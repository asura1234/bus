//! Observe the composer between writes; retry only Enter for the owned draft.
use super::submission_clock::SubmissionClock;
use super::{io::PaneRuntimeIo, TerminalRuntime};
use crate::protocol::api::submission::{confirmation_budget, maximum_budget, WITHDRAW_GRACE};
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

#[cfg(test)]
#[path = "tests/submission_test.rs"]
mod tests;

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

struct ObservedSubmission {
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    content_lock: Arc<Mutex<()>>,
    inspect: Box<dyn Fn(&str) -> InputObservation + Send>,
    enter: Bytes,
    clear: Vec<Bytes>,
    deadline: std::cell::Cell<Instant>,
    hard_deadline: Instant,
    clock: SubmissionClock,
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
        let clock = self.io.submission_clock();
        let now = clock.now();
        let hard_deadline = deadline
            .unwrap_or(now + maximum_budget(delay))
            .min(now + maximum_budget(delay));
        let submission = ObservedSubmission {
            terminal: self.terminal.clone(),
            io: self.io.clone(),
            content_lock: self.content_write_lock.clone(),
            inspect: Box::new(inspect),
            enter,
            clear,
            // Queue/dispatch time must not consume the withdrawal reserve.
            deadline: std::cell::Cell::new(
                (now + confirmation_budget(delay)).min(hard_deadline - WITHDRAW_GRACE),
            ),
            hard_deadline,
            clock,
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
        if self.clock.now() >= self.deadline.get() {
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
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other(PromptNotShown))?;
        // Rendering and paste enqueue share this lock, just like clear keys
        // and Enter. A chooser can have replaced the empty composer since clear.
        if self.observe() != InputObservation::Empty {
            return Err(io::Error::other(PromptNotShown));
        }
        self.check_deadline()?;
        let completion = self.io.queue_user_input_submission(
            bytes,
            Bytes::new(),
            delay,
            Some(self.deadline.get()),
        )?;
        drop(guard);
        completion
            .recv_timeout(self.clock.remaining(self.deadline.get()))
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
        let generation = self.clock.generation();
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
        self.check_deadline()?;
        let completion = self.io.queue_user_input_submission(
            key,
            Bytes::new(),
            Duration::ZERO,
            Some(self.deadline.get()),
        )?;
        drop(guard);
        completion
            .recv_timeout(self.clock.remaining(self.deadline.get()))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
        // Keep editing keys in separate input events for Ink providers. Check
        // for emptiness again before sending the next key or another line kill.
        self.clock.sleep(Duration::from_millis(25), generation);
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
        self.deadline
            .set((self.clock.now() + WITHDRAW_GRACE).min(self.hard_deadline));
        self.clear_draft()?;
        Err(io::Error::other(PromptNotShown))
    }

    fn run(&self, text: Bytes, delay: Duration) -> io::Result<()> {
        // No paste or Enter has been queued yet. A dialog that appeared after
        // API validation is a definite non-delivery, safe to retain in FIFO.
        if matches!(
            self.observe(),
            InputObservation::Unavailable | InputObservation::Accepted
        ) {
            return Err(io::Error::other(PromptNotShown));
        }
        self.clear_draft()
            .map_err(|_| io::Error::other(PromptNotShown))?;
        self.write(text, delay)?;
        // Paste may be buffered by the provider after the PTY flush. Wait for
        // the owned draft to render before attempting the first Enter.
        loop {
            let generation = self.clock.generation();
            if self.observe() == InputObservation::Pending {
                break;
            }
            if let Err(error) = self.check_deadline() {
                return self.withdraw_unshown(error);
            }
            self.clock.sleep(Duration::from_millis(20), generation);
        }
        let mut last_enter = None;
        loop {
            let generation = self.clock.generation();
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
            // An observation can cross the deadline while waiting for the
            // content lock. Confirm acceptance first; only further input expires.
            self.check_deadline()?;
            let retry = observation == InputObservation::Pending
                && last_enter.is_none_or(|at: Instant| {
                    self.clock.now().saturating_duration_since(at) >= Duration::from_millis(250)
                });
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
                    .recv_timeout(self.clock.remaining(self.deadline.get()))
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
                last_enter = Some(self.clock.now());
            }
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }
}
