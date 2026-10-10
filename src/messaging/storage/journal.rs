//! A small write-ahead journal beside `state.json` for the composer's hot
//! commands.
//!
//! A full save re-reads, serializes and fsyncs the whole state. Once a session
//! holds a long history that costs hundreds of milliseconds in a debug build,
//! and doing it for every draft keystroke and every send made Enter wait on
//! several saves in a row. Draft edits and draft submissions are instead
//! recorded here, durably, before they are acknowledged. `state.json` stores the
//! journal sequence it already contains, so loading replays only newer entries
//! and every full save folds them in.
use serde::{Deserialize, Serialize};

use crate::messaging::model::{BusState, ModelError, RoomId};

const JOURNAL_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum JournalOp {
    DraftText {
        room: RoomId,
        text: String,
    },
    /// Sends the room's draft as it stands at this point in the journal.
    Submit {
        room: RoomId,
        queued: bool,
        now_ms: u64,
    },
}

impl JournalOp {
    fn room(&self) -> RoomId {
        match self {
            Self::DraftText { room, .. } | Self::Submit { room, .. } => *room,
        }
    }

    pub(crate) fn apply(&self, state: &mut BusState) -> Result<(), ModelError> {
        match self {
            Self::DraftText { room, text } => state.set_draft_text(*room, text),
            Self::Submit {
                room,
                queued: true,
                now_ms,
            } => state.submit_draft_queued(*room, *now_ms).map(drop),
            Self::Submit {
                room,
                queued: false,
                now_ms,
            } => state.submit_draft(*room, *now_ms).map(drop),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct JournalEntry {
    pub(super) seq: u64,
    pub(super) op: JournalOp,
}

#[derive(Serialize, Deserialize)]
pub(super) struct JournalDocument {
    pub(super) version: u64,
    pub(super) entries: Vec<JournalEntry>,
}

impl JournalDocument {
    pub(super) fn new(entries: Vec<JournalEntry>) -> Self {
        Self {
            version: JOURNAL_VERSION,
            entries,
        }
    }

    pub(super) fn supported(&self) -> bool {
        self.version == JOURNAL_VERSION
    }
}

/// Keeps only the newest draft per room since that room's last submission:
/// an older draft can never be the one a later replay sends.
pub(super) fn push_coalesced(entries: &mut Vec<JournalEntry>, entry: JournalEntry) {
    if let JournalOp::DraftText { room, .. } = &entry.op {
        let previous = entries.iter().rposition(|old| old.op.room() == *room);
        if let Some(index) =
            previous.filter(|&index| matches!(entries[index].op, JournalOp::DraftText { .. }))
        {
            entries.remove(index);
        }
    }
    entries.push(entry);
}

/// Replays entries newer than `base_seq` onto the state they were recorded
/// after. An entry the state no longer accepts (its room was deleted by a later
/// full save) cannot be newer than that save, so it is skipped, not fatal.
pub(super) fn replay(state: &mut BusState, entries: &[JournalEntry], base_seq: u64) {
    for entry in entries.iter().filter(|entry| entry.seq > base_seq) {
        if let Err(error) = entry.op.apply(state) {
            tracing::warn!(
                event = "bus.storage.journal_skipped",
                seq = entry.seq,
                %error,
                "Journal entry no longer applies"
            );
        }
    }
}
