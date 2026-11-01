//! Transfer a developer draft only while pane input is exclusively reserved.
use super::{InputObservation, ObservedSubmission, PromptNotShown};
use crate::platform::draft_editor::Exchange;
use crate::protocol::api::submission::confirmation_budget;
use bytes::Bytes;
use std::{io, path::PathBuf, time::Duration};

pub(crate) enum DraftPreservation {
    Hold,
    Native {
        key: Bytes,
        directory: PathBuf,
        visible_draft: String,
    },
    Editor {
        key: Bytes,
        backspace: Bytes,
        directory: PathBuf,
    },
}

pub(crate) struct EditorSubmission {
    pub(crate) text: String,
    pub(crate) directory: PathBuf,
    pub(crate) key: Bytes,
    pub(crate) backspace: Bytes,
    pub(crate) verification_error: Option<&'static str>,
}

impl ObservedSubmission {
    pub(super) fn run_preserving_draft(
        &self,
        text: Bytes,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        let initial = match self.observe_initial("draft_preflight") {
            Ok(initial) => initial,
            Err(error) => {
                // No preservation or Bus input happened; queued developer
                // keys can return to the untouched native composer.
                lease.restored();
                return Err(error);
            }
        };
        let screen = self.terminal.visible_ansi();
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        let pristine = self
            .startup_suggestion_open
            .load(std::sync::atomic::Ordering::Acquire);
        let preservation = self
            .inspect
            .draft_preservation_since_launch(&screen, cursor, pristine);
        match initial {
            InputObservation::Empty if preservation.is_none() => {
                if let Some(editor) = self.inspect.editor_submission() {
                    return self.submit_through_editor(editor, delay, lease);
                }
                let result = self.run(text, delay);
                if self
                    .inspect
                    .native_draft_restored(&self.terminal.visible_ansi())
                    || matches!(
                        self.observe("input_replay_preflight"),
                        InputObservation::Empty | InputObservation::Accepted
                    )
                {
                    lease.restored();
                }
                return result;
            }
            InputObservation::Empty | InputObservation::Other | InputObservation::Pending => {}
            _ => {
                lease.restored();
                return self.run(text, delay);
            }
        }
        let Some(preservation) = preservation else {
            lease.restored();
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        };
        // This lock is also held by the render task. Never act on a changed
        // frame, including a dialog that appeared after the first inspection.
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        if self.terminal.visible_ansi() != screen {
            lease.restored();
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        drop(guard);
        match preservation {
            DraftPreservation::Hold => {
                lease.restored();
                Err(io::Error::other(PromptNotShown::ComposerBusy))
            }
            DraftPreservation::Native {
                key,
                directory,
                visible_draft,
            } => self.preserve_native(text, delay, key, directory, visible_draft, lease),
            DraftPreservation::Editor {
                key,
                backspace,
                directory,
            } => self.preserve_editor(text, delay, key, backspace, directory, lease),
        }
    }

    fn exchange(&self, directory: PathBuf) -> io::Result<Exchange> {
        Exchange::begin(
            directory.clone(),
            self.hard_deadline
                .saturating_duration_since(self.clock.now()),
        ).map_err(|error| {
            tracing::warn!(event = "bus.draft.exchange_wait", directory = %directory.display(), %error, "Draft transfer is unavailable; delivery remains queued");
            io::Error::other(PromptNotShown::ComposerBusy)
        })
    }

    fn transfer_key(&self, key: Bytes) -> io::Result<()> {
        if self.clock.now() >= self.hard_deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Draft transfer deadline expired",
            ));
        }
        self.io
            .queue_input_write_scoped(
                key,
                Bytes::new(),
                Duration::ZERO,
                Some(self.hard_deadline),
                &self.scope,
            )?
            .recv_timeout(self.clock.remaining(self.hard_deadline))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))?
    }

    fn wait_transfer(&self, mut ready: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
        loop {
            let generation = self.clock.generation();
            if ready()? {
                return Ok(());
            }
            if self.clock.now() >= self.hard_deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Provider did not finish draft transfer",
                ));
            }
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }

    pub(super) fn reset_confirmation(&self, delay: Duration) {
        self.deadline.set(
            (self.clock.now() + confirmation_budget(delay))
                .min(self.hard_deadline - self.withdraw_grace),
        );
    }

    fn preserve_native(
        &self,
        text: Bytes,
        delay: Duration,
        key: Bytes,
        directory: PathBuf,
        visible: String,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        let exchange = self.exchange(directory).inspect_err(|_| lease.restored())?;
        if let Err(error) = exchange.save_recovery(&visible) {
            lease.restored();
            self.recovery_log(&exchange, &error);
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        if let Err(error) = self.transfer_key(key.clone()) {
            self.recovery_log(&exchange, &error);
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        if let Err(error) = self.wait_transfer(|| {
            Ok(
                self.observe("await_native_stash") == InputObservation::Empty
                    && self
                        .inspect
                        .native_stash_visible(&self.terminal.visible_ansi()),
            )
        }) {
            self.recovery_log(&exchange, &error);
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        self.reset_confirmation(delay);
        let result = self.run(text, delay);
        // Native stash pops at the next submit. On an unsubmitted attempt,
        // restore only into an empty composer; never toggle over another draft.
        if !self
            .inspect
            .native_draft_restored(&self.terminal.visible_ansi())
            && self.observe("native_restore_preflight") == InputObservation::Empty
        {
            let restored = self.transfer_key(key).and_then(|()| {
                self.wait_transfer(|| {
                    Ok(self
                        .inspect
                        .native_draft_restored(&self.terminal.visible_ansi()))
                })
            });
            if let Err(error) = restored {
                self.recovery_log(&exchange, &error);
                return result;
            }
        }
        if self
            .inspect
            .native_draft_restored(&self.terminal.visible_ansi())
        {
            lease.restored();
            if let Err(error) = exchange.remove_recovery() {
                self.recovery_log(&exchange, &error);
            }
        } else {
            self.recovery_log(
                &exchange,
                &io::Error::other(
                    "Native stash retained; press the provider stash key to recover it",
                ),
            );
        }
        result
    }

    fn editor_handoff(&self, exchange: &Exchange, key: &Bytes) -> io::Result<()> {
        self.transfer_key(key.clone())?;
        self.wait_transfer(|| exchange.acknowledged())
    }

    /// Queue a key only while both the rendered composer and the input lease
    /// still match the evidence used to authorize it.
    fn guarded_editor_key(
        &self,
        key: Bytes,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
        confirmation: bool,
        valid: impl FnOnce(&str) -> bool,
    ) -> io::Result<()> {
        let deadline = if confirmation {
            self.deadline.get()
        } else {
            self.hard_deadline
        };
        if self.clock.now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Editor transfer expired",
            ));
        }
        let guard = self
            .content_lock
            .lock()
            .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
        let screen = self
            .terminal
            .visible_ansi_if_complete()
            .ok_or_else(|| io::Error::other("Composer repaint is incomplete"))?;
        if !valid(&screen) {
            return Err(io::Error::other("Composer changed during editor transfer"));
        }
        let key = if confirmation {
            self.inspect.editor_submit_key(&screen, &key)
        } else {
            key
        };
        if self.clock.now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Editor transfer expired",
            ));
        }
        if confirmation {
            self.diagnostics.enter(&key, self.clock.now());
        }
        let completion = lease.with_unedited(|| {
            self.io.queue_input_write_scoped(
                key,
                Bytes::new(),
                Duration::ZERO,
                Some(deadline),
                &self.scope,
            )
        })?;
        drop(guard);
        let result = completion
            .recv_timeout(self.clock.remaining(deadline))
            .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))
            .and_then(|result| result);
        if confirmation {
            self.diagnostics.entered(&result, self.clock.now());
        }
        result
    }

    fn checked_editor_marker(
        &self,
        exchange: &Exchange,
        backspace: &Bytes,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<String> {
        self.wait_transfer(|| {
            if lease.edited() {
                return Err(io::Error::other(
                    "Developer input changed during editor transfer",
                ));
            }
            Ok(self
                .terminal
                .visible_ansi_if_complete()
                .is_some_and(|screen| {
                    self.inspect
                        .restoration_marker_visible(&screen, exchange.marker())
                }))
        })?;
        let mut keys = Vec::with_capacity(backspace.len() * exchange.marker().len());
        for _ in exchange.marker().bytes() {
            keys.extend_from_slice(backspace);
        }
        self.guarded_editor_key(Bytes::from(keys), lease, false, |screen| {
            self.inspect
                .restoration_marker_visible(screen, exchange.marker())
        })?;
        let mut result = None;
        self.wait_transfer(|| {
            if lease.edited() {
                return Err(io::Error::other(
                    "Developer input changed during editor transfer",
                ));
            }
            if let Some(screen) = self.terminal.visible_ansi_if_complete() {
                if self.inspect.editor_frame_readable(&screen)
                    && !self
                        .inspect
                        .restoration_marker_visible(&screen, exchange.marker())
                {
                    result = Some(screen);
                    return Ok(true);
                }
            }
            Ok(false)
        })?;
        result.ok_or_else(|| io::Error::other("Editor frame is unavailable"))
    }

    fn submit_through_editor(
        &self,
        editor: EditorSubmission,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        if editor.text.len() as u64 > crate::platform::draft_editor::MAX_BYTES {
            lease.restored();
            return Err(io::Error::other(
                "Prompt exceeds the 2 MiB editor exchange limit",
            ));
        }
        let exchange = self
            .exchange(editor.directory)
            .inspect_err(|_| lease.restored())?;
        exchange
            .save_recovery(&editor.text)
            .inspect_err(|_| lease.restored())?;
        let transfer = (|| {
            exchange.request_restore()?;
            self.guarded_editor_key(editor.key.clone(), lease, false, |screen| {
                self.inspect.observe(screen) == InputObservation::Empty
            })?;
            self.wait_transfer(|| exchange.acknowledged())?;
            let first = self.checked_editor_marker(&exchange, &editor.backspace, lease)?;
            let verified = if editor.verification_error.is_some() {
                exchange.request_verify()?;
                self.guarded_editor_key(editor.key.clone(), lease, false, |screen| {
                    self.inspect.editor_frame_unchanged(&first, screen)
                })?;
                self.wait_transfer(|| exchange.acknowledged())?;
                self.checked_editor_marker(&exchange, &editor.backspace, lease)?
            } else {
                first
            };
            self.reset_confirmation(delay);
            self.check_deadline()?;
            self.guarded_editor_key(self.enter.clone(), lease, true, |screen| {
                self.inspect.editor_frame_unchanged(&verified, screen)
            })?;
            self.wait_transfer(|| {
                if let Some(screen) = self.terminal.visible_ansi_if_complete() {
                    if screen != verified
                        && (self.inspect.observe(&screen) == InputObservation::Empty
                            || self.inspect.editor_working(&screen))
                    {
                        return Ok(true);
                    }
                }
                if self.check_deadline().is_err() {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        editor.verification_error.unwrap_or(
                            "Provider submission may have run; inspect the transcript before resending",
                        ),
                    ));
                }
                Ok(false)
            })?;
            lease.restored();
            if let Err(error) = exchange.remove_recovery() {
                self.recovery_log(&exchange, &error);
            }
            Ok(())
        })();
        if let Err(error) = &transfer {
            self.recovery_log(&exchange, error);
        }
        transfer
    }

    fn remove_marker(&self, exchange: &Exchange, backspace: &Bytes) -> io::Result<()> {
        self.wait_transfer(|| {
            Ok(self
                .inspect
                .restoration_marker_visible(&self.terminal.visible_ansi(), exchange.marker()))
        })?;
        let mut keys = Vec::with_capacity(backspace.len() * exchange.marker().len());
        for _ in exchange.marker().bytes() {
            keys.extend_from_slice(backspace);
        }
        self.transfer_key(Bytes::from(keys))?;
        self.wait_transfer(|| {
            Ok(!self
                .inspect
                .restoration_marker_visible(&self.terminal.visible_ansi(), exchange.marker()))
        })
    }

    fn restore_editor(
        &self,
        exchange: &Exchange,
        key: &Bytes,
        backspace: &Bytes,
    ) -> io::Result<()> {
        // A failed Bus withdrawal can leave an owned or edited prompt behind.
        // Retain the recovery file instead of replacing or merging that input.
        if self.observe("editor_restore_preflight") != InputObservation::Empty {
            return Err(io::Error::other(
                "Composer changed; saved draft was not merged",
            ));
        }
        exchange.request_restore()?;
        self.editor_handoff(exchange, key)?;
        self.remove_marker(exchange, backspace)?;
        // A native editor round-trip is the authoritative exact read, including
        // offscreen text, tabs and trailing whitespace that a screen cannot show.
        exchange.request_verify()?;
        self.editor_handoff(exchange, key)?;
        self.remove_marker(exchange, backspace)?;
        exchange.remove_recovery()
    }

    fn preserve_editor(
        &self,
        text: Bytes,
        delay: Duration,
        key: Bytes,
        backspace: Bytes,
        directory: PathBuf,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        let exchange = self.exchange(directory).inspect_err(|_| lease.restored())?;
        exchange
            .request_capture()
            .inspect_err(|_| lease.restored())?;
        let captured = self.editor_handoff(&exchange, &key).and_then(|()| {
            exchange.captured_text()?;
            self.wait_transfer(|| Ok(self.observe("await_editor_clear") == InputObservation::Empty))
        });
        if let Err(error) = captured {
            self.recovery_log(&exchange, &error);
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        self.reset_confirmation(delay);
        let result = self.run(text, delay);
        match self.restore_editor(&exchange, &key, &backspace) {
            Ok(()) => lease.restored(),
            Err(error) => self.recovery_log(&exchange, &error),
        }
        result
    }

    fn recovery_log(&self, exchange: &Exchange, error: &io::Error) {
        tracing::error!(event = "bus.draft.recovery_required", recovery = %exchange.recovery().display(), directory = %exchange.path().display(), %error, "Draft restoration was not proven; recoverable draft retained");
    }
}
