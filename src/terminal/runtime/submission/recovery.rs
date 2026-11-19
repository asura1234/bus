//! A flushed paste may render after the attempt that wrote it has timed out.
use super::{ClearOutcome, InputObservation, ObservedSubmission, PromptNotShown};
use crate::protocol::api::submission::{CLEAR_KEY_DELAY, WITHDRAW_GRACE};
use crate::terminal::runtime::draft_input::DraftInputLease;
use bytes::Bytes;
use std::{
    io,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

// The incidents rendered about two seconds late on a saturated machine. Keep
// ownership far beyond that window, but do not let a provider that drops input
// hold every later message forever. This is an availability bound, not proof
// that a raw-mode provider acknowledged or discarded the paste. Always prefer
// an exact owned match over expiry, and preserve foreign drafts after expiry.
const STALE_PASTE_MAX_AGE: Duration = Duration::from_secs(60);

struct ClearProgress {
    revision: u64,
    expires: Instant,
}

impl ObservedSubmission {
    pub(super) fn clear_draft_with_lease(
        &self,
        lease: Option<&DraftInputLease>,
    ) -> io::Result<ClearOutcome> {
        self.clear_draft(lease, None)
    }

    pub(super) fn clear_draft(
        &self,
        lease: Option<&DraftInputLease>,
        retirement: Option<&std::cell::Cell<Option<u64>>>,
    ) -> io::Result<ClearOutcome> {
        let mut sent = false;
        let mut progress = ClearProgress {
            revision: self.inspect.cleanup_progress(),
            expires: self.clock.now() + WITHDRAW_GRACE,
        };
        loop {
            for (index, key) in self.clear.iter().enumerate() {
                if let Some(outcome) = self.clear_step(
                    key.clone(),
                    index,
                    &mut sent,
                    &mut progress,
                    lease,
                    retirement,
                )? {
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
        lease: Option<&DraftInputLease>,
        retirement: Option<&std::cell::Cell<Option<u64>>>,
    ) -> io::Result<Option<ClearOutcome>> {
        let generation = self.clock.generation();
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        let mut retirement_frame = None;
        if let Some(retirement) = retirement {
            let screen = self
                .terminal
                .active_ansi_if_complete()
                .ok_or_else(|| io::Error::other("Incomplete Claude clear readback"))?;
            let cursor = self
                .terminal
                .cursor_state()
                .map(|cursor| (cursor.x, cursor.y));
            let body = self.inspect.interrupted_input(&screen, cursor);
            if self.inspect.native_draft_restored(&screen)
                || self.inspect.native_stash_visible(&screen)
                || !self.inspect.submission_available(&screen)
            {
                return Err(io::Error::other("Claude clear readback changed"));
            }
            if body.as_deref() == Some("") {
                return Ok(Some(
                    if retirement
                        .get()
                        .is_some_and(|before| before != self.content_seq.load(Ordering::Acquire))
                    {
                        ClearOutcome::Cleared
                    } else {
                        ClearOutcome::AlreadyEmpty
                    },
                ));
            }
            // A nonempty readback invalidates an earlier expected empty frame.
            retirement.set(None);
            if !body
                .as_deref()
                .is_some_and(|body| self.inspect.interrupted_remainder(body))
            {
                return Err(io::Error::other(
                    "Claude clear remainder is no longer owned",
                ));
            }
            retirement_frame = Some((screen, cursor));
        }
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
            Some(lease)
                if retirement.is_some()
                    || self.inspect.confirm_consumption()
                    || self.inspect.retry_owned_enter() =>
            {
                lease.with_unedited(queue)
            }
            Some(lease) => lease.with_uninvalidated(queue),
            None => queue(),
        }?;
        if let Some((screen, cursor)) = retirement_frame {
            self.inspect.retirement_clear_frame(index, &screen, cursor);
        }
        self.inspect.clearing(index);
        if let Some(retirement) = retirement {
            if index != 0 && self.inspect.cleanup_expects_empty() {
                retirement.set(Some(self.content_seq.load(Ordering::Acquire)));
            }
        }
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
                .active_ansi_if_complete()
                .is_some_and(|screen| self.inspect.native_draft_restored(&screen))
            {
                return Ok(());
            }
            let observation = self.observe("late_paste");
            match observation {
                InputObservation::Pending => {
                    self.deadline
                        .set((self.clock.now() + withdraw_grace).min(self.hard_deadline));
                    let cleared = self
                        .close_owned_popup(lease)
                        .and_then(|()| self.clear_draft_with_lease(Some(lease)));
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
