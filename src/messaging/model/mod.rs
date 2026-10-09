//! Messaging model facade; state operations are owned by their round-trip domain.
mod agents;
mod callbacks;
mod requests;
mod rooms;
mod state;
mod status;
mod types;
#[cfg(test)]
use requests::rejection_reason;

pub(crate) use state::BusState;
#[cfg(test)]
use types::payload_matches;
pub(crate) use {
    crate::messaging::model::types::RoomAgent,
    types::{
        AgentId, AgentRecipients, AgentRuntimeIdentity, Author, CallbackDisposition,
        CallbackEventKind, CallbackRejection, Compactions, Draft, ModelError, PendingFinal, Prompt,
        PromptId, Provider, ProviderCallback, Reply, Request, RequestId, RequestPhase, Room,
        RoomId, RoomKind, RuntimeStatus, SubmissionOutcome, BLOCKED_STALL_MS, DELIVERED_STALL_MS,
        HUMAN_RECIPIENT, MASTER_ROOM_NAME, QUEUED_STALL_MS, STEERING_SETTLE_MS, UNBOUND_SETTLE_MS,
    },
};

#[cfg(test)]
#[path = "tests/types_test.rs"]
mod tests;
