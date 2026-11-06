//! Identities retain their native namespace; correlation belongs to the journal.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct NativeId {
    pub(crate) namespace: String,
    pub(crate) value: String,
}

/// Existing launch identity, not an additional restart epoch.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct EndpointIdentity {
    pub(crate) terminal: String,
    pub(crate) pane: String,
    pub(crate) launch: String,
    pub(crate) session: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", content = "value", rename_all = "snake_case")]
pub(crate) enum Position {
    Bytes(u64),
    Sequence(u64),
}

/// A source-local file/stream identity and last complete position. The identity
/// changes on replacement; a timestamp is not proof of source continuity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceCursor {
    pub(crate) stream: String,
    pub(crate) identity: String,
    pub(crate) through: Position,
}

/// UI and native data have separate watermarks; neither claims atomicity with
/// the other. Admission must revalidate them when production consumers arrive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ObservationToken {
    pub(crate) endpoint: EndpointIdentity,
    pub(crate) content_revision: u64,
    pub(crate) native_through: Option<SourceCursor>,
}
