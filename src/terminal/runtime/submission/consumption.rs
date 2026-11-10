//! Confirm legacy Codex consumption, then retire only its still-owned input.
use super::{ClearOutcome, InputObservation, ObservedSubmission, PromptNotShown};
use crate::terminal::runtime::draft_input::DraftInputLease;
use std::{io, sync::atomic::Ordering, time::Duration};

impl ObservedSubmission {
    pub(super) fn confirm_legacy_submission(
        &self,
        result: io::Result<()>,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        if !self.inspect.confirm_consumption()
            || result.is_err()
                && (self.diagnostics.enter_attempted()
                    || self.diagnostics.pending_paste(&result).is_none())
        {
            return result;
        }
        let entered = self.diagnostics.enter_attempted();
        let error = match result.and_then(|()| self.wait_legacy_consumption()) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        self.diagnose_refusal("legacy_unconfirmed");
        self.deadline
            .set((self.clock.now() + self.withdraw_grace).min(self.hard_deadline));
        // Buffered developer input revokes the attempt's cleanup ownership.
        // Its recovery buffer and a changed visible draft must remain intact.
        let cleared = if lease.edited() {
            Err(io::Error::other(
                "Developer input revoked Bus draft ownership",
            ))
        } else {
            self.close_owned_popup(lease)
                .and_then(|()| self.clear_draft_with_lease(Some(lease)))
        };
        self.diagnostics.cleared(&cleared, self.clock.now());
        let detail = match cleared {
            Ok(ClearOutcome::AlreadyEmpty | ClearOutcome::Cleared) => {
                lease.restored();
                if !entered {
                    return Err(io::Error::other(PromptNotShown::NotSubmitted));
                }
                "Remaining Bus input was withdrawn"
            }
            Ok(ClearOutcome::Accepted) => "Provider work appeared; no further input was sent",
            Err(_) => "Changed or unavailable input was preserved",
        };
        // A prior Enter may already have run. Cleanup never authorizes a retry
        // or proves a model turn; the coordinator still requires its callback.
        Err(io::Error::new(
            error.kind(),
            format!("{error}. {detail}; submission remains unconfirmed; no Enter was replayed"),
        ))
    }

    fn wait_legacy_consumption(&self) -> io::Result<()> {
        loop {
            let generation = self.clock.generation();
            let guard = self
                .content_lock
                .lock()
                .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
            // A stale pre-Enter empty frame is not consumption. This is only
            // composer evidence: native Started/Steered still owns the turn.
            if self.content_seq.load(Ordering::Acquire) != self.enter_content_seq.get()
                && matches!(
                    self.observe("legacy_consumption"),
                    InputObservation::Empty | InputObservation::Accepted
                )
            {
                return Ok(());
            }
            self.check_deadline()?;
            drop(guard);
            #[cfg(test)]
            self.clock.submission_checked(generation);
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }

    pub(super) fn close_owned_popup(&self, lease: &DraftInputLease) -> io::Result<()> {
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        let screen = self.terminal.active_ansi_if_complete().unwrap_or_default();
        let Some(key) = self.inspect.owned_popup_escape(&screen) else {
            return Ok(());
        };
        if self.observe("owned_popup") != InputObservation::Pending {
            return Err(io::Error::other("Picker input is not owned by Bus"));
        }
        self.check_deadline()?;
        let completion = lease.with_uninvalidated(|| {
            self.io.queue_input_write_scoped(
                key,
                bytes::Bytes::new(),
                Duration::ZERO,
                Some(self.deadline.get()),
                &self.scope,
            )
        })?;
        drop(guard);
        completion
            .recv_timeout(self.clock.remaining(self.deadline.get()))
            .map_err(io::Error::other)??;
        // One Esc closes a known picker; never repeat it against a live turn.
        loop {
            let generation = self.clock.generation();
            if self
                .terminal
                .active_ansi_if_complete()
                .is_some_and(|screen| self.inspect.owned_popup_escape(&screen).is_none())
            {
                return Ok(());
            }
            self.check_deadline()?;
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }
}
