use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct NativeSubmissionTimestamps {
    pub server_accepted_at_ms: Option<u64>,
    pub writer_queued_at_ms: Option<u64>,
    pub paste_started_at_ms: Option<u64>,
    pub text_written_at_ms: Option<u64>,
    pub paste_completed_at_ms: Option<u64>,
    pub composer_owned_at_ms: Option<u64>,
    pub enter_sent_at_ms: Option<u64>,
    pub enter_written_at_ms: Option<u64>,
    pub native_confirmed_at_ms: Option<u64>,
}

impl NativeSubmissionTimestamps {
    pub(crate) fn merge_from(&mut self, other: Self) {
        for (current, new) in [
            (&mut self.server_accepted_at_ms, other.server_accepted_at_ms),
            (&mut self.writer_queued_at_ms, other.writer_queued_at_ms),
            (&mut self.paste_started_at_ms, other.paste_started_at_ms),
            (&mut self.text_written_at_ms, other.text_written_at_ms),
            (&mut self.paste_completed_at_ms, other.paste_completed_at_ms),
            (&mut self.composer_owned_at_ms, other.composer_owned_at_ms),
            (&mut self.enter_sent_at_ms, other.enter_sent_at_ms),
            (&mut self.enter_written_at_ms, other.enter_written_at_ms),
            (
                &mut self.native_confirmed_at_ms,
                other.native_confirmed_at_ms,
            ),
        ] {
            if current.is_none() {
                *current = new;
            }
        }
    }
}

impl From<crate::protocol::api::schema::NativeSubmissionTimestamps> for NativeSubmissionTimestamps {
    fn from(other: crate::protocol::api::schema::NativeSubmissionTimestamps) -> Self {
        Self {
            server_accepted_at_ms: other.server_accepted_at_ms,
            writer_queued_at_ms: other.writer_queued_at_ms,
            paste_started_at_ms: other.paste_started_at_ms,
            text_written_at_ms: other.text_written_at_ms,
            paste_completed_at_ms: other.paste_completed_at_ms,
            composer_owned_at_ms: other.composer_owned_at_ms,
            enter_sent_at_ms: other.enter_sent_at_ms,
            enter_written_at_ms: other.enter_written_at_ms,
            native_confirmed_at_ms: other.native_confirmed_at_ms,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct DeliveryLatency {
    pub(crate) accepted_at_ms: Option<u64>,
    pub(crate) queued_at_ms: Option<u64>,
    pub(crate) picked_at_ms: Option<u64>,
    pub(crate) native_requested_at_ms: Option<u64>,
    pub(crate) native: NativeSubmissionTimestamps,
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
