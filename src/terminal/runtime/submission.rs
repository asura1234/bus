//! Observe the composer between writes; retry only Enter for the owned draft.
mod diagnostics;

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

/// No Enter was sent for this delivery. Input was refused before pasting or
/// the paste was withdrawn, so this is a definite non-delivery, safe to retry.
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
    diagnostics: diagnostics::Diagnostics,
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
            diagnostics: diagnostics::Diagnostics::default(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let span = tracing::Span::current();
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let _entered = span.enter();
                submission.diagnostics.begin(
                    submission.clock.now(),
                    submission.deadline.get(),
                    submission.hard_deadline,
                );
                let result = submission.run(text, delay);
                submission
                    .diagnostics
                    .finish(&result, submission.clock.now());
                let _ = tx.send(result);
            })
        });
        Ok(rx)
    }
}

impl ObservedSubmission {
    fn observe(&self, phase: &'static str) -> InputObservation {
        let screen = self.terminal.visible_ansi();
        let observation = (self.inspect)(&screen);
        self.diagnostics
            .observe(phase, observation, screen.len(), self.clock.now());
        observation
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
        if self.observe("paste_preflight") != InputObservation::Empty {
            return Err(io::Error::other(PromptNotShown));
        }
        self.check_deadline()?;
        self.diagnostics
            .paste(bytes.len(), delay.as_millis() as u64, self.clock.now());
        let completion = self.io.queue_user_input_submission(
            bytes,
            Bytes::new(),
            delay,
            Some(self.deadline.get()),
        )?;
        drop(guard);
        let result = completion
            .recv_timeout(self.clock.remaining(self.deadline.get()))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
            .and_then(|result| result);
        self.diagnostics.pasted(&result, self.clock.now());
        result
    }

    fn clear_draft(&self, owned_only: bool) -> io::Result<()> {
        loop {
            for key in &self.clear {
                if self.clear_step(key.clone(), owned_only)? {
                    return Ok(());
                }
            }
        }
    }

    fn clear_step(&self, key: Bytes, owned_only: bool) -> io::Result<bool> {
        let generation = self.clock.generation();
        self.check_deadline()?;
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        match self.observe("clear_preflight") {
            InputObservation::Empty => return Ok(true),
            InputObservation::Pending => {}
            InputObservation::Other if !owned_only => {}
            _ => {
                return Err(io::Error::other(
                    "Provider composer changed or is unavailable; draft was not cleared",
                ))
            }
        }
        self.check_deadline()?;
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.clear", key = ?key, owned_only);
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

    /// No Enter was sent. Withdraw the paste, including a draft first seen
    /// across the confirmation deadline. Preserve ownership checks for a
    /// recognized draft; report definite non-delivery only once it is empty.
    fn withdraw_before_enter(&self, timeout: io::Error, owned_seen: bool) -> io::Result<()> {
        let owned_only = match self.observe("withdraw_before_enter") {
            InputObservation::Pending => true,
            InputObservation::Other if !owned_seen => false,
            _ => {
                self.diagnostics.cleanup_refused(self.clock.now());
                return Err(timeout);
            }
        };
        self.deadline
            .set((self.clock.now() + WITHDRAW_GRACE).min(self.hard_deadline));
        let cleared = self.clear_draft(owned_only);
        self.diagnostics.cleared(&cleared, self.clock.now());
        cleared?;
        Err(io::Error::other(PromptNotShown))
    }

    /// Enter was flushed but acceptance is still unknown. Withdraw only the
    /// unchanged owned draft, rechecking ownership under the render lock before
    /// every editing key. Clearing cannot prove the provider never ran it.
    fn withdraw_pending(&self, timeout: io::Error) -> io::Result<()> {
        self.deadline
            .set((self.clock.now() + WITHDRAW_GRACE).min(self.hard_deadline));
        let cleared = self.clear_draft(true);
        self.diagnostics.cleared(&cleared, self.clock.now());
        let detail = match cleared {
            Ok(()) => "The remaining draft was cleared".to_owned(),
            Err(error) => format!("The draft could not be safely cleared: {error}"),
        };
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{timeout}. {detail}. The message may have run; inspect the agent's transcript and composer before resending"),
        ))
    }

    fn run(&self, text: Bytes, delay: Duration) -> io::Result<()> {
        // No paste or Enter has been queued yet. A dialog that appeared after
        // API validation is a definite non-delivery, safe to retain in FIFO.
        if matches!(
            self.observe("initial"),
            InputObservation::Unavailable | InputObservation::Accepted
        ) {
            return Err(io::Error::other(PromptNotShown));
        }
        self.clear_draft(false)
            .map_err(|_| io::Error::other(PromptNotShown))?;
        self.write(text, delay)?;
        // Paste may be buffered by the provider after the PTY flush. Wait for
        // the owned draft to render before attempting the first Enter.
        loop {
            let generation = self.clock.generation();
            if self.observe("await_paste") == InputObservation::Pending {
                break;
            }
            if let Err(error) = self.check_deadline() {
                return self.withdraw_before_enter(error, false);
            }
            #[cfg(test)]
            self.clock.waiting_for_draft();
            self.clock.sleep(Duration::from_millis(20), generation);
        }
        let mut last_enter = None;
        loop {
            let generation = self.clock.generation();
            let guard = self
                .content_lock
                .lock()
                .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
            let observation = self.observe("confirm");
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
            if let Err(error) = self.check_deadline() {
                drop(guard);
                return if last_enter.is_some() {
                    self.withdraw_pending(error)
                } else {
                    self.withdraw_before_enter(error, true)
                };
            }
            let retry = observation == InputObservation::Pending
                && last_enter.is_none_or(|at: Instant| {
                    self.clock.now().saturating_duration_since(at) >= Duration::from_millis(250)
                });
            let completion = if retry {
                // Inspect and enqueue under the same lock as PTY rendering.
                // No paste retry: a lost response must never duplicate text.
                self.diagnostics.enter(&self.enter, self.clock.now());
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
                let result = completion
                    .recv_timeout(self.clock.remaining(self.deadline.get()))
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
                    .and_then(|result| result);
                self.diagnostics.entered(&result, self.clock.now());
                result?;
                last_enter = Some(self.clock.now());
                #[cfg(test)]
                self.clock.entered();
            }
            #[cfg(test)]
            self.clock.submission_checked(generation);
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }
}
