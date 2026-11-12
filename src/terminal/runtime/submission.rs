//! Save the native draft, submit guarded owned input, then restore it.
mod consumption;
mod diagnostics;
mod plain;
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
        atomic::{AtomicBool, AtomicU64, Ordering},
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
pub(crate) mod tests;

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
    /// Input availability is independent of the contents of Bus's paste.
    fn submission_available(&self, screen: &str) -> bool {
        self.observe(screen) != InputObservation::Unavailable
            && !crate::terminal::driver::has_input_controls(screen)
    }
    /// Some providers register pasted attachments asynchronously and swallow
    /// Enter until registration finishes. This is independent of pane focus
    /// and of the developer draft already saved by the input lease.
    fn paste_ready(&self, _screen: &str) -> bool {
        true
    }
    /// Claude plain pastes need owned render and consumption evidence; a
    /// still-owned draft may receive one extra Enter after a dropped key.
    fn retry_owned_enter(&self) -> bool {
        false
    }
    /// Native restoration can replace the composer without passing through Bus input.
    fn native_draft_preserved(&self) -> bool {
        false
    }
    fn draft_ready(&self, screen: &str, cursor: Option<(u16, u16)>, pristine: bool) -> bool {
        self.observe_with_launch(screen, cursor, pristine) == InputObservation::Empty
    }
    fn always_use_editor(&self) -> bool {
        false
    }
    fn confirm_consumption(&self) -> bool {
        false
    }
    /// Read-only eligibility; withdrawal still checks a complete owned frame.
    fn recoverable_input(&self, _screen: &str, _cursor: Option<(u16, u16)>) -> bool {
        false
    }
    /// Only a recognized picker over an already attested owned composer.
    fn owned_popup_escape(&self, _screen: &str) -> Option<Bytes> {
        None
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
        false
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
    /// Select an already encoded key from the same guarded frame that
    /// authorizes submission; provider activity can change during editing.
    fn editor_submit_key(&self, _screen: &str, default: &Bytes) -> Bytes {
        default.clone()
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
    content_seq: Arc<AtomicU64>,
    enter_content_seq: std::cell::Cell<u64>,
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

/// Owned pane references; preparing a submission does not acquire a scope or draft.
#[derive(Clone)]
pub(crate) struct SubmissionHandle {
    terminal: Arc<PaneTerminal>,
    content_seq: Arc<AtomicU64>,
    io: PaneRuntimeIo,
    content_write_lock: Arc<Mutex<()>>,
    draft_input: super::draft_input::SharedDraftInput,
    startup_suggestion_open: Arc<AtomicBool>,
}

impl TerminalRuntime {
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

    pub(crate) fn submission_handle(&self, raw: bool) -> SubmissionHandle {
        if raw {
            self.invalidate_shell_prompt();
        }
        SubmissionHandle {
            terminal: self.terminal.clone(),
            content_seq: self.content_seq.clone(),
            io: self.io.clone(),
            content_write_lock: self.content_write_lock.clone(),
            draft_input: self.draft_input.clone(),
            startup_suggestion_open: self.startup_suggestion_open.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn queue_observed_input_with_inspector(
        &self,
        text: Bytes,
        enter: Bytes,
        clear: Vec<Bytes>,
        delay: Duration,
        deadline: Option<Instant>,
        inspect: Box<dyn SubmissionInspector>,
    ) -> io::Result<InputCompletion> {
        self.submission_handle(false)
            .queue_observed_input_with_inspector(text, enter, clear, delay, deadline, inspect)
    }
}

impl SubmissionHandle {
    pub(crate) fn try_send_developer_bytes(&self, bytes: Bytes) -> io::Result<()> {
        super::draft_input::DraftInput::send(&self.draft_input, &self.io, bytes)
            .map_err(io::Error::other)
    }

    pub(crate) fn queue_user_input_submission(
        &self,
        text: Bytes,
        enter: Bytes,
        delay: Duration,
        deadline: Option<Instant>,
    ) -> io::Result<InputCompletion> {
        super::draft_input::DraftInput::invalidate(&self.draft_input);
        self.io.queue_input_write(text, enter, delay, deadline)
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
        // Acceptance is the caller's sample from before this queue, and the
        // writer stamp is the sample from after it returns. Filling both here
        // collapses that interval before the caller can record it.
        let mut submission = ObservedSubmission {
            terminal: self.terminal.clone(),
            content_seq: self.content_seq.clone(),
            enter_content_seq: std::cell::Cell::new(0),
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
            diagnostics: diagnostics::Diagnostics::with_timestamps(scope.timestamps()),
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
                        // Composer consumption is not a native turn claim.
                        // Preserve Codex ownership through a transient empty
                        // repaint; developer input still revokes this record.
                        let pending = if result.is_ok() && submission.inspect.confirm_consumption()
                        {
                            submission.diagnostics.written_paste()
                        } else {
                            submission.diagnostics.pending_paste(&result)
                        };
                        if let Some((since, entered)) = pending {
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
        let Some(screen) = self.terminal.active_ansi_if_complete() else {
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
        let screen = self.terminal.active_ansi();
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
        let completion = self.io.queue_payload_input_write_scoped(
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

    fn clear_draft_with_lease(
        &self,
        lease: Option<&super::draft_input::DraftInputLease>,
    ) -> io::Result<ClearOutcome> {
        let mut sent = false;
        let mut progress = ClearProgress {
            revision: self.inspect.cleanup_progress(),
            expires: self.clock.now() + WITHDRAW_GRACE,
        };
        loop {
            for (index, key) in self.clear.iter().enumerate() {
                if let Some(outcome) =
                    self.clear_step(key.clone(), index, &mut sent, &mut progress, lease)?
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
        lease: Option<&super::draft_input::DraftInputLease>,
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
        let queue = || {
            self.io.queue_input_write_scoped(
                key,
                Bytes::new(),
                Duration::ZERO,
                Some(write_deadline),
                &self.scope,
            )
        };
        let completion = match lease {
            Some(lease) if self.inspect.confirm_consumption() => lease.with_unedited(queue),
            Some(lease) => lease.with_uninvalidated(queue),
            None => queue(),
        }?;
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
    fn withdraw_before_enter(
        &self,
        timeout: io::Error,
        lease: Option<&super::draft_input::DraftInputLease>,
    ) -> io::Result<()> {
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
        let cleared = self.clear_draft_with_lease(lease);
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

    fn finish_enter(&self, completion: io::Result<InputCompletion>) -> io::Result<()> {
        let result = completion.and_then(|completion| {
            completion
                .recv_timeout(self.clock.remaining(self.deadline.get()))
                .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
                .and_then(|result| result)
        });
        self.diagnostics.entered(&result, self.clock.now());
        result.map_err(Self::uncertain_submission)
    }

    fn uncertain_submission(error: io::Error) -> io::Error {
        io::Error::new(error.kind(), format!("{error}. The message may have run; inspect the agent's transcript and composer before resending"))
    }

    /// One Enter after a submittable frame. An incomplete or blank redraw is
    /// waited out; a complete frame that still cannot take input is refused.
    fn await_submit_key(&self) -> io::Result<()> {
        let blocked = || {
            Self::before_enter_failure(
                io::Error::other("Native input is blocked"),
                "Saved input is retained; no submission key was sent",
            )
        };
        loop {
            let generation = self.clock.generation();
            let queued = {
                let _guard = self
                    .content_lock
                    .lock()
                    .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
                match self.terminal.active_ansi_if_complete() {
                    Some(screen) if self.inspect.submission_available(&screen) => {
                        if !self.inspect.paste_ready(&screen) {
                            self.observe("paste_processing");
                            None
                        } else {
                            self.check_deadline()?;
                            let key = self.inspect.editor_submit_key(&screen, &self.enter);
                            if self.inspect.confirm_consumption()
                                || self.inspect.retry_owned_enter()
                            {
                                self.observe("enter_ownership");
                            }
                            self.enter_content_seq
                                .set(self.content_seq.load(Ordering::Acquire));
                            self.diagnostics.enter(&key, self.clock.now());
                            Some(self.io.queue_input_write_scoped(
                                Bytes::new(),
                                key,
                                Duration::ZERO,
                                Some(self.deadline.get()),
                                &self.scope,
                            ))
                        }
                    }
                    Some(screen) if screen.trim().is_empty() => None,
                    Some(_) => return Err(blocked()),
                    None => None,
                }
            };
            if let Some(queued) = queued {
                // The content lock is released before the write is awaited.
                return self.finish_enter(queued);
            }
            if let Err(error) = self.check_deadline() {
                return Err(Self::before_enter_failure(
                    error,
                    "Saved input is retained; no submission key was sent",
                ));
            }
            #[cfg(test)]
            self.clock.waiting_for_draft();
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }

    fn run(
        &self,
        text: Bytes,
        delay: Duration,
        lease: Option<&super::draft_input::DraftInputLease>,
    ) -> io::Result<()> {
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
                self.withdraw_before_enter(error, lease)
            };
        }
        // The pane lease buffers developer input. Claude waits for its owned
        // plain draft or attachment registration before the first Enter;
        // other providers keep their existing availability guard.
        self.await_submit_key()?;
        let entered_at = self.clock.now();
        #[cfg(test)]
        self.clock.entered();
        if self.inspect.retry_owned_enter() {
            return self.confirm_plain_submission(lease, entered_at);
        }
        // Native Started/Steered callbacks settle delivery asynchronously.
        // An unchanged empty frame here is not proof of provider acceptance.
        Ok(())
    }

    pub(super) fn wait_for_consumption(&self) -> io::Result<()> {
        loop {
            let generation = self.clock.generation();
            let guard = self
                .content_lock
                .lock()
                .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
            // Claude restores its native stash synchronously at submit. That
            // is acceptance evidence even though the composer is nonempty.
            if self
                .inspect
                .native_draft_restored(&self.terminal.active_ansi())
            {
                return Ok(());
            }
            let observation = self.observe("confirm");
            if matches!(
                observation,
                InputObservation::Empty | InputObservation::Accepted
            ) {
                return Ok(());
            }
            // An observation can cross the deadline while waiting for the
            // content lock. Confirm acceptance first; only further input expires.
            if let Err(error) = self.check_deadline() {
                drop(guard);
                return Err(error);
            }
            drop(guard);
            #[cfg(test)]
            self.clock.submission_checked(generation);
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }
}
