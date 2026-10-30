//! Observe the composer between writes; retry only Enter for the owned draft.
mod diagnostics;
mod preservation;
mod recovery;
pub(crate) use preservation::{DraftPreservation, EditorSubmission};

use super::submission_clock::SubmissionClock;
use super::{io::PaneRuntimeIo, TerminalRuntime};
use crate::protocol::api::submission::{
    confirmation_budget, maximum_budget, withdrawal_budget, CLEAR_KEY_DELAY, WITHDRAW_GRACE,
};
use crate::terminal::emulator::PaneTerminal;
use crate::terminal::pty::actor::{InputCompletion, InputScope};
use bytes::Bytes;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
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
pub(crate) enum PromptNotShown {
    NotSubmitted,
    InputOccupied,
    ComposerBusy,
}

impl std::fmt::Display for PromptNotShown {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if matches!(self, Self::ComposerBusy) {
            return formatter
                .write_str("Waiting for the agent's composer; existing input is preserved");
        }
        formatter.write_str(crate::protocol::api::AGENT_PROMPT_NOT_SHOWN_MESSAGE)?;
        if matches!(self, Self::InputOccupied) {
            formatter.write_str(". Existing input was preserved; inspect the agent's composer and resolve its draft before retrying")?;
        }
        Ok(())
    }
}

impl std::error::Error for PromptNotShown {}

impl PromptNotShown {
    pub(crate) fn composer_busy(error: &io::Error) -> bool {
        error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<Self>())
            .is_some_and(|error| matches!(error, Self::ComposerBusy))
    }
    pub(crate) fn is(error: &io::Error) -> bool {
        error
            .get_ref()
            .is_some_and(|inner| inner.downcast_ref::<Self>().is_some())
    }
}

