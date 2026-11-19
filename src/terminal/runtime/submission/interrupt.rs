//! A Bus stop retires only its exact, confirmed Claude draft before settlement.
use super::{ClearOutcome, InputObservation, ObservedSubmission, SubmissionHandle};
use crate::terminal::runtime::draft_input::{ConfirmedPaste, DraftInput, DraftInputLease};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::{cell::Cell, io, path::Path, sync::atomic::Ordering, time::Duration};

impl SubmissionHandle {
    #[cfg(test)]
    pub(crate) fn retire_claude_interrupt(
        &self,
        session: &str,
        payloads: &[String],
        stop: Bytes,
    ) -> io::Result<()> {
        self.retire_claude_interrupt_with_receipt(session, payloads, stop, None)
    }

    pub(crate) fn retire_claude_interrupt_with_receipt(
        &self,
        session: &str,
        payloads: &[String],
        stop: Bytes,
        completion_path: Option<&Path>,
    ) -> io::Result<()> {
        // An autonomous turn has no Bus request whose draft can be abandoned.
        if payloads.is_empty() {
            return self.try_send_developer_bytes(stop);
        }
        let lease = DraftInput::acquire(&self.draft_input, &self.io)?;
        lease.restored();
        let key = match completion_path {
            Some(path) => path.to_string_lossy().into_owned(),
            None => {
                let encoded = serde_json::to_vec(&(session, payloads)).map_err(io::Error::other)?;
                format!("{:x}", Sha256::digest(encoded))
            }
        };
        if completion_path.is_some_and(confirmed_on_disk) || lease.completed_retirement(&key) {
            return write_completion(completion_path);
        }
        let (mut receipt, mut receipts) = take_claude_receipts(&lease, session, payloads, &key)?;
        receipt.attempt = Some(key.clone());
        let final_clear_revision = Cell::new(receipt.final_clear_revision);
        let clock = self.io.submission_clock();
        let deadline = clock.now()
            + payloads
                .iter()
                .map(|payload| {
                    crate::protocol::api::submission::claude_retirement_budget(payload.as_bytes())
                })
                .max()
                .expect("nonempty guarded payloads");
        let mut submission = ObservedSubmission {
            terminal: self.terminal.clone(),
            content_seq: self.content_seq.clone(),
            enter_content_seq: std::cell::Cell::new(0),
            io: self.io.clone(),
            scope: self.io.input_scope(),
            content_lock: self.content_write_lock.clone(),
            inspect: receipt.paste.inspect,
            enter: Bytes::new(),
            clear: receipt.paste.clear,
            deadline: std::cell::Cell::new(deadline),
            hard_deadline: deadline,
            withdraw_grace: receipt.paste.withdraw_grace,
            clock,
            diagnostics: Default::default(),
            startup_suggestion_open: self.startup_suggestion_open.clone(),
        };
        let allow_joined_witness = !receipt.cleanup_started;
        let result = (|| {
            if !receipt.stopped {
                let completion = lease.with_unedited(|| {
                    submission.io.queue_input_write_scoped(
                        stop,
                        Bytes::new(),
                        Duration::ZERO,
                        Some(deadline),
                        &submission.scope,
                    )
                })?;
                // Even a lost receipt may have pressed Esc: never send it again.
                receipt.stopped = true;
                completion
                    .recv_timeout(submission.clock.remaining(deadline))
                    .map_err(io::Error::other)??;
            }
            loop {
                let generation = submission.clock.generation();
                if lease.edited() {
                    return Err(io::Error::other(
                        "Developer input revoked retirement ownership",
                    ));
                }
                let guard = submission
                    .content_lock
                    .lock()
                    .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
                if let Some(screen) = submission.terminal.active_ansi_if_complete() {
                    let cursor = submission.terminal.cursor_state().map(|c| (c.x, c.y));
                    let body = submission.inspect.interrupted_input(&screen, cursor);
                    let available = !submission.inspect.native_draft_restored(&screen)
                        && !submission.inspect.native_stash_visible(&screen)
                        && submission.inspect.submission_available(&screen);
                    if available
                        && body.as_deref() == Some("")
                        && final_clear_revision.get().is_some_and(|before| {
                            before != submission.content_seq.load(Ordering::Acquire)
                        })
                    {
                        return Ok(());
                    }
                    if available && allow_joined_witness {
                        if let Some(body) = body.as_deref().filter(|body| *body != receipt.body) {
                            // Choose the exact full restored draft from the same
                            // joined turn. Once any cleanup runs, only its own
                            // recorded partial remainder may authorize a retry.
                            if let Some(mut selected) =
                                take_joined_witness(&mut receipts, session, payloads, body)
                            {
                                std::mem::swap(
                                    &mut submission.inspect,
                                    &mut selected.paste.inspect,
                                );
                                std::mem::swap(&mut submission.clear, &mut selected.paste.clear);
                                submission.withdraw_grace = selected.paste.withdraw_grace;
                                receipt.body = selected.body;
                                receipt.payload = selected.payload;
                            }
                        }
                    }
                    if available
                        && body.as_ref().is_some_and(|body| {
                            body == &receipt.body || submission.inspect.interrupted_remainder(body)
                        })
                    {
                        final_clear_revision.set(None);
                        // Attest the exact full frame before the existing clear
                        // model can authorize any logical-line editing keys.
                        if submission.observe("interrupt_owned") != InputObservation::Pending {
                            return Err(io::Error::other(
                                "Restored Claude draft ownership changed",
                            ));
                        }
                        receipt.cleanup_started = true;
                        drop(guard);
                        return match submission
                            .clear_draft(Some(&lease), Some(&final_clear_revision))?
                        {
                            ClearOutcome::Cleared => Ok(()),
                            _ => Err(io::Error::other(
                                "Claude draft retirement has no cleared readback",
                            )),
                        };
                    }
                    if body.is_some_and(|body| !body.is_empty()) {
                        return Err(io::Error::other(
                            "Changed or foreign Claude draft preserved",
                        ));
                    }
                }
                submission.check_deadline()?;
                drop(guard);
                // Empty/working is not proof against a delayed restoration.
                submission
                    .clock
                    .sleep(Duration::from_millis(20), generation);
            }
        })();
        if result.is_ok() {
            lease.record_completed_retirement(key)?;
            return write_completion(completion_path);
        }
        receipt.final_clear_revision = final_clear_revision.get();
        receipt.paste.inspect = submission.inspect;
        receipt.paste.clear = submission.clear;
        for saved in receipts {
            lease.retain_confirmed(saved, true);
        }
        lease.retain_confirmed(receipt, true);
        result
    }
}

