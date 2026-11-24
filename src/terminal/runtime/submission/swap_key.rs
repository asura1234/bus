//! Enqueue editor input under the same guard that protects the composer frame.
use super::{InputObservation, ObservedSubmission, PromptNotShown};
use crate::terminal::runtime::draft_input::DraftInputLease;
use bytes::Bytes;
use std::{
    io,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

pub(super) enum SwapKey<'a> {
    Transfer(Bytes),
    Enter,
    Resume(Option<&'a str>),
    EraseMarker {
        keys: Bytes,
        marked: &'a str,
        deadline: Instant,
    },
}

impl ObservedSubmission {
    pub(super) fn swap_key(&self, lease: &DraftInputLease, action: SwapKey<'_>) -> io::Result<()> {
        let submit = matches!(action, SwapKey::Enter | SwapKey::Resume(_));
        let deadline = match action {
            SwapKey::EraseMarker { deadline, .. } => deadline,
            _ if submit => self.deadline.get(),
            _ => self.hard_deadline,
        };
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        let completion = lease.with_uninvalidated(|| {
            let screen = self
                .terminal
                .active_ansi_if_complete()
                .filter(|screen| self.inspect.submission_available(screen))
                .ok_or_else(|| {
                    if matches!(action, SwapKey::Resume(_) | SwapKey::EraseMarker { .. }) {
                        io::Error::other(PromptNotShown::ComposerBusy)
                    } else {
                        io::Error::other("Native input is blocked")
                    }
                })?;
            if let SwapKey::Resume(marker) = action {
                let cursor = self
                    .terminal
                    .cursor_state()
                    .map(|cursor| (cursor.x, cursor.y));
                let pristine = self.startup_suggestion_open.load(Ordering::Acquire);
                // This check and Enter share content_lock with the PTY render
                // path. The pre-wait observation cannot authorize this key.
                if self.inspect.observe_with_launch(&screen, cursor, pristine)
                    != InputObservation::Pending
                    || marker.is_some_and(|marker| {
                        self.inspect.restoration_marker_visible(&screen, marker)
                    })
                {
                    return Err(io::Error::other(PromptNotShown::ComposerBusy));
                }
            }
            if let SwapKey::EraseMarker { marked, .. } = action {
                if !self.matches_exact_editor_line(&screen, marked) {
                    return Err(io::Error::other(PromptNotShown::ComposerBusy));
                }
            }
            if self.clock.now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Draft transfer expired",
                ));
            }
            let key = match action {
                SwapKey::Transfer(key) | SwapKey::EraseMarker { keys: key, .. } => key,
                SwapKey::Enter | SwapKey::Resume(_) => {
                    self.inspect.editor_submit_key(&screen, &self.enter)
                }
            };
            if submit {
                if self.inspect.confirm_consumption() {
                    self.observe("legacy_editor_enter_ownership");
                    self.enter_content_seq
                        .set(self.content_seq.load(Ordering::Acquire));
                }
                self.diagnostics.enter(&key, self.clock.now());
            }
            let (text, enter) = if submit {
                (Bytes::new(), key)
            } else {
                (key, Bytes::new())
            };
            self.io.queue_input_write_scoped(
                text,
                enter,
                Duration::ZERO,
                Some(deadline),
                &self.scope,
            )
        })?;
        drop(guard);
        let result = completion
            .recv_timeout(self.clock.remaining(deadline))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?;
        if submit {
            self.diagnostics.entered(&result, self.clock.now());
            result.map_err(Self::uncertain_submission)
        } else {
            result
        }
    }
}
