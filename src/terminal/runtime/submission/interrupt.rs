//! A Bus stop retires only its exact, confirmed Claude draft before settlement.
use super::{ClearOutcome, InputObservation, ObservedSubmission, SubmissionHandle};
use crate::agents::providers::claude_code::retirement::{CancelWatermark, TranscriptAnchor};
use crate::terminal::runtime::draft_input::{ConfirmedPaste, DraftInput, DraftInputLease};
use bytes::Bytes;
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    io,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

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

    #[cfg(test)]
    pub(crate) fn retire_claude_interrupt_with_receipt(
        &self,
        session: &str,
        payloads: &[String],
        stop: Bytes,
        completion_path: Option<&Path>,
    ) -> io::Result<()> {
        self.retire_claude_interrupt_with_anchor(session, payloads, stop, completion_path, None)
    }

    pub(crate) fn retire_claude_interrupt_with_anchor(
        &self,
        session: &str,
        payloads: &[String],
        stop: Bytes,
        completion_path: Option<&Path>,
        anchor: Option<&TranscriptAnchor>,
    ) -> io::Result<()> {
        // An autonomous turn has no Bus request whose draft can be abandoned.
        if payloads.is_empty() {
            return self.try_send_developer_bytes(stop);
        }
        let lease = DraftInput::acquire(&self.draft_input, &self.io)?;
        lease.restored();
        let key = retirement_key(session, payloads, completion_path)?;
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
        let mut submission = self.interrupt_observation(
            receipt.paste.inspect,
            receipt.paste.clear,
            receipt.paste.withdraw_grace,
            deadline,
            clock,
        );
        let allow_joined_witness = !receipt.cleanup_started;
        let anchor = anchor.filter(|_| {
            payloads.len() == 1
                && receipts.is_empty()
                && !submission.inspect.retains_joined_claude_receipts()
                && allow_joined_witness
        });
        let result = (|| {
            let mut no_restoration = press_claude_stop(
                &submission,
                &lease,
                stop,
                &mut receipt.stopped,
                anchor,
                session,
            )?;
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
                let mut verified_empty = false;
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
                    verified_empty = no_restoration.as_ref().is_some_and(|proof| {
                        available
                            && body.as_deref() == Some("")
                            && submission.content_seq.load(Ordering::Acquire) != proof.before_stop
                            && submission
                                .inspect
                                .claude_empty_editable_composer(&screen, cursor)
                    });
                    if body.is_some_and(|body| !body.is_empty()) {
                        return Err(io::Error::other(
                            "Changed or foreign Claude draft preserved",
                        ));
                    }
                }
                if no_restoration.as_mut().is_some_and(|proof| {
                    proof.observe(verified_empty, submission.clock.now(), deadline)
                }) && lease.session().as_deref() == Some(session)
                    && !lease.edited()
                {
                    return Ok(());
                }
                submission.check_deadline()?;
                drop(guard);
                // Keep looking for actual restoration through the entire budget.
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

    fn interrupt_observation(
        &self,
        inspect: Box<dyn super::SubmissionInspector>,
        clear: Vec<Bytes>,
        withdraw_grace: Duration,
        deadline: Instant,
        clock: super::super::submission_clock::SubmissionClock,
    ) -> ObservedSubmission {
        ObservedSubmission {
            terminal: self.terminal.clone(),
            content_seq: self.content_seq.clone(),
            enter_content_seq: std::cell::Cell::new(0),
            io: self.io.clone(),
            scope: self.io.input_scope(),
            content_lock: self.content_write_lock.clone(),
            inspect,
            enter: Bytes::new(),
            clear,
            deadline: std::cell::Cell::new(deadline),
            hard_deadline: deadline,
            withdraw_grace,
            clock,
            diagnostics: Default::default(),
            startup_suggestion_open: self.startup_suggestion_open.clone(),
        }
    }
}

fn press_claude_stop<'a>(
    submission: &ObservedSubmission,
    lease: &DraftInputLease,
    stop: Bytes,
    stopped: &mut bool,
    anchor: Option<&'a TranscriptAnchor>,
    session: &str,
) -> io::Result<Option<NoRestoration<'a>>> {
    if *stopped {
        return Ok(None);
    }
    // Only the first Esc can establish a fresh transcript boundary.
    let proof = anchor
        .and_then(|anchor| anchor.watermark(session))
        .map(|watermark| NoRestoration {
            watermark,
            before_stop: submission.content_seq.load(Ordering::Acquire),
            empty_since: None,
        });
    let deadline = submission.hard_deadline;
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
    *stopped = true;
    completion
        .recv_timeout(submission.clock.remaining(deadline))
        .map_err(io::Error::other)??;
    Ok(proof)
}

struct NoRestoration<'a> {
    watermark: CancelWatermark<'a>,
    before_stop: u64,
    empty_since: Option<Instant>,
}

impl NoRestoration<'_> {
    fn observe(&mut self, empty: bool, now: Instant, deadline: Instant) -> bool {
        if !empty || !self.watermark.has_cancellation() {
            self.empty_since = None;
            return false;
        }
        let since = *self.empty_since.get_or_insert(now);
        // Re-read the linked cancellation at the last complete composer sample.
        now >= deadline
            && now - since >= Duration::from_millis(150)
            && self.watermark.cancellation().is_some()
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

fn retirement_key(
    session: &str,
    payloads: &[String],
    completion_path: Option<&Path>,
) -> io::Result<String> {
    if let Some(path) = completion_path {
        return Ok(path.to_string_lossy().into_owned());
    }
    let encoded = serde_json::to_vec(&(session, payloads)).map_err(io::Error::other)?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
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
