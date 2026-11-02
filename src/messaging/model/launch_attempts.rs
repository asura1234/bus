//! Durable launch audit records, including attempts rejected before roster insertion.
use super::{AgentId, Provider, RoomId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaunchKind {
    Create,
    Resume,
    Recovery,
    NativeResume,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaunchOutcome {
    Pending,
    Usable,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct LaunchAttempt {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) provider: Provider,
    pub(crate) room_id: RoomId,
    pub(crate) kind: LaunchKind,
    pub(crate) agent_id: Option<AgentId>,
    pub(crate) launch_id: Option<String>,
    pub(crate) terminal_id: Option<String>,
    pub(crate) started_at_ms: u64,
    pub(crate) deadline_at_ms: u64,
    pub(crate) settled_at_ms: Option<u64>,
    pub(crate) outcome: LaunchOutcome,
    pub(crate) cause: Option<String>,
}

pub(super) mod records {
    use super::LaunchAttempt;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::{collections::BTreeMap, sync::Arc};
    type Records = BTreeMap<u64, LaunchAttempt>;

    pub(in crate::messaging::model) fn serialize<S: Serializer>(
        records: &Arc<Records>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        records.as_ref().serialize(s)
    }
    pub(in crate::messaging::model) fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Arc<Records>, D::Error> {
        Records::deserialize(d).map(Arc::new)
    }
}
