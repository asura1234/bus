//! Input capabilities, observations and receipts are separate data contracts.
use super::identity::{NativeId, ObservationToken};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Support {
    Present,
    Absent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationKind {
    Submit,
    Steer,
    Stop,
    /// Authorized editor readback may send keys; never a readiness probe.
    ReadDraft,
}

/// Declared transport semantics, never a screen predicate or policy callback.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct OperationCapability {
    pub(crate) kind: OperationKind,
    pub(crate) keys: Vec<String>,
    pub(crate) native_acknowledgment: Support,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Capabilities {
    pub(crate) native_lifecycle: Support,
    pub(crate) generic_lifecycle: Support,
    pub(crate) operations: Vec<OperationCapability>,
}

/// A visual/input assessment does not establish native acceptance or completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum Readiness {
    CandidateReady,
    Busy,
    Draft,
    NeedsAttention(String),
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct InputAssessment {
    pub(crate) token: ObservationToken,
    pub(crate) readiness: Readiness,
    pub(crate) evidence: Vec<String>,
}

/// Scope is captured before a write. A later attempt must not inherit a stop
/// or an acknowledgment merely because it occupies the same terminal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct OperationScope {
    pub(crate) id: String,
    pub(crate) kind: OperationKind,
    pub(crate) token: ObservationToken,
    pub(crate) attempts: Vec<NativeId>,
    pub(crate) inputs: Vec<NativeId>,
}

/// None of these transport receipts is a native start/end certificate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub(crate) enum WriteDisposition {
    Enqueued,
    Written,
    DefinitelyUnwritten(String),
    Uncertain(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct WriteReceipt {
    pub(crate) operation_id: String,
    pub(crate) disposition: WriteDisposition,
}
