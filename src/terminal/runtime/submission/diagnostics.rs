//! Content-free native submission evidence, owned by the writer thread.
use super::{ClearOutcome, InputObservation, PromptNotShown};
use std::{cell::Cell, io, time::Instant};

#[derive(Default)]
pub(super) struct Diagnostics {
    timestamps: crate::terminal::pty::actor::SharedSubmissionTimestamps,
    refusal_recorded: Cell<bool>,
    started: Cell<Option<Instant>>,
    confirmation_ms: Cell<u64>,
    hard_ms: Cell<u64>,
    observations: Cell<usize>,
    last: Cell<Option<InputObservation>>,
    owned_seen: Cell<bool>,
    paste_bytes: Cell<usize>,
    paste_flushed: Cell<bool>,
    paste_at: Cell<Option<Instant>>,
    enter_attempts: Cell<usize>,
    enter_flushed: Cell<usize>,
    cleanup: Cell<&'static str>,
}

impl Diagnostics {
    pub(super) fn enter_attempted(&self) -> bool {
        self.enter_attempts.get() > 0
    }
    pub(super) fn with_timestamps(
        timestamps: crate::terminal::pty::actor::SharedSubmissionTimestamps,
    ) -> Self {
        Self {
            timestamps,
            ..Default::default()
        }
    }

