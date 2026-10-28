//! A flushed paste may render after the attempt that wrote it has timed out.
use super::{ClearOutcome, InputObservation, ObservedSubmission, PromptNotShown};
use crate::terminal::runtime::draft_input::DraftInputLease;
use std::{io, time::Duration};

// The incidents rendered about two seconds late on a saturated machine. Keep
// ownership far beyond that window, but do not let a provider that drops input
// hold every later message forever. This is an availability bound, not proof
// that a raw-mode provider acknowledged or discarded the paste. Always prefer
// an exact owned match over expiry, and preserve foreign drafts after expiry.
const STALE_PASTE_MAX_AGE: Duration = Duration::from_secs(60);

impl ObservedSubmission {
    pub(super) fn recover_pending_paste(
        &mut self,
        delay: Duration,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        let Some(mut pending) = lease.take_pending() else {
            return Ok(());
        };
        std::mem::swap(&mut self.inspect, &mut pending.inspect);
        std::mem::swap(&mut self.clear, &mut pending.clear);
        let result = self.await_late_paste(
            lease,
            pending.since,
            pending.entered,
            pending.withdraw_grace,
        );
        std::mem::swap(&mut self.inspect, &mut pending.inspect);
        std::mem::swap(&mut self.clear, &mut pending.clear);
        if result.is_err() {
            // Keep the original age and the clearing model, including any
            // proven progress. Never replace this record with the new brief.
            lease.retain_pending(pending);
        } else {
            self.reset_confirmation(delay);
        }
        result
    }

    fn await_late_paste(
        &self,
        lease: &DraftInputLease,
        since: std::time::Instant,
        entered: bool,
        withdraw_grace: Duration,
    ) -> io::Result<()> {
        loop {
            let generation = self.clock.generation();
            // Incoming developer input is buffered by the lease. It revokes
            // old ownership even if it has not reached the composer yet.
            if lease.edited() {
                return Ok(());
            }
            // A restored native stash is positive proof of the developer's
            // draft. Its chip counts can happen to match our old paste.
            if self
                .terminal
                .visible_ansi_if_complete()
                .is_some_and(|screen| self.inspect.native_draft_restored(&screen))
            {
                return Ok(());
            }
            let observation = self.observe("late_paste");
            match observation {
                InputObservation::Pending => {
                    self.deadline
                        .set((self.clock.now() + withdraw_grace).min(self.hard_deadline));
                    let cleared = self.clear_draft();
                    self.diagnostics.cleared(&cleared, self.clock.now());
                    tracing::debug!(target: "bus::terminal::submission",
                        event = "bus.terminal.submission.recovery",
                        outcome = if cleared.is_ok() { "resolved" } else { "retained" },
                        age_ms = self.clock.now().saturating_duration_since(since).as_millis() as u64,
                        "Recovering a prior uncertain paste; no old Enter is replayed");
                    return match cleared {
                        Ok(
                            ClearOutcome::AlreadyEmpty
                            | ClearOutcome::Cleared
                            | ClearOutcome::Accepted,
                        ) => Ok(()),
                        Err(_) => {
                            self.diagnose_refusal("late_paste_clear");
                            Err(io::Error::other(PromptNotShown::ComposerBusy))
                        }
                    };
                }
                InputObservation::Accepted => return Ok(()),
                InputObservation::Empty if entered || lease.turn_completed_since(since) => {
                    return Ok(());
                }
                _ => {}
            }
            if self.clock.now().saturating_duration_since(since) >= STALE_PASTE_MAX_AGE {
                tracing::warn!(target: "bus::terminal::submission",
                    event = "bus.terminal.submission.recovery", outcome = "expired",
                    observation = ?observation,
                    "Old paste ownership expired; normal draft preservation still applies");
                return Ok(());
            }
            if self.check_deadline().is_err() {
                self.diagnose_refusal("late_paste_wait");
                return Err(io::Error::other(PromptNotShown::ComposerBusy));
            }
            // Empty is not proof that a flushed paste was discarded. Do not
            // interleave another paste while the old render is unresolved.
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }
}