/// Provider-specific ownership transitions after Bus's own editing keys.
pub(crate) trait SubmissionInspector: Send {
    fn observe(&self, screen: &str) -> InputObservation;
    fn observe_with_cursor(&self, screen: &str, cursor: Option<(u16, u16)>) -> InputObservation {
        let _ = cursor;
        self.observe(screen)
    }
    fn observe_with_launch(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        _pristine: bool,
    ) -> InputObservation {
        self.observe_with_cursor(screen, cursor)
    }
    /// Called once when this attempt is refused, outside the render lock.
    fn diagnose_refusal(&self, _screen: &str, _phase: &'static str, _cursor: Option<(u16, u16)>) {}
    /// Indices identify the Ctrl+A, Ctrl+K, Ctrl+U sequence supplied by the API.
    /// Called only after its key was queued under the content/render lock.
    fn clearing(&self, _key_index: usize) {}
    /// Advances only when a new owned remainder is actually observed.
    fn cleanup_progress(&self) -> u64 {
        0
    }
    fn draft_preservation(&self, _screen: &str) -> Option<DraftPreservation> {
        None
    }
    fn draft_preservation_with_cursor(
        &self,
        screen: &str,
        _cursor: Option<(u16, u16)>,
    ) -> Option<DraftPreservation> {
        self.draft_preservation(screen)
    }
    fn draft_preservation_since_launch(
        &self,
        screen: &str,
        cursor: Option<(u16, u16)>,
        _pristine: bool,
    ) -> Option<DraftPreservation> {
        self.draft_preservation_with_cursor(screen, cursor)
    }
    fn native_stash_visible(&self, _screen: &str) -> bool {
        true
    }
    fn native_draft_restored(&self, _screen: &str) -> bool {
        false
    }
    fn restoration_marker_visible(&self, _screen: &str, _marker: &str) -> bool {
        false
    }
    fn preservation_budget(&self) -> Duration {
        Duration::ZERO
    }
    fn editor_submission(&self) -> Option<EditorSubmission> {
        None
    }
    fn editor_frame_readable(&self, _screen: &str) -> bool {
        false
    }
    fn editor_working(&self, _screen: &str) -> bool {
        false
    }
}

impl<F: Fn(&str) -> InputObservation + Send> SubmissionInspector for F {
    fn observe(&self, screen: &str) -> InputObservation {
        self(screen)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClearOutcome {
    AlreadyEmpty,
    Cleared,
    Accepted,
}

struct ClearProgress {
    revision: u64,
    expires: Instant,
}

struct ObservedSubmission {
    terminal: Arc<PaneTerminal>,
    io: PaneRuntimeIo,
    scope: InputScope,
    content_lock: Arc<Mutex<()>>,
    inspect: Box<dyn SubmissionInspector>,
    enter: Bytes,
    clear: Vec<Bytes>,
    deadline: std::cell::Cell<Instant>,
    hard_deadline: Instant,
    withdraw_grace: Duration,
    clock: SubmissionClock,
    diagnostics: diagnostics::Diagnostics,
    startup_suggestion_open: Arc<AtomicBool>,
}

impl TerminalRuntime {
    /// Completion means the owned composer cleared after Enter, or a turn
    /// visibly began. A PTY write alone cannot confirm provider acceptance.
    #[cfg(test)]
    pub(crate) fn queue_observed_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        clear: Vec<Bytes>,
        delay: Duration,
        deadline: Option<Instant>,
        inspect: impl Fn(&str) -> InputObservation + Send + 'static,
    ) -> io::Result<InputCompletion> {
        self.queue_observed_input_with_inspector(
            text,
            enter,
            clear,
            delay,
            deadline,
            Box::new(inspect),
        )
    }

    pub(crate) fn queue_observed_input_with_inspector(
        &self,
        text: Bytes,
        enter: Bytes,
        clear: Vec<Bytes>,
        delay: Duration,
        deadline: Option<Instant>,
        inspect: Box<dyn SubmissionInspector>,
    ) -> io::Result<InputCompletion> {
        let clock = self.io.submission_clock();
        let now = clock.now();
        let withdraw_grace = withdrawal_budget(&text);
        let maximum = maximum_budget(delay, &text) + inspect.preservation_budget();
        let hard_deadline = deadline.unwrap_or(now + maximum).min(now + maximum);
        let scope = self.io.input_scope();
        let mut submission = ObservedSubmission {
            terminal: self.terminal.clone(),
            io: self.io.clone(),
            scope: scope.clone(),
            content_lock: self.content_write_lock.clone(),
            inspect,
            enter,
            clear,
            // Queue/dispatch time must not consume the withdrawal reserve.
            deadline: std::cell::Cell::new(
                (now + confirmation_budget(delay)).min(hard_deadline - withdraw_grace),
            ),
            hard_deadline,
            withdraw_grace,
            clock,
            diagnostics: diagnostics::Diagnostics::default(),
            startup_suggestion_open: self.startup_suggestion_open.clone(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let span = tracing::Span::current();
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        let draft_input = self.draft_input.clone();
        std::thread::spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let _entered = span.enter();
                submission.diagnostics.begin(
                    submission.clock.now(),
                    submission.deadline.get(),
                    submission.hard_deadline,
                );
                let result = super::draft_input::DraftInput::acquire(&draft_input, &submission.io)
                    .and_then(|lease| {
                        let result = submission
                            .recover_pending_paste(delay, &lease)
                            .and_then(|()| submission.run_preserving_draft(text, delay, &lease));
                        // Inspect before releasing/replaying queued developer input.
                        if result.is_err() {
                            submission.diagnose_refusal("result_error");
                        }
                        if let Some((since, entered)) =
                            submission.diagnostics.pending_paste(&result)
                        {
                            lease.retain_pending(super::draft_input::PendingPaste {
                                inspect: std::mem::replace(
                                    &mut submission.inspect,
                                    Box::new(|_: &str| InputObservation::Unavailable),
                                ),
                                clear: submission.clear.clone(),
                                since,
                                entered,
                                withdraw_grace: submission.withdraw_grace,
                            });
                        }
                        result
                    });
                if result.is_err() {
                    submission.diagnose_refusal("result_error");
                }
                submission
                    .diagnostics
                    .finish(&result, submission.clock.now());
                let _ = tx.send(result);
            })
        });
        Ok(scope.completion(rx))
    }
}

impl ObservedSubmission {
    fn observe(&self, phase: &'static str) -> InputObservation {
        self.observe_frame(phase)
            .unwrap_or(InputObservation::Unavailable)
    }

    fn observe_frame(&self, phase: &'static str) -> Option<InputObservation> {
        let Some(screen) = self.terminal.visible_ansi_if_complete() else {
            self.diagnostics
                .observe(phase, InputObservation::Unavailable, 0, self.clock.now());
            return None;
        };
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        let pristine = self.startup_suggestion_open.load(Ordering::Acquire);
        let observation = self.inspect.observe_with_launch(&screen, cursor, pristine);
        self.diagnostics
            .observe(phase, observation, screen.len(), self.clock.now());
        Some(observation)
    }

