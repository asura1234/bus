//! Snapshot, clear, deliver and restore when the provider handoff is unavailable.
use super::{InputObservation, ObservedSubmission, PromptNotShown};
use crate::protocol::api::submission::{withdrawal_budget, CLEAR_KEY_DELAY};
use crate::terminal::runtime::draft_input::DraftInputLease;
use bytes::Bytes;
use std::{io, sync::atomic::Ordering, time::Duration};

mod recovery;
use recovery::Outcome;

const RESTORE_BUDGET: Duration = Duration::from_secs(1);

impl ObservedSubmission {
    pub(super) fn fallback_copy_after_handoff_failure(
        &self,
        text: Bytes,
        delay: Duration,
        lease: &DraftInputLease,
    ) -> Option<io::Result<()>> {
        let screen = self.terminal.active_ansi_if_complete()?;
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        let pristine = self.startup_suggestion_open.load(Ordering::Acquire);
        let (snapshot, restore_bytes) = self
            .inspect
            .composer_copy_fallback(&screen, cursor, pristine)?;
        self.inspect.clear_native_draft();
        Some(self.preserve_copy(snapshot, restore_bytes, text, delay, lease))
    }

    pub(super) fn preserve_copy(
        &self,
        snapshot: String,
        restore_bytes: Option<Bytes>,
        text: Bytes,
        delay: Duration,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        let backup = self
            .inspect
            .copy_recovery_directory()
            .and_then(|directory| recovery::start(directory, &snapshot));
        if let Err(error) = self.clear_for_copy(&snapshot, lease) {
            if let Some(backup) = backup {
                let _ = backup.send(Outcome::Retain {
                    reason: "clear_incomplete",
                    error: error.to_string(),
                });
            }
            return Err(error);
        }

        self.reset_confirmation(delay);
        let withdrawal_lease =
            (self.inspect.confirm_consumption() || self.inspect.waits_for_idle()).then_some(lease);
        let result = self.confirm_legacy_submission(self.run(text, delay, withdrawal_lease), lease);
        let restored = match (&result, restore_bytes) {
            (Ok(()), Some(restore)) => self.restore_copied_draft(&snapshot, restore, lease),
            (Err(error), _) => Err(io::Error::other(format!(
                "Delivery did not settle: {error}"
            ))),
            (_, None) => Err(io::Error::other(
                "Native draft cannot be reconstructed from its visible snapshot",
            )),
        };
        match restored {
            Ok(()) => {
                lease.restored();
                if let Some(backup) = backup {
                    let _ = backup.send(Outcome::Restored);
                }
            }
            Err(error) => {
                lease.restoration_unproven();
                if let Some(backup) = backup {
                    let _ = backup.send(Outcome::Retain {
                        reason: "copy_restore_failed",
                        error: error.to_string(),
                    });
                } else {
                    let reason = if snapshot.is_empty() {
                        "unreadable_composer_content"
                    } else {
                        "snapshot_storage_unavailable"
                    };
                    tracing::warn!(event = "bus.draft.copy_loss", reason, %error, "Copied draft restoration was not proven and no recovery record was scheduled");
                }
            }
        }
        result
    }

    fn clear_for_copy(&self, snapshot: &str, lease: &DraftInputLease) -> io::Result<()> {
        let deadline = (self.clock.now() + withdrawal_budget(snapshot.as_bytes()))
            .min(self.hard_deadline - self.withdraw_grace);
        // A definite pre-paste refusal lets the queue retry under its
        // existing stall bound if the same pane never accepts clear.
        let not_submitted = || io::Error::other(PromptNotShown::NotSubmitted);
        let mut keys = self.clear.iter().cycle();
        loop {
            if self.clock.now() >= deadline {
                return Err(not_submitted());
            }
            let generation = self.clock.generation();
            let completion = {
                let _guard = self.content_lock.lock().map_err(|_| not_submitted())?;
                let screen = self.terminal.active_ansi_if_complete();
                if screen.as_ref().is_some_and(|screen| {
                    self.inspect.submission_available(screen) && !self.provider_queues_input(screen)
                }) {
                    if self.observe("copy_clear") == InputObservation::Empty {
                        return lease
                            .with_uninvalidated(|| Ok(()))
                            .map_err(|_| not_submitted());
                    }
                    let key = keys.next().ok_or_else(not_submitted)?;
                    Some(
                        lease
                            .with_uninvalidated(|| {
                                self.io.queue_input_write_scoped(
                                    key.clone(),
                                    Bytes::new(),
                                    Duration::ZERO,
                                    Some(deadline),
                                    &self.scope,
                                )
                            })
                            .map_err(|_| not_submitted())?,
                    )
                } else {
                    None
                }
            };
            if let Some(completion) = completion {
                completion
                    .recv_timeout(self.clock.remaining(deadline))
                    .map_err(|_| not_submitted())?
                    .map_err(|_| not_submitted())?;
            }
            self.clock.sleep(CLEAR_KEY_DELAY, generation);
        }
    }

    fn restore_copied_draft(
        &self,
        snapshot: &str,
        restore: Bytes,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        let deadline = (self.clock.now() + RESTORE_BUDGET).min(self.hard_deadline);
        let busy = || io::Error::other(PromptNotShown::ComposerBusy);
        let (completion, before_paste) = loop {
            if self.clock.now() >= deadline {
                return Err(busy());
            }
            let generation = self.clock.generation();
            {
                let _guard = self.content_lock.lock().map_err(|_| busy())?;
                let screen = self.terminal.active_ansi_if_complete();
                if screen
                    .as_ref()
                    .is_some_and(|screen| self.inspect.submission_available(screen))
                    && self.observe("copy_restore_ready") == InputObservation::Empty
                {
                    let seq = self.content_seq.load(Ordering::Acquire);
                    let completion = lease.with_uninvalidated(|| {
                        self.io.queue_input_write_scoped(
                            restore,
                            Bytes::new(),
                            Duration::ZERO,
                            Some(deadline),
                            &self.scope,
                        )
                    })?;
                    break (completion, seq);
                }
                lease.with_uninvalidated(|| Ok(()))?;
            }
            #[cfg(test)]
            self.clock.submission_checked(generation);
            self.clock.sleep(CLEAR_KEY_DELAY, generation);
        };
        // Once enqueued, a lost receipt can't authorize replay of the paste.
        completion
            .recv_timeout(self.clock.remaining(deadline))
            .map_err(|_| busy())??;
        self.wait_transfer_until(deadline, || {
            let _guard = self.content_lock.lock().map_err(|_| busy())?;
            lease.with_uninvalidated(|| {
                if self.content_seq.load(Ordering::Acquire) == before_paste {
                    return Ok(false);
                }
                let Some(screen) = self.terminal.active_ansi_if_complete() else {
                    return Ok(false);
                };
                let cursor = self
                    .terminal
                    .cursor_state()
                    .map(|cursor| (cursor.x, cursor.y));
                Ok(self
                    .inspect
                    .composer_copy_fallback(&screen, cursor, false)
                    .is_some_and(|(body, _)| {
                        body == snapshot
                            // Some providers paint a multiline paste as one
                            // line. Compare words without joining their boundaries.
                            || snapshot.contains('\n')
                                && !body.contains('\n')
                                && body.split_whitespace().eq(snapshot.split_whitespace())
                    }))
            })
        })
    }
}
