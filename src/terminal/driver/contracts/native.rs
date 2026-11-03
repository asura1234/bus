//! Normalized observations retain raw evidence; the journal supplies authority.
use super::identity::{EndpointIdentity, NativeId, SourceCursor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Only a timestamp actually supplied by the native record belongs here.
/// Display/calendar estimates are not native reset evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct NativeReset {
    pub(crate) at_ms: u64,
    pub(crate) source: SourceCursor,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ErrorEvidence {
    pub(crate) raw: Value,
    pub(crate) message: Option<String>,
    pub(crate) code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    /// A native assertion, never the common scheduler's retry decision.
    /// None preserves the difference between absent metadata and false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retryable: Option<bool>,
    /// Native cause order/nesting survives normalization without requiring a
    /// reducer to decode provider-specific fields in raw payloads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) causes: Vec<ErrorEvidence>,
    pub(crate) http_status: Option<u16>,
    pub(crate) native_reset: Option<NativeReset>,
}

/// These are native outcome observations, not a retry verdict. In particular,
/// Interrupted does not claim a user stop; operation evidence is correlated later.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum Outcome {
    Completed,
    Failed(Box<ErrorEvidence>),
    Interrupted { cause: Option<Box<ErrorEvidence>> },
    Cancelled,
    Incomplete,
}

/// Ordered native input parts, before request correlation. Opaque attachment
/// evidence is retained when the native format supplies no stable identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum InputPart {
    Text(String),
    Attachment {
        native_id: Option<NativeId>,
        raw: Value,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum NativeEvent {
    InputAccepted {
        input: NativeId,
        attempt: Option<NativeId>,
        parts: Vec<InputPart>,
    },
    AttemptStarted {
        attempt: NativeId,
        inputs: Vec<NativeId>,
    },
    /// Emit only an observed relationship, never one guessed from append order.
    AttemptLinked {
        attempt: NativeId,
        parent: NativeId,
    },
    AttemptEndObserved {
        attempt: NativeId,
        outcome: Outcome,
    },
    InputCommitObserved {
        input: NativeId,
        attempt: NativeId,
        outcome: Outcome,
    },
    ReplyObserved {
        attempt: NativeId,
        text: String,
        final_part: bool,
    },
    BackgroundObserved {
        tasks: Vec<NativeId>,
    },
    /// Unsupported native records remain inspectable instead of being coerced
    /// into success, failure or a guessed UI state.
    Unrecognized {
        native_kind: String,
        payload: Value,
    },
}

/// Native event time has its own clock domain and may precede capture time.
/// It is correlation evidence, not the Bus retry budget's first-failure clock.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct NativeTime {
    pub(crate) clock: String,
    pub(crate) millis: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct FactEnvelope {
    pub(crate) endpoint: EndpointIdentity,
    pub(crate) source: SourceCursor,
    pub(crate) observed_at_ms: u64,
    pub(crate) native_time: Option<NativeTime>,
    pub(crate) raw: Value,
    pub(crate) event: NativeEvent,
}

/// A trailing fragment is not a malformed complete record. Readers retain its
/// last complete cursor/bytes; this enum neither advances nor acknowledges it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum ReadProgress {
    Complete(SourceCursor),
    PendingTail {
        last_complete: SourceCursor,
        fragment: Vec<u8>,
    },
    EvidenceGap {
        last_complete: SourceCursor,
        reason: String,
        retained: Vec<u8>,
    },
}