    fn record(
        &self,
        update: impl FnOnce(&mut crate::protocol::api::schema::NativeSubmissionTimestamps),
    ) {
        update(
            &mut self
                .timestamps
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
    }

    pub(super) fn editor_installed(&self, bytes: usize) {
        let at_ms = crate::utils::time::now_ms();
        self.record(|timestamps| {
            timestamps.text_written_at_ms.get_or_insert(at_ms);
        });
        tracing::info!(target: "bus::terminal::submission", event = "bus.terminal.submission.editor_installed", at_ms, bytes, "Bus input installed through native editor");
    }

    pub(super) fn retain_editor_input(&self, now: Instant) {
        // This is an owned input age, not a PTY paste receipt. Keep the paste
        // byte counters and timing fields separate from native-editor writes.
        self.paste_at.set(Some(now));
    }

    pub(super) fn editor_restored(&self) {
        self.paste_at.set(None);
    }

    pub(super) fn ownership_verified(&self) {
        self.record(|timestamps| {
            timestamps
                .composer_owned_at_ms
                .get_or_insert(crate::utils::time::now_ms());
        });
    }

    pub(super) fn claim_refusal(&self) -> bool {
        !self.refusal_recorded.replace(true)
    }

    pub(super) fn begin(&self, now: Instant, confirmation: Instant, hard: Instant) {
        self.started.set(Some(now));
        self.confirmation_ms
            .set(confirmation.saturating_duration_since(now).as_millis() as u64);
        self.hard_ms
            .set(hard.saturating_duration_since(now).as_millis() as u64);
        self.cleanup.set("not_needed");
    }

    fn elapsed(&self, now: Instant) -> u64 {
        self.started.get().map_or(0, |start| {
            now.saturating_duration_since(start).as_millis() as u64
        })
    }

    pub(super) fn observe(
        &self,
        phase: &'static str,
        observed: InputObservation,
        screen_bytes: usize,
        now: Instant,
    ) {
        self.observations.set(self.observations.get() + 1);
        if observed == InputObservation::Pending {
            self.owned_seen.set(true);
            self.record(|timestamps| {
                timestamps
                    .composer_owned_at_ms
                    .get_or_insert(crate::utils::time::now_ms());
            });
        }
        let changed = self.last.replace(Some(observed)) != Some(observed);
        if changed {
            tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.observed", phase, observation = ?observed, screen_bytes, elapsed_ms = self.elapsed(now));
        }
    }

    pub(super) fn paste(&self, bytes: usize, delay_ms: u64, now: Instant) {
        self.paste_bytes.set(bytes);
        self.paste_at.set(Some(now));
        self.record(|timestamps| {
            timestamps
                .paste_started_at_ms
                .get_or_insert(crate::utils::time::now_ms());
        });
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.paste", bytes, delay_ms, elapsed_ms = self.elapsed(now));
    }

    pub(super) fn pasted(&self, result: &io::Result<()>, now: Instant) {
        self.paste_flushed.set(result.is_ok());
        if result.is_ok() {
            self.paste_at.set(Some(now));
            self.record(|timestamps| {
                timestamps
                    .paste_completed_at_ms
                    .get_or_insert(crate::utils::time::now_ms());
            });
        }
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.paste_flushed", flushed = result.is_ok(), error_kind = ?result.as_ref().err().map(io::Error::kind), elapsed_ms = self.elapsed(now));
    }

    pub(super) fn enter(&self, bytes: &bytes::Bytes, now: Instant) {
        self.enter_attempts.set(self.enter_attempts.get() + 1);
        self.record(|timestamps| {
            timestamps
                .enter_sent_at_ms
                .get_or_insert(crate::utils::time::now_ms());
        });
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.enter", attempt = self.enter_attempts.get(), key = ?bytes, elapsed_ms = self.elapsed(now));
    }

    pub(super) fn entered(&self, result: &io::Result<()>, now: Instant) {
        self.enter_flushed
            .set(self.enter_flushed.get() + usize::from(result.is_ok()));
        if result.is_ok() {
            self.record(|timestamps| {
                timestamps
                    .enter_written_at_ms
                    .get_or_insert(crate::utils::time::now_ms());
            });
        }
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.enter_flushed", attempt = self.enter_attempts.get(), flushed = result.is_ok(), error_kind = ?result.as_ref().err().map(io::Error::kind), elapsed_ms = self.elapsed(now));
    }

    pub(super) fn cleared(&self, result: &io::Result<ClearOutcome>, now: Instant) {
        let outcome = match result {
            Ok(ClearOutcome::AlreadyEmpty) => "already_empty",
            Ok(ClearOutcome::Accepted) => "accepted",
            Ok(ClearOutcome::Cleared) => "cleared",
            Err(_) => "unsafe_or_failed",
        };
        self.cleanup.set(outcome);
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.cleanup", outcome, final_observation = ?self.last.get(), error_kind = ?result.as_ref().err().map(io::Error::kind), elapsed_ms = self.elapsed(now));
    }

    pub(super) fn cleanup_refused(&self, now: Instant) {
        self.cleanup.set("ownership_lost_or_unavailable");
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.cleanup", outcome = self.cleanup.get(), final_observation = ?self.last.get(), elapsed_ms = self.elapsed(now));
    }

    pub(super) fn finish(&self, result: &io::Result<()>, now: Instant) {
        let outcome = match result {
            Ok(()) => "confirmed",
            Err(error) if PromptNotShown::is(error) => "not_submitted",
            Err(_) => "uncertain",
        };
        if result.is_ok() {
            self.record(|timestamps| {
                timestamps
                    .native_confirmed_at_ms
                    .get_or_insert(crate::utils::time::now_ms());
            });
        }
        tracing::info!(target: "bus::terminal::submission", event = "bus.terminal.submission.summary", outcome,
            paste_bytes = self.paste_bytes.get(), paste_flushed = self.paste_flushed.get(),
            owned_seen = self.owned_seen.get(), enter_attempts = self.enter_attempts.get(), enter_flushed = self.enter_flushed.get(),
            observations = self.observations.get(), final_observation = ?self.last.get(), cleanup = self.cleanup.get(),
            error_kind = ?result.as_ref().err().map(io::Error::kind), elapsed_ms = self.elapsed(now),
            confirmation_ms = self.confirmation_ms.get(), hard_ms = self.hard_ms.get());
    }

    pub(super) fn pending_paste(&self, result: &io::Result<()>) -> Option<(Instant, bool)> {
        if result
            .as_ref()
            .is_err_and(|error| !PromptNotShown::is(error))
        {
            self.written_paste()
        } else {
            None
        }
    }

    pub(super) fn written_paste(&self) -> Option<(Instant, bool)> {
        self.paste_at
            .get()
            .map(|at| (at, self.enter_attempts.get() > 0))
    }
}
