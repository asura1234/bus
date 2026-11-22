//! Confirm Claude consumed the paste; retry only an unchanged, owned plain draft.
use super::{ClearOutcome, InputObservation, ObservedSubmission, PromptNotShown};
use crate::terminal::runtime::draft_input::DraftInputLease;
use bytes::Bytes;
use std::{
    io,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

impl ObservedSubmission {
    pub(super) fn await_submit_key_or_withdraw(
        &self,
        lease: Option<&DraftInputLease>,
    ) -> io::Result<()> {
        match self.await_submit_key() {
            Err(error) if self.inspect.waits_for_idle() && !self.diagnostics.enter_attempted() => {
                self.withdraw_before_enter(error, lease)
            }
            result => result,
        }
    }

    pub(super) fn confirm_claude_submission(
        &self,
        lease: Option<&DraftInputLease>,
        entered_at: Instant,
    ) -> io::Result<()> {
        let mut retried = false;
        loop {
            let generation = self.clock.generation();
            let guard = self
                .content_lock
                .lock()
                .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
            let screen = self.terminal.active_ansi_if_complete();
            let fresh = self.content_seq.load(Ordering::Acquire) != self.enter_content_seq.get();
            let queued = screen
                .as_ref()
                .is_some_and(|screen| self.inspect.native_queue_visible(screen));
            if fresh
                && !queued
                && screen
                    .as_ref()
                    .is_some_and(|screen| self.inspect.native_draft_restored(screen))
            {
                return Ok(());
            }
            let observation = self.observe("plain_consumption");
            if fresh && !queued && observation == InputObservation::Empty {
                return Ok(());
            }
            // Some turns hide the composer. Require a new transcript echo
            // of this prompt, since Working alone can be unrelated activity.
            if fresh
                && !queued
                && observation == InputObservation::Accepted
                && !self.inspect.native_draft_preserved()
                && screen
                    .as_ref()
                    .is_some_and(|screen| self.inspect.submitted_prompt_visible(screen))
            {
                return Ok(());
            }
            if let Err(error) = self.check_deadline() {
                drop(guard);
                if observation == InputObservation::Pending || queued {
                    self.diagnose_refusal("post_enter_retained");
                }
                if fresh && !queued && observation == InputObservation::Pending {
                    if let Some(lease) = lease.filter(|lease| !lease.edited()) {
                        self.deadline
                            .set((self.clock.now() + self.withdraw_grace).min(self.hard_deadline));
                        let cleared = self.clear_draft_with_lease(Some(lease));
                        self.diagnostics.cleared(&cleared, self.clock.now());
                        if matches!(cleared, Ok(ClearOutcome::Cleared)) {
                            tracing::info!(
                                event = "bus.terminal.submission.deferred",
                                reason = "queued_in_target_input",
                                "Withdrew the unsent Claude draft before closing the request"
                            );
                            lease.restored();
                            return Err(io::Error::other(PromptNotShown::QueuedInputWithdrawn));
                        }
                    }
                }
                return Err(Self::uncertain_submission(error));
            }
            // Claude restores a stash outside the input lease; a held repaint
            // must never authorize Enter against that developer draft.
            let retry = self.inspect.retry_owned_enter()
                && !retried
                && fresh
                && !self.inspect.native_draft_preserved()
                && observation == InputObservation::Pending
                && screen.as_ref().is_some_and(|screen| {
                    self.inspect.submission_available(screen) && !self.provider_queues_input(screen)
                })
                && self.clock.now().saturating_duration_since(entered_at)
                    >= Duration::from_millis(250);
            let completion = if retry {
                match lease.filter(|lease| !lease.edited()) {
                    Some(lease) => {
                        if let Some(screen) = &screen {
                            self.inspect.record_submit_frame(screen);
                        }
                        self.enter_content_seq
                            .set(self.content_seq.load(Ordering::Acquire));
                        self.diagnostics.enter(&self.enter, self.clock.now());
                        Some(lease.with_unedited(|| {
                            self.io.queue_input_write_scoped(
                                Bytes::new(),
                                self.enter.clone(),
                                Duration::ZERO,
                                Some(self.deadline.get()),
                                &self.scope,
                            )
                        }))
                    }
                    None => None,
                }
            } else {
                None
            };
            drop(guard);
            if let Some(completion) = completion {
                // No paste replay, and no retry of a lost Enter receipt.
                self.finish_enter(completion)?;
                retried = true;
                #[cfg(test)]
                self.clock.entered();
            }
            #[cfg(test)]
            self.clock.submission_checked(generation);
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }
}
