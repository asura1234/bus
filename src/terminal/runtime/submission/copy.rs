//! Shared composer snapshot -> send -> restore delivery path.
//!
//! Used whenever composer content is present and native stash / external editor
//! handoff is unavailable or does not respond within its bounded fallback window.
//! Never holds the queue or blocks on proof; logs any loss and delivers anyway.
use super::{InputObservation, ObservedSubmission};
use crate::protocol::api::submission::{withdrawal_budget, CLEAR_KEY_DELAY, WITHDRAW_GRACE};
use bytes::Bytes;
use std::{io, time::Duration};

impl ObservedSubmission {
    pub(super) fn fallback_copy_after_handoff_failure(
        &self,
        text: Bytes,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> Option<io::Result<()>> {
        let screen = self.terminal.active_ansi();
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        let pristine = self
            .startup_suggestion_open
            .load(std::sync::atomic::Ordering::Acquire);
        let (snapshot, restore_bytes) = self
            .inspect
            .composer_copy_fallback(&screen, cursor, pristine)?;
        self.inspect.clear_native_draft();
        Some(self.preserve_copy(snapshot, restore_bytes, text, delay, lease))
    }

    /// Unified shared delivery path when composer content is present:
    /// snapshot (already captured) -> clear -> send new message -> restore copy -> drop copy.
    /// Never holds the queue or blocks on proof; logs any loss and delivers anyway.
    pub(super) fn preserve_copy(
        &self,
        snapshot: String,
        restore_bytes: Option<Bytes>,
        text: Bytes,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        if !snapshot.is_empty() && restore_bytes.is_none() {
            tracing::warn!(
                event = "bus.draft.copy_loss",
                reason = "unrestorable_composer_content",
                "Composer content cannot be restored reliably; delivering anyway"
            );
        }
        let cleared = self.best_effort_clear_composer(&snapshot, lease);
        if !cleared {
            tracing::warn!(
                event = "bus.draft.copy_loss",
                reason = "clear_incomplete",
                "Composer did not confirm empty after clear; delivering anyway"
            );
        }
        self.reset_confirmation(delay);
        let result = self.run_for_copy_delivery(text, delay, lease, cleared);
        if result.is_ok() && !self.inspect.waits_for_idle() {
            let saved_deadline = self.deadline.get();
            self.deadline
                .set((self.clock.now() + Duration::from_secs(2)).min(self.hard_deadline));
            let _ = self.wait_for_consumption();
            self.deadline.set(saved_deadline);
        }
        if let Some(restore) = restore_bytes {
            if let Err(error) = self.restore_copied_draft(restore, lease) {
                tracing::warn!(
                    event = "bus.draft.copy_loss",
                    reason = "restore_failed",
                    %error,
                    "Could not restore copied composer content after delivery"
                );
            }
        }
        lease.restored();
        result
    }

    fn best_effort_clear_composer(
        &self,
        snapshot: &str,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> bool {
        if self.observe("copy_clear") == InputObservation::Empty {
            return true;
        }
        let clear_deadline = (self.clock.now()
            + WITHDRAW_GRACE.max(withdrawal_budget(snapshot.as_bytes())))
        .min(self.hard_deadline);
        let max_passes = snapshot.lines().count().max(1).saturating_mul(2).min(32);
        for _ in 0..max_passes {
            for key in &self.clear {
                if self.observe("copy_clear") == InputObservation::Empty {
                    return true;
                }
                if self.clock.now() >= clear_deadline {
                    return false;
                }
                let generation = self.clock.generation();
                let sent = lease.with_uninvalidated(|| {
                    self.io.queue_input_write_scoped(
                        key.clone(),
                        Bytes::new(),
                        Duration::ZERO,
                        Some(clear_deadline),
                        &self.scope,
                    )
                });
                let Ok(completion) = sent else {
                    return false;
                };
                if completion
                    .recv_timeout(self.clock.remaining(clear_deadline))
                    .ok()
                    .and_then(Result::ok)
                    .is_none()
                {
                    return false;
                }
                self.clock.sleep(CLEAR_KEY_DELAY, generation);
                if self.observe("copy_clear") == InputObservation::Empty {
                    return true;
                }
            }
        }
        self.observe("copy_clear") == InputObservation::Empty
    }

    fn run_for_copy_delivery(
        &self,
        text: Bytes,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
        cleared: bool,
    ) -> io::Result<()> {
        let withdrawal_lease =
            (self.inspect.confirm_consumption() || self.inspect.waits_for_idle()).then_some(lease);
        if cleared {
            return self.confirm_legacy_submission(self.run(text, delay, withdrawal_lease), lease);
        }
        // Clear did not confirm empty; still paste and submit without blocking on proof.
        if self.inspect.retry_owned_enter() {
            self.deadline.set(self.hard_deadline - self.withdraw_grace);
        }
        self.diagnostics
            .paste(text.len(), delay.as_millis() as u64, self.clock.now());
        let completion = self.io.queue_payload_input_write_scoped(
            text,
            Bytes::new(),
            delay,
            Some(self.deadline.get()),
            &self.scope,
        )?;
        let write_result = completion
            .recv_timeout(self.clock.remaining(self.deadline.get()))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
            .and_then(|result| result);
        if write_result.is_ok() {
            self.startup_suggestion_open
                .store(false, std::sync::atomic::Ordering::Release);
        }
        self.diagnostics.pasted(&write_result, self.clock.now());
        write_result?;
        self.await_submit_key_or_withdraw(withdrawal_lease)?;
        let entered_at = self.clock.now();
        #[cfg(test)]
        self.clock.entered();
        if self.inspect.waits_for_idle() {
            return self.confirm_claude_submission(withdrawal_lease, entered_at);
        }
        self.confirm_legacy_submission(Ok(()), lease)
    }

    fn restore_copied_draft(
        &self,
        restore: Bytes,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        let deadline = self
            .clock
            .now()
            .checked_add(Duration::from_secs(1))
            .unwrap_or(self.hard_deadline)
            .min(self.hard_deadline);
        let completion = lease.with_uninvalidated(|| {
            self.io.queue_input_write_scoped(
                restore,
                Bytes::new(),
                Duration::ZERO,
                Some(deadline),
                &self.scope,
            )
        })?;
        completion
            .recv_timeout(self.clock.remaining(deadline))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?
    }
}
