//! Transfer a developer draft only while pane input is exclusively reserved.
use super::{InputObservation, ObservedSubmission, PromptNotShown};
use crate::platform::draft_editor::Exchange;
use crate::protocol::api::submission::{confirmation_budget, CLEAR_KEY_DELAY, WITHDRAW_GRACE};
use bytes::Bytes;
use std::{io, path::PathBuf, time::Duration};

const FALLBACK_HANDOFF_BUDGET: Duration = Duration::from_millis(500);

pub(crate) enum DraftPreservation {
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
    Copy {
        snapshot: String,
        restore_bytes: Option<Bytes>,
    },
}

pub(crate) struct EditorSubmission {
    pub(crate) text: String,
    pub(crate) directory: PathBuf,
    pub(crate) key: Bytes,
    pub(crate) backspace: Bytes,
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
        let screen = self.terminal.active_ansi();
        let cursor = self
            .terminal
            .cursor_state()
            .map(|cursor| (cursor.x, cursor.y));
        let pristine = self
            .startup_suggestion_open
            .load(std::sync::atomic::Ordering::Acquire);
        if self.provider_queues_input(&screen) {
            lease.restored();
            return Err(io::Error::other(PromptNotShown::ClaudeTurnBusy));
        }
        // A Codex picker can coexist with a complete developer draft. Refuse
        // before reserving its durable editor exchange: no handoff can run
        // while that picker owns the keys.
        if self.inspect.confirm_consumption() && !self.inspect.submission_available(&screen) {
            lease.restored();
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        if self.inspect.submission_available(&screen) {
            if let Some(editor) = self
                .inspect
                .editor_submission()
                .filter(|editor| Exchange::has_preserved_draft(&editor.directory))
            {
                return self.finish_preserved_draft(editor, delay, lease);
            }
        }
        let mut initial = initial;
        let mut screen = screen;
        let mut cursor = cursor;
        if matches!(initial, InputObservation::Other | InputObservation::Pending)
            && self.inspect.observe_aborted_payload(
                &screen,
                cursor,
                pristine,
                lease.claude_stopped_without_edits(),
            ) == InputObservation::Pending
        {
            if let Err(error) = self.discard_aborted_payload(pristine, lease) {
                lease.restored();
                return Err(error);
            }
            self.inspect.mark_aborted_payload_discarded();
            self.reset_confirmation(delay);
            initial = InputObservation::Empty;
            screen = self.terminal.active_ansi();
            cursor = self
                .terminal
                .cursor_state()
                .map(|cursor| (cursor.x, cursor.y));
        }
        let preservation = self
            .inspect
            .draft_preservation_since_launch(&screen, cursor, pristine);
        let withdrawal_lease =
            (self.inspect.confirm_consumption() || self.inspect.waits_for_idle()).then_some(lease);
        match initial {
            InputObservation::Empty if preservation.is_none() => {
                if self.inspect.always_use_editor() {
                    if let Some(editor) = self.inspect.editor_submission() {
                        return self.preserve_editor(
                            text,
                            delay,
                            editor.key,
                            editor.backspace,
                            editor.directory,
                            lease,
                        );
                    }
                }
                let existing_stash = self.inspect.native_stash_visible(&screen);
                let result =
                    self.confirm_legacy_submission(self.run(text, delay, withdrawal_lease), lease);
                if result.is_ok() {
                    // A previously stashed Claude draft pops after submit.
                    // Its receipt, rather than the Enter write, releases user keys.
                    if existing_stash {
                        self.wait_transfer(|| {
                            Ok(self
                                .inspect
                                .native_draft_restored(&self.terminal.active_ansi()))
                        })?;
                    } else if self.inspect.waits_for_idle() {
                        lease.record_claude_submission();
                    }
                    lease.restored();
                }
                return result;
            }
            InputObservation::Empty | InputObservation::Other | InputObservation::Pending => {}
            _ if preservation.is_none() || !self.inspect.submission_available(&screen) => {
                lease.restored();
                return self
                    .confirm_legacy_submission(self.run(text, delay, withdrawal_lease), lease);
            }
            _ => {}
        }
        let Some(preservation) = preservation else {
            lease.restored();
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        };
        // The lease buffers incoming developer keys. Each native editing key
        // checks the current blocker independently; transcript animation need
        // not keep the initial screen byte-identical.
        match preservation {
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
            DraftPreservation::Copy {
                snapshot,
                restore_bytes,
            } => self.preserve_copy(snapshot, restore_bytes, text, delay, &screen, lease),
        }
    }

