//! Messaging model facade; state operations are owned by their round-trip domain.
mod agents;
mod callbacks;
mod compactions;
mod history;
mod interrupts;
mod recovery;
mod requests;
mod rooms;
mod settlement;
mod state;
mod status;
mod types;
#[cfg(test)]
use requests::rejection_reason;

#[cfg(test)]
pub(crate) use history::ARCHIVE_AFTER_MS;
pub(crate) use history::{Archived, History, DIALOG_ANSWER_TTL_MS, HISTORY_RETENTION_MS};
pub(crate) use interrupts::INTERRUPTED;
pub(crate) use state::{fingerprint_digest, BusState};
#[cfg(test)]
use types::payload_matches;
pub(crate) use {
    crate::messaging::model::types::RoomAgent,
    types::{
        AgentId, AgentRecipients, AgentRuntimeIdentity, AgentStartup, Author, CallbackDisposition,
        CallbackEventKind, CallbackRejection, Compactions, Draft, ModelError, PendingFinal, Prompt,
        PromptId, Provider, ProviderCallback, Reply, Request, RequestId, RequestPhase, Room,
        RoomId, RoomKind, RuntimeStatus, SubmissionOutcome, BLOCKED_STALL_MS, DELIVERED_STALL_MS,
        DEVELOPER_RECIPIENT, HELD_REPLY_SETTLE_MS, MASTER_ROOM_NAME, QUEUED_STALL_MS,
        STEERING_SETTLE_MS, TURN_SETTLE_MS, UNBOUND_SETTLE_MS,
    },
};

#[cfg(test)]
#[path = "tests/types_test.rs"]
mod tests;
