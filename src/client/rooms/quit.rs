//! Quit confirmation timing and retry state.
use super::{storage_paused, BusUi, Effect};
use std::time::Instant;

const QUIT_SAVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const QUIT_FORCE_WINDOW: std::time::Duration = std::time::Duration::from_secs(5);
const QUIT_UNSAVED_NOTICE: &str = "Unsaved changes or slow storage. Press Ctrl+Q again within 5s to force quit; unsaved edits may be lost. Wait 5s, then Ctrl+Q retries saving.";
const QUIT_RETRY_NOTICE: &str = "Unsaved changes or slow storage. Ctrl+Q retries saving.";

impl BusUi {
    pub(super) fn quit_retry_pending(&self) -> bool {
        self.error.as_deref() == Some(QUIT_RETRY_NOTICE)
    }

    pub(super) fn request_quit(&mut self) {
        // Repeated presses while saving must not postpone the warning.
        if self.quitting.is_some() {
            return;
        }
        tracing::info!(
            event = "bus.quit.requested",
            pending_count = self.pending.len(),
            failed_count = self.failed.len(),
            last_command_id = self.snapshot.last_command_id,
            "Save and quit requested"
        );
        self.force_exit_until = None;
        if matches!(
            self.error.as_deref(),
            Some(QUIT_UNSAVED_NOTICE | QUIT_RETRY_NOTICE)
        ) {
            self.error = None;
        }
        self.quitting = Some(std::time::Instant::now());
        let failed = std::mem::take(&mut self.failed);
        for pending in failed {
            match pending.effect {
                Effect::Text(room, _) => self.text_changed(room),
                Effect::Notes(room, _) => self.notes_changed(room),
                Effect::Recipients(room, _) => self.recipients_changed(room),
                _ => {
                    self.queue(pending.command, pending.effect);
                }
            }
        }
    }

    pub(super) fn tick_quit(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.force_exit_until.is_some_and(|until| now >= until) {
            self.force_exit_until = None;
            if self.error.as_deref() == Some(QUIT_UNSAVED_NOTICE) {
                self.error = Some(QUIT_RETRY_NOTICE.into());
            }
            changed = true;
        }
        if let Some(started) = self
            .quitting
            .filter(|started| {
                now.duration_since(*started) > QUIT_SAVE_TIMEOUT || !self.failed.is_empty()
            })
            .filter(|_| !self.exit_ready)
        {
            let pending = self.pending.front();
            let failed = self.failed.first();
            tracing::warn!(
                event = "bus.quit.warning",
                reason = if self.failed.is_empty() { "save_timeout" } else { "failed_command" },
                elapsed_ms = now.duration_since(started).as_millis() as u64,
                pending_count = self.pending.len(),
                failed_count = self.failed.len(),
                pending_command_id = ?pending.map(|pending| pending.id),
                pending_command = ?pending.map(|pending| crate::messaging::perf::command_name(&pending.command)),
                pending_enqueued = pending.is_some_and(|pending| pending.enqueued),
                pending_result = match pending.and_then(|pending| pending.result.as_ref()) {
                    None => "waiting",
                    Some(Ok(())) => "ok",
                    Some(Err(_)) => "error",
                },
                pending_snapshot_ack = pending.is_some_and(|pending| self.snapshot.last_command_id >= pending.id),
                failed_command_id = ?failed.map(|pending| pending.id),
                failed_command = ?failed.map(|pending| crate::messaging::perf::command_name(&pending.command)),
                storage_paused = self.failed.iter().chain(self.pending.iter()).any(storage_paused),
                last_command_id = self.snapshot.last_command_id,
                snapshot_error_present = self.snapshot.error.is_some(),
                "Quit waiting for saves; force-exit confirmation available for 5s"
            );
            self.quitting = None;
            self.force_exit_until = Some(now + QUIT_FORCE_WINDOW);
            self.error = Some(QUIT_UNSAVED_NOTICE.into());
            changed = true;
        }
        changed
    }
}
