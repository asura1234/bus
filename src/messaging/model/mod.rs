//! Messaging model facade; state operations are owned by their round-trip domain.
mod agents;
mod attention;
mod callbacks;
mod compactions;
mod developer_notifications;
mod disk_space;
mod history;
mod idle_agents;
mod interrupts;
mod payload;
mod recovery;
mod reply_routing;
mod requests;
mod rooms;
mod settlement;
mod staffing;
mod state;
mod status;
mod types;
mod worker_events;
mod worker_notifications;
#[cfg(test)]
use requests::rejection_reason;

pub(crate) use agents::AGENT_DELETED;
pub(crate) use attention::{AttentionDelivery, AttentionOrigin};
pub(crate) use developer_notifications::new_developer_notifications;
#[cfg(test)]
pub(crate) use history::ARCHIVE_AFTER_MS;
pub(crate) use history::{Archived, History, DIALOG_ANSWER_TTL_MS, HISTORY_RETENTION_MS};
#[cfg(test)]
pub(crate) use idle_agents::MIN_IDLE_FOR_ALERT_MS;
pub(crate) use interrupts::INTERRUPTED;
pub(crate) use settlement::RecordedTurnEnd;
pub(crate) use state::{fingerprint_digest, BusState};
#[cfg(test)]
use types::payload_matches;
pub(crate) use {
    crate::messaging::model::types::RoomAgent,
    types::{
        is_developer_recipient, AgentId, AgentRecipients, AgentRuntimeIdentity, AgentStartup,
        Author, CallbackDisposition, CallbackEventKind, CallbackRejection, Compactions,
        DialogNoticeTo, Draft, ExitSnapshot, ModelError, PendingFinal, Prompt, PromptId, Provider,
        ProviderCallback, Reply, Request, RequestId, RequestPhase, Room, RoomId, RoomKind,
        RuntimeStatus, SubmissionOutcome, BLOCKED_STALL_MS, BUS_SENDER, DELIVERED_STALL_MS,
        HELD_REPLY_SETTLE_MS, MASTER_ROOM_NAME, QUEUED_STALL_MS, STEERING_SETTLE_MS,
        TURN_SETTLE_MS, UNBOUND_SETTLE_MS,
    },
};

#[cfg(test)]
#[path = "tests/types_test.rs"]
mod tests;
