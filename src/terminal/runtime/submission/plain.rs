//! Confirm Claude's plain paste; retry only its unchanged, still-owned draft.
use super::{InputObservation, ObservedSubmission};
use crate::terminal::runtime::draft_input::DraftInputLease;
use bytes::Bytes;
use std::{
    io,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

impl ObservedSubmission {
    pub(super) fn confirm_plain_submission(
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
            if fresh
                && screen
                    .as_ref()
                    .is_some_and(|screen| self.inspect.native_draft_restored(screen))
            {
                return Ok(());
            }
            let observation = self.observe("plain_consumption");
            if fresh && observation == InputObservation::Empty {
                return Ok(());
            }
            // Working chrome can belong to an unrelated background turn;
            // only a cleared composer or restored native stash proves consumption.
            // A changed draft loses ownership in the inspector permanently.
            self.check_deadline().map_err(Self::uncertain_submission)?;
            // Claude restores a stash outside the input lease; a held repaint
            // must never authorize Enter against that developer draft.
            let retry = !retried
                && fresh
                && !self.inspect.native_draft_preserved()
                && observation == InputObservation::Pending
                && screen
                    .as_ref()
                    .is_some_and(|screen| self.inspect.submission_available(screen))
                && self.clock.now().saturating_duration_since(entered_at)
                    >= Duration::from_millis(250);
            let completion = if retry {
                match lease.filter(|lease| !lease.edited()) {
                    Some(lease) => {
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