fn take_claude_receipts(
    lease: &DraftInputLease,
    session: &str,
    payloads: &[String],
    key: &str,
) -> io::Result<(ConfirmedPaste, Vec<ConfirmedPaste>)> {
    let mut receipts = lease.take_confirmed();
    let Some(receipt) = receipts.pop() else {
        let reason = if lease.confirmed_revoked() {
            "Claude input receipt was revoked; request remains tracked"
        } else {
            "No confirmed Claude input receipt; request remains tracked"
        };
        return Err(io::Error::other(reason));
    };
    let reason = if receipt.session.as_deref() != Some(session)
        || !payloads.contains(&receipt.payload)
        || receipt
            .attempt
            .as_deref()
            .is_some_and(|attempt| attempt != key)
    {
        "Claude input receipt identity changed"
    } else if receipt.opaque {
        "Claude input receipt is opaque; request remains tracked"
    } else {
        return Ok((receipt, receipts));
    };
    for saved in receipts {
        lease.retain_confirmed(saved, true);
    }
    lease.retain_confirmed(receipt, true);
    Err(io::Error::other(reason))
}

fn take_joined_witness(
    receipts: &mut Vec<ConfirmedPaste>,
    session: &str,
    payloads: &[String],
    body: &str,
) -> Option<ConfirmedPaste> {
    let index = receipts.iter().rposition(|candidate| {
        !candidate.opaque
            && candidate.body == body
            && candidate.session.as_deref() == Some(session)
            && payloads.contains(&candidate.payload)
            && candidate.attempt.is_none()
    })?;
    Some(receipts.remove(index))
}

fn confirmed_on_disk(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file() && metadata.len() == 8)
        && std::fs::read(path).is_ok_and(|bytes| bytes == b"retired\n")
}

fn write_completion(path: Option<&Path>) -> io::Result<()> {
    if let Some(path) = path {
        if !confirmed_on_disk(path) {
            std::fs::create_dir_all(
                path.parent()
                    .ok_or_else(|| io::Error::other("Missing receipt directory"))?,
            )?;
            crate::platform::fs::guarded_atomic_write(path, b"retired\n")?;
        }
    }
    Ok(())
}
