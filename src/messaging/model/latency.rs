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

/// One clock read for a step inside delivery selection.
///
/// Assigning `Some` records that the step finished. Leaving the field `None`
/// means the step did not run. A finished step can still be `Some(0)` only
/// when the clock itself is 0; callers must not use 0 as a missing-step sentinel.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DispatchMark {
    Started,
    Render,
    Boundary,
    Notice,
    Save,
    Diagnostics,
    Method,
}

/// Clocks for the work around `record_picked` → `note_native_requested`.
///
/// `started` through `boundary` finish before the pick stamp, so those phases
/// are outside `dispatch`. `notice` through `method` plus the native-request
/// stamp partition `dispatch`. When every in-window stamp is present the
/// durations sum to `dispatch` exactly; shared edges cancel, so the residual
/// is 0. A missing later stamp stays `None` and that phase is omitted.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct DispatchTimestamps {
    pub(crate) started_at_ms: Option<u64>,
    pub(crate) render_at_ms: Option<u64>,
    pub(crate) boundary_at_ms: Option<u64>,
    pub(crate) notice_at_ms: Option<u64>,
    pub(crate) save_at_ms: Option<u64>,
    pub(crate) diagnostics_at_ms: Option<u64>,
    pub(crate) method_at_ms: Option<u64>,
}

impl DispatchTimestamps {
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct DeliveryLatency {
    pub(crate) accepted_at_ms: Option<u64>,
    pub(crate) queued_at_ms: Option<u64>,
    pub(crate) picked_at_ms: Option<u64>,
    pub(crate) native_requested_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "DispatchTimestamps::is_empty")]
    pub(crate) dispatch: DispatchTimestamps,
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

    /// Drops one native attempt. Admission, pick, and room delivery stay,
    /// and the next attempt's first write still wins inside that attempt.
    /// Dispatch sub-phase stamps belong to the attempt and clear with it.
    pub(crate) fn clear_attempt(&mut self) {
        self.native_requested_at_ms = None;
        self.dispatch = DispatchTimestamps::default();
        self.native = NativeSubmissionTimestamps::default();
        self.provider_started_at_ms = None;
        self.reply_captured_at_ms = None;
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