    fn observe_initial(&self, phase: &'static str) -> io::Result<InputObservation> {
        let mut waited = false;
        loop {
            let generation = self.clock.generation();
            if let Some(observation) = self
                .observe_frame(phase)
                .filter(|_| !waited || self.check_deadline().is_ok())
            {
                return Ok(observation);
            }
            if self.check_deadline().is_err() {
                // No paste was sent. A slow/incomplete repaint is a transient
                // wait, not prompt_not_shown's rejected-message expiry clock.
                return Err(io::Error::other(PromptNotShown::ComposerBusy));
            }
            waited = true;
            #[cfg(test)]
            self.clock.waiting_for_draft();
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }

    fn diagnose_refusal(&self, phase: &'static str) {
        let screen = self.terminal.visible_ansi();
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        self.diagnose_refusal_snapshot(&screen, phase, cursor);
    }

    fn diagnose_refusal_snapshot(
        &self,
        screen: &str,
        phase: &'static str,
        cursor: Option<(u16, u16)>,
    ) {
        if self.diagnostics.claim_refusal() {
            self.inspect.diagnose_refusal(screen, phase, cursor);
        }
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
        self.check_deadline()
            .map_err(|_| io::Error::other(PromptNotShown::NotSubmitted))?;
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other(PromptNotShown::NotSubmitted))?;
        // Rendering and paste enqueue share this lock, just like clear keys
        // and Enter. A chooser can have replaced the initially empty composer.
        if self.observe("paste_preflight") != InputObservation::Empty {
            return Err(io::Error::other(PromptNotShown::NotSubmitted));
        }
        self.check_deadline()
            .map_err(|_| io::Error::other(PromptNotShown::NotSubmitted))?;
        self.diagnostics
            .paste(bytes.len(), delay.as_millis() as u64, self.clock.now());
        let completion = self.io.queue_input_write_scoped(
            bytes,
            Bytes::new(),
            delay,
            Some(self.deadline.get()),
            &self.scope,
        )?;
        drop(guard);
        let result = completion
            .recv_timeout(self.clock.remaining(self.deadline.get()))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
            .and_then(|result| result);
        if result.is_ok() {
            // The placeholder is no longer the only thing Bus might overwrite.
            // A failed paste leaves the flag open so the next attempt can
            // still deliver into an untouched composer.
            self.startup_suggestion_open.store(false, Ordering::Release);
        }
        self.diagnostics.pasted(&result, self.clock.now());
        result
    }

    fn clear_draft(&self) -> io::Result<ClearOutcome> {
        let mut sent = false;
        let mut progress = ClearProgress {
            revision: self.inspect.cleanup_progress(),
            expires: self.clock.now() + WITHDRAW_GRACE,
        };
        loop {
            for (index, key) in self.clear.iter().enumerate() {
                if let Some(outcome) =
                    self.clear_step(key.clone(), index, &mut sent, &mut progress)?
                {
                    return Ok(outcome);
                }
            }
        }
    }

