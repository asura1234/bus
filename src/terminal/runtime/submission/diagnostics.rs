//! Content-free native submission evidence, owned by the writer thread.
use super::{ClearOutcome, InputObservation, PromptNotShown};
use std::{cell::Cell, io, time::Instant};

#[derive(Default)]
pub(super) struct Diagnostics {
    refusal_recorded: Cell<bool>,
    started: Cell<Option<Instant>>,
    confirmation_ms: Cell<u64>,
    hard_ms: Cell<u64>,
    observations: Cell<usize>,
    last: Cell<Option<InputObservation>>,
    owned_seen: Cell<bool>,
    paste_bytes: Cell<usize>,
    paste_flushed: Cell<bool>,
    enter_attempts: Cell<usize>,
    enter_flushed: Cell<usize>,
    cleanup: Cell<&'static str>,
}

impl Diagnostics {
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
        self.owned_seen
            .set(self.owned_seen.get() || observed == InputObservation::Pending);
        let changed = self.last.replace(Some(observed)) != Some(observed);
        if changed {
            tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.observed", phase, observation = ?observed, screen_bytes, elapsed_ms = self.elapsed(now));
        }
    }

    pub(super) fn paste(&self, bytes: usize, delay_ms: u64, now: Instant) {
        self.paste_bytes.set(bytes);
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.paste", bytes, delay_ms, elapsed_ms = self.elapsed(now));
    }

    pub(super) fn pasted(&self, result: &io::Result<()>, now: Instant) {
        self.paste_flushed.set(result.is_ok());
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.paste_flushed", flushed = result.is_ok(), error_kind = ?result.as_ref().err().map(io::Error::kind), elapsed_ms = self.elapsed(now));
    }

    pub(super) fn enter(&self, bytes: &bytes::Bytes, now: Instant) {
        self.enter_attempts.set(self.enter_attempts.get() + 1);
        tracing::debug!(target: "bus::terminal::submission", event = "bus.terminal.submission.enter", attempt = self.enter_attempts.get(), key = ?bytes, elapsed_ms = self.elapsed(now));
    }

    pub(super) fn entered(&self, result: &io::Result<()>, now: Instant) {
        self.enter_flushed
            .set(self.enter_flushed.get() + usize::from(result.is_ok()));
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
        tracing::info!(target: "bus::terminal::submission", event = "bus.terminal.submission.summary", outcome,
            paste_bytes = self.paste_bytes.get(), paste_flushed = self.paste_flushed.get(),
            owned_seen = self.owned_seen.get(), enter_attempts = self.enter_attempts.get(), enter_flushed = self.enter_flushed.get(),
            observations = self.observations.get(), final_observation = ?self.last.get(), cleanup = self.cleanup.get(),
            error_kind = ?result.as_ref().err().map(io::Error::kind), elapsed_ms = self.elapsed(now),
            confirmation_ms = self.confirmation_ms.get(), hard_ms = self.hard_ms.get());
    }
}
