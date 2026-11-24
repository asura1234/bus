//! Repair a previously returned draft while preserving unowned composer input.
use super::{
    copy::RESTORE_BUDGET, swap_key::SwapKey, EditorSubmission, ObservedSubmission, PromptNotShown,
};
use crate::{
    platform::draft_editor::TRANSFER_MARKER_PREFIX, terminal::runtime::draft_input::DraftInputLease,
};
use bytes::Bytes;
use std::io;

impl ObservedSubmission {
    pub(super) fn repair_marked_draft(
        &self,
        editor: &EditorSubmission,
        lease: &DraftInputLease,
    ) -> io::Result<()> {
        if !self.terminal.active_ansi().contains(TRANSFER_MARKER_PREFIX) {
            return Ok(());
        }
        let limit = (self.clock.now() + RESTORE_BUDGET).min(self.hard_deadline);
        let saved = self.open_marked_draft(&editor.directory, limit)?;
        let mut keys = Vec::with_capacity(saved.marker_len() * editor.backspace.len());
        for _ in 0..saved.marker_len() {
            keys.extend_from_slice(&editor.backspace);
        }
        let key = SwapKey::EraseMarker {
            keys: Bytes::from(keys),
            marked: saved.marked(),
            deadline: limit,
        };
        match self.swap_key(lease, key) {
            Err(error) if PromptNotShown::composer_busy(&error) => return Ok(()),
            result => result?,
        }
        self.wait_transfer_until(limit, || {
            let _guard = self
                .content_lock
                .lock()
                .map_err(|_| io::Error::other("terminal content lock poisoned"))?;
            Ok(self
                .terminal
                .active_ansi_if_complete()
                .is_some_and(|screen| self.matches_exact_editor_line(&screen, saved.original())))
        })?;
        saved.retire()
    }

    pub(super) fn matches_exact_editor_line(&self, screen: &str, text: &str) -> bool {
        self.terminal
            .active_literal_line()
            .is_some_and(|(row, literal)| {
                self.inspect
                    .matches_exact_editor_line(screen, row, &literal, text)
            })
    }
}
