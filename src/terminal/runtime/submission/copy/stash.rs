//! Reconstruct a Claude stash promoted when Bus submitted a copied draft.
use super::{recovery, DraftInputLease, ObservedSubmission, Outcome, Readback};
use bytes::Bytes;
use std::{
    io,
    sync::{atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

enum StashFrame {
    Occupied,
    Promoted {
        snapshot: String,
        attested: bool,
        distinct: bool,
    },
}

impl ObservedSubmission {
    pub(super) fn retain_visible_promotion(&self, error: &io::Error) {
        let frame = {
            let Ok(_guard) = self.content_lock.lock() else {
                return;
            };
            self.terminal
                .active_ansi_if_complete()
                .as_deref()
                .and_then(|screen| self.copy_stash_frame(screen))
        };
        let Some(StashFrame::Promoted { snapshot, .. }) = frame else {
            return;
        };
        if let Some(backup) = self
            .inspect
            .copy_recovery_directory()
            .and_then(|directory| {
                recovery::start(directory, &snapshot, &self.diagnostics.copy_recovery)
            })
        {
            backup.send(retain(error));
        } else {
            self.diagnostics.copy_recovery.copy_loss(
                "snapshot_storage_unavailable",
                io::Error::other(error.to_string()),
            );
        }
    }

    pub(super) fn restore_occupied_stash_copy(
        &self,
        visible: &str,
        paste: Bytes,
        deadline: Instant,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        let mut frame = None;
        if let Err(error) = self.wait_transfer_until(deadline, || {
            let _guard = self.content_lock.lock().map_err(|_| transfer_changed())?;
            frame = self
                .editable_stash_screen()
                .as_deref()
                .and_then(|screen| self.copy_stash_frame(screen));
            Ok(frame.is_some())
        }) {
            self.retain_visible_promotion(&error);
            return Err(error);
        }
        let Some(StashFrame::Promoted {
            snapshot,
            attested,
            distinct,
        }) = frame
        else {
            let result = self.restore_copied_draft(
                visible,
                paste,
                deadline,
                Readback::ExactWithOccupiedStash,
                lease,
            );
            if let Err(error) = &result {
                self.retain_visible_promotion(error);
            }
            return result;
        };

        let (ready, verified) = mpsc::channel();
        let diagnostics = &self.diagnostics.copy_recovery;
        let directory = self.inspect.copy_recovery_directory().ok_or_else(|| {
            io::Error::other("Promoted draft left in composer; recovery storage unavailable")
        })?;
        let backup = recovery::start_with_receipt(directory, &snapshot, diagnostics, Some(ready));
        let result = (|| {
            let key = self
                .inspect
                .copy_stash_key()
                .ok_or_else(|| io::Error::other("Native stash key is unavailable"))?;
            if !distinct {
                return Err(io::Error::other(
                    "Promoted draft cannot be verified distinct from the incoming prompt",
                ));
            }
            if !attested {
                return Err(io::Error::other(
                    "Promoted draft left native; readback may lose whitespace or native objects",
                ));
            }
            // A missed durability receipt leaves P where it was. With a
            // verified backup, move the original native object directly:
            // the terminal's representation can omit trailing spaces.
            verified
                .recv_timeout(self.clock.remaining(deadline))
                .map_err(|_| {
                    io::Error::other(
                        "Promoted draft left in composer; recovery copy was not verified",
                    )
                })?;
            self.stash_verified_promotion(&snapshot, key, deadline, lease)?;
            self.restore_copied_draft(
                visible,
                paste,
                deadline,
                Readback::ExactWithOccupiedStash,
                lease,
            )
        })();
        if let Some(backup) = backup {
            backup.send(match &result {
                Ok(()) => Outcome::Restored,
                Err(error) => retain(error),
            });
        } else if let Err(error) = &result {
            diagnostics.copy_loss(
                "snapshot_storage_unavailable",
                io::Error::other(error.to_string()),
            );
        }
        result
    }

    fn editable_stash_screen(&self) -> Option<String> {
        self.terminal.active_ansi_if_complete().filter(|screen| {
            self.inspect.submission_available(screen) && !self.provider_queues_input(screen)
        })
    }

    fn copy_stash_frame(&self, screen: &str) -> Option<StashFrame> {
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        let (snapshot, attested) = self.inspect.copy_stash_readback(screen, cursor)?;
        let occupied = self.inspect.native_stash_visible(screen);
        if occupied && snapshot.is_empty() {
            Some(StashFrame::Occupied)
        } else if !occupied && !snapshot.is_empty() {
            let distinct = self.inspect.copy_stash_payload_is_distinct(screen, cursor);
            Some(StashFrame::Promoted {
                snapshot,
                attested,
                distinct,
            })
        } else {
            None
        }
    }

    fn stash_verified_promotion(
        &self,
        snapshot: &str,
        key: Bytes,
        deadline: Instant,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        let (completion, before) = {
            let _guard = self.content_lock.lock().map_err(|_| transfer_changed())?;
            let promoted = self
                .editable_stash_screen()
                .as_deref()
                .and_then(|screen| self.copy_stash_frame(screen));
            if self.clock.now() >= deadline
                || !matches!(promoted, Some(StashFrame::Promoted { snapshot: ref body, attested: true, distinct: true }) if body == snapshot)
            {
                return Err(transfer_changed());
            }
            let before = self.content_seq.load(Ordering::Acquire);
            let completion = lease.with_uninvalidated(|| {
                self.io.queue_input_write_scoped(
                    key,
                    Bytes::new(),
                    Duration::ZERO,
                    Some(deadline),
                    &self.scope,
                )
            })?;
            (completion, before)
        };
        completion
            .recv_timeout(self.clock.remaining(deadline))
            .map_err(io::Error::other)??;
        self.wait_transfer_until(deadline, || {
            let _guard = self.content_lock.lock().map_err(|_| transfer_changed())?;
            lease.with_uninvalidated(|| {
                Ok(self.content_seq.load(Ordering::Acquire) != before
                    && self
                        .editable_stash_screen()
                        .as_deref()
                        .and_then(|screen| self.copy_stash_frame(screen))
                        .is_some_and(|frame| matches!(frame, StashFrame::Occupied)))
            })
        })
    }
}

fn transfer_changed() -> io::Error {
    io::Error::other("Claude composer or stash changed during copied draft restoration")
}

fn retain(error: &io::Error) -> Outcome {
    Outcome::Retain {
        reason: "copy_stash_restore_failed",
        error: error.to_string(),
    }
}
