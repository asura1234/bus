use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct DeliveryLatency {
    pub(crate) accepted_at_ms: Option<u64>,
    pub(crate) queued_at_ms: Option<u64>,
    pub(crate) picked_at_ms: Option<u64>,
    pub(crate) native_requested_at_ms: Option<u64>,
    pub(crate) native: crate::protocol::api::schema::NativeSubmissionTimestamps,
    pub(crate) provider_started_at_ms: Option<u64>,
    pub(crate) reply_captured_at_ms: Option<u64>,
    pub(crate) reply_delivered_at_ms: Option<u64>,
    pub(crate) ready_at_enqueue: bool,
    pub(crate) agent_waited: bool,
}

impl DeliveryLatency {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum DialogNoticeDelivery {
    /// Saved before native I/O; a restart may lose its outcome entirely.
    Unconfirmed {
        since_ms: u64,
    },
    Confirmed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct QueuedRejection {
    /// The first definite refusal, not renewed by retries or unrelated turns.
    pub(crate) at_ms: u64,
    pub(crate) reason: String,
}
