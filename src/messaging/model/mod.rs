//! Messaging model facade; state operations are owned by their round-trip domain.
mod agent_names;
mod agents;
mod attention;
mod callbacks;
mod compactions;
mod developer_notifications;
mod disk_space;
mod error;
mod error_reports;
pub(crate) mod feishu;
mod history;
mod history_index;
mod idle_agents;
mod interrupts;
mod latency;
mod launch_attempts;
mod payload;
mod provider_failure;
mod provider_retry;
mod request_map;
use provider_retry::ProviderRetry;
mod recipients;
mod recovery;
mod replies;
mod reply_routing;
mod request_outcomes;
mod requests;
mod restart;
mod restart_continuation;
mod rooms;
mod settlement;
mod staffing;
mod stall;
mod state;
mod status;
mod types;
mod worker_events;
mod worker_notifications;
mod worker_result_scope;
#[cfg(test)]
use requests::rejection_reason;

pub(crate) use attention::{AttentionDelivery, AttentionOrigin};
pub(crate) use developer_notifications::{developer_alerts, new_developer_notifications};
pub(crate) use error::{
    is_recipient_deleted_refusal, recipient_deleted_message, ModelError, RECIPIENT_DELETED_REFUSAL,
};
pub(crate) use history::{Archived, History};
#[cfg(test)]
pub(crate) use history::{ARCHIVE_AFTER_MS, HISTORY_RETENTION_MS};
#[cfg(test)]
pub(crate) use idle_agents::MIN_IDLE_FOR_ALERT_MS;
pub(crate) use interrupts::INTERRUPTED;
pub(crate) use launch_attempts::{LaunchAttempt, LaunchKind, LaunchOutcome};
#[cfg(test)]
use payload::payload_matches;
pub(crate) use recovery::ManualRecovery;
pub(crate) use request_outcomes::{
    classify_request_failure, request_pre_start_failure, request_start_bound, RequestFailureClass,
};
pub(crate) use settlement::RecordedTurnEnd;
pub(crate) use state::BusState;

#[cfg(test)]
pub(crate) use latency::DispatchTimestamps;
pub(crate) use {
    crate::messaging::model::{
        latency::{
            DeliveryLatency, DialogNoticeDelivery, DispatchMark, NativeSubmissionTimestamps,
            QueuedRejection,
        },
        types::RoomAgent,
    },
    types::{
        is_developer_recipient, AgentId, AgentRecipients, AgentRuntimeIdentity, AgentStartup,
        Author, CallbackDisposition, CallbackEventKind, CallbackRejection, Compactions,
        DeliveryEpoch, DialogNoticeTo, Draft, ExitSnapshot, PendingFinal, Prompt, PromptId,
        Provider, ProviderCallback, Reply, Request, RequestId, RequestPhase, Room,
        RoomHistoryFilters, RoomId, RoomKind, RuntimeStatus, StartupFailure, StartupRecovery,
        SubmissionOutcome, AGENT_STARTUP_FAILED, BLOCKED_STALL_MS, BUS_SENDER, DELIVERED_STALL_MS,
        MASTER_ROOM_NAME, QUEUED_STALL_MS, TURN_SETTLE_MS, UNBOUND_SETTLE_MS,
    },
};

#[cfg(test)]
pub(crate) use self::stall::UNCONFIRMED_SUBMISSION_MS;

#[cfg(test)]
#[path = "tests/types_test.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/launch_attempts_test.rs"]
mod launch_attempts_test;

#[cfg(test)]
#[path = "tests/clear_dashboard_test.rs"]
mod clear_dashboard_test;