    fn clear_step(
        &self,
        key: Bytes,
        index: usize,
        sent: &mut bool,
        progress: &mut ClearProgress,
    ) -> io::Result<Option<ClearOutcome>> {
        let generation = self.clock.generation();
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        match self.observe("clear_preflight") {
            InputObservation::Empty => {
                return Ok(Some(if *sent {
                    ClearOutcome::Cleared
                } else {
                    ClearOutcome::AlreadyEmpty
                }))
            }
            InputObservation::Accepted => return Ok(Some(ClearOutcome::Accepted)),
            InputObservation::Pending => {}
            _ => {
                return Err(io::Error::other(
                    "Provider composer changed or is unavailable; draft was not cleared",
                ))
            }
        }
        let revision = self.inspect.cleanup_progress();
        if progress.revision != revision {
            progress.revision = revision;
            progress.expires = self.clock.now() + WITHDRAW_GRACE;
        }
        if self.clock.now() >= progress.expires {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Provider made no owned-draft withdrawal progress",
            ));
        }
        self.check_deadline()?;
        let write_deadline = self.deadline.get().min(progress.expires);
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.clear", key = ?key, owned_only = true);
        let completion = self.io.queue_input_write_scoped(
            key,
            Bytes::new(),
            Duration::ZERO,
            Some(write_deadline),
            &self.scope,
        )?;
        self.inspect.clearing(index);
        drop(guard);
        completion
            .recv_timeout(self.clock.remaining(write_deadline))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
        // Keep editing keys in separate input events for Ink providers. Check
        // for emptiness again before sending the next key or another line kill.
        self.clock.sleep(CLEAR_KEY_DELAY, generation);
        *sent = true;
        Ok(None)
    }

    /// No Enter was sent. Withdraw the paste, including a draft first seen
    /// across the confirmation deadline. Preserve ownership checks for a
    /// recognized draft; report definite non-delivery only once it is empty.
    fn withdraw_before_enter(&self, timeout: io::Error) -> io::Result<()> {
        let observation = self.observe("withdraw_before_enter");
        self.diagnose_refusal("withdraw_before_enter");
        if observation != InputObservation::Pending {
            self.diagnostics.cleanup_refused(self.clock.now());
            let detail = match observation {
                InputObservation::Other => {
                    "The draft was left because it was edited or could not be attributed to Bus"
                }
                InputObservation::Empty => {
                    "No remaining draft was visible; the paste write may still be pending"
                }
                _ => "The draft was left because the composer could not be safely edited",
            };
            return Err(Self::before_enter_failure(timeout, detail));
        }
        self.deadline
            .set((self.clock.now() + self.withdraw_grace).min(self.hard_deadline));
        let cleared = self.clear_draft();
        self.diagnostics.cleared(&cleared, self.clock.now());
        match cleared {
            Ok(ClearOutcome::AlreadyEmpty | ClearOutcome::Cleared) => {
                Err(io::Error::other(PromptNotShown::NotSubmitted))
            }
            Ok(ClearOutcome::Accepted) => Err(Self::before_enter_failure(
                timeout,
                "Provider work appeared before the draft could be withdrawn",
            )),
            Err(error) => Err(Self::before_enter_failure(
                timeout,
                &format!("The draft was left because cleanup could not finish safely: {error}"),
            )),
        }
    }

    fn before_enter_failure(error: io::Error, detail: &str) -> io::Error {
        io::Error::new(error.kind(), format!("{error}. {detail}. No Enter was sent by Bus. Inspect the agent's transcript and composer before resending; preserve your edits while removing any remaining Bus draft"))
    }

    /// Enter was attempted but acceptance is still unknown. Withdraw only the
    /// unchanged owned draft, rechecking ownership under the render lock before
    /// every editing key. Clearing cannot prove the provider never ran it.
    fn withdraw_pending(&self, timeout: io::Error) -> io::Result<()> {
        let screen = self.terminal.visible_ansi();
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        self.deadline
            .set((self.clock.now() + self.withdraw_grace).min(self.hard_deadline));
        let cleared = self.clear_draft();
        self.diagnostics.cleared(&cleared, self.clock.now());
        let detail = match cleared {
            Ok(ClearOutcome::AlreadyEmpty | ClearOutcome::Accepted) => return Ok(()),
            Ok(ClearOutcome::Cleared) => "The remaining draft was cleared".to_owned(),
            Err(error) => format!("The draft could not be safely cleared: {error}"),
        };
        self.diagnose_refusal_snapshot(&screen, "withdraw_pending", cursor);
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("{timeout}. {detail}. The message may have run; inspect the agent's transcript and composer before resending"),
        ))
    }

    // The caller holds content_lock while queueing, and releases it before waiting.
    fn enqueue_enter(&self) -> io::Result<InputCompletion> {
        self.diagnostics.enter(&self.enter, self.clock.now());
        self.io.queue_input_write_scoped(
            Bytes::new(),
            self.enter.clone(),
            Duration::ZERO,
            Some(self.deadline.get()),
            &self.scope,
        )
    }

    fn finish_enter(&self, completion: io::Result<InputCompletion>) -> io::Result<()> {
        let result = completion.and_then(|completion| {
            completion
                .recv_timeout(self.clock.remaining(self.deadline.get()))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
                .and_then(|result| result)
        });
        self.diagnostics.entered(&result, self.clock.now());
        result
    }

    fn run(&self, text: Bytes, delay: Duration) -> io::Result<()> {
        // Nothing from this attempt has been pasted. Preserve all existing
        // input, even text identical to the new brief: ownership is unproven.
        match self.observe_initial("initial")? {
            InputObservation::Empty => {}
            InputObservation::Pending | InputObservation::Other => {
                self.diagnose_refusal("initial_refusal");
                return Err(io::Error::other(PromptNotShown::InputOccupied));
            }
            _ => {
                self.diagnose_refusal("initial_refusal");
                return Err(io::Error::other(PromptNotShown::NotSubmitted));
            }
        }
        if let Err(error) = self.write(text, delay) {
            return if PromptNotShown::is(&error) {
                Err(error)
            } else {
                self.withdraw_before_enter(error)
            };
        }
        // Paste may be buffered by the provider after the PTY flush. Wait for
        // the owned draft to render before attempting the first Enter.
        loop {
            let generation = self.clock.generation();
            if self.observe("await_paste") == InputObservation::Pending {
                break;
            }
            if let Err(error) = self.check_deadline() {
                return self.withdraw_before_enter(error);
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
            // Claude restores its native stash synchronously at submit. That
            // is acceptance evidence even though the composer is nonempty.
            if last_enter.is_some()
                && self
                    .inspect
                    .native_draft_restored(&self.terminal.visible_ansi())
            {
                return Ok(());
            }
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
                    self.withdraw_before_enter(error)
                };
            }
            let retry = observation == InputObservation::Pending
                && last_enter.is_none_or(|at: Instant| {
                    self.clock.now().saturating_duration_since(at) >= Duration::from_millis(250)
                });
            let completion = if retry {
                // Inspect and enqueue under the same lock as PTY rendering.
                // No paste retry: a lost response must never duplicate text.
                Some(self.enqueue_enter())
            } else {
                None
            };
            drop(guard);
            if let Some(completion) = completion {
                if let Err(error) = self.finish_enter(completion) {
                    return self.withdraw_pending(error);
                }
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