    fn finish_preserved_draft(
        &self,
        editor: EditorSubmission,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        match self.observe("preserved_draft") {
            // The Bus payload is already on screen. One Enter submits
            // that attempt; opening the editor would replace it.
            InputObservation::Pending => {
                if self.inspect.confirm_consumption() {
                    let exchange = Exchange::begin_restoration(
                        editor.directory.clone(),
                        self.clock.remaining(self.hard_deadline),
                    )
                    .map_err(|_| io::Error::other(PromptNotShown::ComposerBusy))?;
                    return self.submit_editor_attempt(&exchange, editor, delay, lease);
                }
                let submitted = self.swap_key(lease, self.enter.clone(), true);
                lease.restored();
                return submitted;
            }
            InputObservation::Empty => {}
            // Developer text after the interrupted swap stays put.
            _ => {
                lease.restored();
                return Err(io::Error::other(PromptNotShown::ComposerBusy));
            }
        }
        let exchange = Exchange::begin_restoration(
            editor.directory.clone(),
            self.clock.remaining(self.hard_deadline),
        )
        .map_err(|_| io::Error::other(PromptNotShown::ComposerBusy))?;
        self.restore_editor_transfer(&exchange, &editor.key, &editor.backspace, true, lease)
            .map_err(|_| io::Error::other(PromptNotShown::ComposerBusy))?;
        lease.restored();
        Err(io::Error::other(PromptNotShown::ComposerBusy))
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

    pub(super) fn wait_transfer_until(
        &self,
        deadline: std::time::Instant,
        mut ready: impl FnMut() -> io::Result<bool>,
    ) -> io::Result<()> {
        let limit = deadline.min(self.hard_deadline);
        loop {
            let generation = self.clock.generation();
            if ready()? {
                return Ok(());
            }
            if self.clock.now() >= limit {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Provider did not finish draft transfer",
                ));
            }
            self.clock.sleep(Duration::from_millis(20), generation);
        }
    }

    fn wait_transfer(&self, ready: impl FnMut() -> io::Result<bool>) -> io::Result<()> {
        self.wait_transfer_until(self.hard_deadline, ready)
    }

    pub(super) fn reset_confirmation(&self, delay: Duration) {
        self.deadline.set(
            (self.clock.now() + confirmation_budget(delay))
                .min(self.hard_deadline - self.withdraw_grace),
        );
    }

    /// Clear an interrupted Bus request's payload that the provider restored
    /// into the composer, stopping immediately if a developer edit appears.
    fn discard_aborted_payload(
        &self,
        pristine: bool,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        if self.clear.is_empty() {
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        let mut progress_rev = self.inspect.aborted_cleanup_progress();
        let mut expires = self.clock.now() + WITHDRAW_GRACE;
        loop {
            for (index, key) in self.clear.iter().enumerate() {
                let generation = self.clock.generation();
                let guard = self
                    .content_lock
                    .lock()
                    .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
                let Some(screen) = self.terminal.active_ansi_if_complete() else {
                    drop(guard);
                    if self.clock.now() >= expires || self.check_deadline().is_err() {
                        return Err(io::Error::other(PromptNotShown::ComposerBusy));
                    }
                    self.clock.sleep(CLEAR_KEY_DELAY, generation);
                    break;
                };
                let cursor = self
                    .terminal
                    .cursor_state()
                    .map(|cursor| (cursor.x, cursor.y));
                match self.inspect.observe_aborted_payload(
                    &screen,
                    cursor,
                    pristine,
                    lease.claude_stopped_without_edits(),
                ) {
                    InputObservation::Empty => return Ok(()),
                    InputObservation::Pending => {}
                    _ => return Err(io::Error::other(PromptNotShown::ComposerBusy)),
                }
                let revision = self.inspect.aborted_cleanup_progress();
                if progress_rev != revision {
                    progress_rev = revision;
                    expires = self.clock.now() + WITHDRAW_GRACE;
                }
                if self.clock.now() >= expires || self.check_deadline().is_err() {
                    return Err(io::Error::other(PromptNotShown::ComposerBusy));
                }
                let write_deadline = self.hard_deadline.min(expires);
                tracing::debug!(
                    target: "bus::terminal::submission",
                    event = "bus.terminal.submission.discard_aborted",
                    key = ?key,
                );
                let completion = lease.with_unedited(|| {
                    self.io.queue_input_write_scoped(
                        key.clone(),
                        Bytes::new(),
                        Duration::ZERO,
                        Some(write_deadline),
                        &self.scope,
                    )
                })?;
                self.inspect.clearing_aborted(index);
                drop(guard);
                completion
                    .recv_timeout(self.clock.remaining(write_deadline))
                    .map_err(|error| io::Error::new(io::ErrorKind::TimedOut, error))??;
                self.clock.sleep(CLEAR_KEY_DELAY, generation);
            }
        }
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
        let exchange = match self.exchange(directory) {
            Ok(exchange) => exchange,
            Err(error) => {
                if let Some(fallback) = self.fallback_copy_after_handoff_failure(text, delay, lease)
                {
                    return fallback;
                }
                lease.restored();
                return Err(error);
            }
        };
        if let Err(error) = exchange.save_recovery(&visible) {
            drop(exchange);
            if let Some(fallback) = self.fallback_copy_after_handoff_failure(text, delay, lease) {
                return fallback;
            }
            lease.restored();
            tracing::warn!(event = "bus.draft.copy_loss", %error, "Could not save native recovery backup");
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        if let Err(error) = self.swap_key(lease, key.clone(), false) {
            let _ = exchange.remove_recovery();
            let path = exchange.path().to_owned();
            drop(exchange);
            if let Some(fallback) = self.fallback_copy_after_handoff_failure(text, delay, lease) {
                return fallback;
            }
            tracing::error!(event = "bus.draft.recovery_required", directory = %path.display(), %error, "Draft restoration was not proven; recoverable draft retained");
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        if let Err(error) =
            self.wait_transfer_until(self.clock.now() + FALLBACK_HANDOFF_BUDGET, || {
                Ok(
                    self.observe("await_native_stash") == InputObservation::Empty
                        && self
                            .inspect
                            .native_stash_visible(&self.terminal.active_ansi()),
                )
            })
        {
            let _ = exchange.remove_recovery();
            let path = exchange.path().to_owned();
            drop(exchange);
            if let Some(fallback) = self.fallback_copy_after_handoff_failure(text, delay, lease) {
                return fallback;
            }
            tracing::error!(event = "bus.draft.recovery_required", directory = %path.display(), %error, "Draft restoration was not proven; recoverable draft retained");
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        self.reset_confirmation(delay);
        let result = self.run(text, delay, self.inspect.waits_for_idle().then_some(lease));
        if result.is_ok() {
            if let Err(error) = self.wait_transfer(|| {
                Ok(self
                    .inspect
                    .native_draft_restored(&self.terminal.active_ansi()))
            }) {
                self.recovery_log(&exchange, &error);
                return Err(error);
            }
        }
        // A flushed paste may still materialize after a pre-Enter readiness
        // timeout. Empty is not proof of withdrawal: keep the native stash and
        // recovery record instead of restoring into that unresolved write.
        if self
            .diagnostics
            .pending_paste(&result)
            .is_some_and(|(_, entered)| !entered)
            && !self
                .inspect
                .native_draft_restored(&self.terminal.active_ansi())
        {
            self.recovery_log(
                &exchange,
                &io::Error::other("Native stash retained while a flushed paste is unresolved"),
            );
            return result;
        }
        // Native stash pops at the next submit. On an unsubmitted attempt,
        // restore only into an empty composer; never toggle over another draft.
        if !self
            .inspect
            .native_draft_restored(&self.terminal.active_ansi())
            && self.observe("native_restore_preflight") == InputObservation::Empty
        {
            let restored = self.transfer_key(key).and_then(|()| {
                self.wait_transfer(|| {
                    Ok(self
                        .inspect
                        .native_draft_restored(&self.terminal.active_ansi()))
                })
            });
            if let Err(error) = restored {
                self.recovery_log(&exchange, &error);
                return result;
            }
        }
        if self
            .inspect
            .native_draft_restored(&self.terminal.active_ansi())
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

    fn editor_handoff(
        &self,
        exchange: &Exchange,
        key: &Bytes,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        self.swap_key(lease, key.clone(), false)?;
        self.wait_transfer(|| exchange.acknowledged())
    }

    fn swap_key(
        &self,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
        key: Bytes,
        submit: bool,
    ) -> io::Result<()> {
        let deadline = if submit {
            self.deadline.get()
        } else {
            self.hard_deadline
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
                .ok_or_else(|| io::Error::other("Native input is blocked"))?;
            if self.clock.now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Draft transfer expired",
                ));
            }
            let key = if submit {
                self.inspect.editor_submit_key(&screen, &key)
            } else {
                key
            };
            if submit {
                if self.inspect.confirm_consumption() {
                    self.observe("legacy_editor_enter_ownership");
                    self.enter_content_seq
                        .set(self.content_seq.load(std::sync::atomic::Ordering::Acquire));
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

    fn remove_marker(
        &self,
        exchange: &Exchange,
        backspace: &Bytes,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        self.wait_transfer(|| {
            Ok(self
                .inspect
                .restoration_marker_visible(&self.terminal.active_ansi(), exchange.marker()))
        })?;
        self.close_marker_popup(lease, exchange.marker())?;
        let mut keys = Vec::with_capacity(backspace.len() * exchange.marker().len());
        for _ in exchange.marker().bytes() {
            keys.extend_from_slice(backspace);
        }
        self.swap_key(lease, Bytes::from(keys), false)?;
        self.wait_transfer(|| {
            Ok(self
                .terminal
                .active_ansi_if_complete()
                .is_some_and(|screen| {
                    !self
                        .inspect
                        .restoration_marker_visible(&screen, exchange.marker())
                }))
        })?;
        self.close_deleted_marker_popup(lease, exchange.marker())?;
        self.wait_transfer(|| {
            let screen = self.terminal.active_ansi();
            Ok(self.inspect.submission_available(&screen)
                && !self
                    .inspect
                    .restoration_marker_visible(&screen, exchange.marker()))
        })
    }

    fn restore_editor(
        &self,
        exchange: &Exchange,
        key: &Bytes,
        backspace: &Bytes,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        self.restore_editor_transfer(exchange, key, backspace, false, lease)
    }

    fn restore_editor_transfer(
        &self,
        exchange: &Exchange,
        key: &Bytes,
        backspace: &Bytes,
        requested: bool,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        // A failed Bus withdrawal can leave an owned or edited prompt behind.
        // Retain the recovery file instead of replacing or merging that input.
        if !requested && self.observe("editor_restore_preflight") != InputObservation::Empty {
            return Err(io::Error::other(
                "Composer changed; saved draft was not merged",
            ));
        }
        if !requested {
            exchange.request_restore()?;
        }
        self.editor_handoff(exchange, key, lease)?;
        self.remove_marker(exchange, backspace, lease)?;
        // A native editor round-trip is the authoritative exact read, including
        // offscreen text, tabs and trailing whitespace that a screen cannot show.
        exchange.request_verify()?;
        self.editor_handoff(exchange, key, lease)?;
        self.remove_marker(exchange, backspace, lease)?;
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
        let editor = self
            .inspect
            .editor_submission()
            .ok_or_else(|| io::Error::other("Native editor input is unavailable"))?;
        let exchange = match Exchange::begin_preserving(
            directory.clone(),
            self.clock.remaining(self.hard_deadline),
        ) {
            Ok(exchange) => exchange,
            Err(error) => {
                if !self.inspect.always_use_editor() {
                    if let Some(fallback) =
                        self.fallback_copy_after_handoff_failure(text, delay, lease)
                    {
                        return fallback;
                    }
                }
                lease.restored();
                tracing::warn!(event = "bus.draft.exchange_wait", directory = %directory.display(), %error, "Draft transfer is unavailable; delivery remains queued");
                return Err(io::Error::other(PromptNotShown::ComposerBusy));
            }
        };
        if let Err(error) = exchange.request_swap(&editor.text) {
            drop(exchange);
            if !self.inspect.always_use_editor() {
                if let Some(fallback) = self.fallback_copy_after_handoff_failure(text, delay, lease)
                {
                    return fallback;
                }
            }
            lease.restored();
            return Err(error);
        }
        let handoff_deadline = if self.inspect.always_use_editor() {
            self.hard_deadline
        } else {
            self.clock.now() + FALLBACK_HANDOFF_BUDGET
        };
        let captured = self
            .swap_key(lease, key.clone(), false)
            .and_then(|()| self.wait_transfer_until(handoff_deadline, || exchange.acknowledged()))
            .and_then(|()| self.remove_marker(&exchange, &backspace, lease));
        if let Err(error) = captured {
            let has_preserved = Exchange::has_preserved_draft(exchange.path());
            if !self.inspect.always_use_editor() && !has_preserved {
                let _ = exchange.remove_recovery();
                let path = exchange.path().to_owned();
                drop(exchange);
                if let Some(fallback) = self.fallback_copy_after_handoff_failure(text, delay, lease)
                {
                    return fallback;
                }
                lease.restored();
                tracing::error!(event = "bus.draft.recovery_required", directory = %path.display(), %error, "Draft restoration was not proven; recoverable draft retained");
                return Err(io::Error::other(PromptNotShown::ComposerBusy));
            }
            if !has_preserved {
                lease.restored();
            }
            self.recovery_log(&exchange, &error);
            return Err(io::Error::other(PromptNotShown::ComposerBusy));
        }
        self.submit_editor_attempt(
            &exchange,
            EditorSubmission {
                key,
                backspace,
                ..editor
            },
            delay,
            lease,
        )
    }

    fn submit_editor_attempt(
        &self,
        exchange: &Exchange,
        editor: EditorSubmission,
        delay: Duration,
        lease: &crate::terminal::runtime::draft_input::DraftInputLease,
    ) -> io::Result<()> {
        // This receipt records installed composer input, not a native turn.
        // Keep its recovery ownership until exact restoration is proven;
        // model ownership still requires the native callback.
        self.diagnostics.editor_installed(editor.text.len());
        if self.inspect.confirm_consumption() {
            self.diagnostics.retain_editor_input(self.clock.now());
        }
        self.reset_confirmation(delay);
        let submitted = self.swap_key(lease, self.enter.clone(), true);
        if submitted.is_ok() {
            self.diagnostics.ownership_verified();
        }
        let result = submitted.and_then(|()| {
            if self.inspect.confirm_consumption() {
                self.wait_legacy_consumption()
            } else {
                self.wait_for_consumption()
            }
        });
        let restoration = if result.is_err() {
            exchange
                .request_restore_after_attempt(&editor.text)
                .and_then(|()| {
                    self.restore_editor_transfer(
                        exchange,
                        &editor.key,
                        &editor.backspace,
                        true,
                        lease,
                    )
                })
        } else {
            self.restore_editor(exchange, &editor.key, &editor.backspace, lease)
        };
        match restoration {
            Ok(()) => {
                lease.restored();
                self.diagnostics.editor_restored();
            }
            Err(error) => {
                self.recovery_log(exchange, &error);
                // Enter already cleared the composer. Reporting the later
                // restore failure would invite a second submit; the recovery
                // file stays for a later empty-composer restoration.
                if result.is_err() {
                    return Err(
                        if self.inspect.confirm_consumption() && self.diagnostics.enter_attempted()
                        {
                            Self::uncertain_submission(error)
                        } else {
                            error
                        },
                    );
                }
            }
        }
        result
    }

    fn recovery_log(&self, exchange: &Exchange, error: &io::Error) {
        tracing::error!(event = "bus.draft.recovery_required", recovery = %exchange.recovery().display(), directory = %exchange.path().display(), %error, "Draft restoration was not proven; recoverable draft retained");
    }
}
