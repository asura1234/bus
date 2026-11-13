use crate::messaging::model::{AgentId, PromptId, RequestId, RoomId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AlertSource {
    pub(crate) prompt_id: PromptId,
    pub(crate) request_id: Option<RequestId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AlertRoute {
    pub(crate) source: AlertSource,
    pub(crate) room: RoomId,
    pub(crate) raising_agent: Option<AgentId>,
    pub(crate) orchestrator: Option<AgentId>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct OutboxRecord {
    pub(crate) code: String,
    pub(crate) key: String,
    pub(crate) session: String,
    pub(crate) source: AlertSource,
    pub(crate) room: RoomId,
    pub(crate) raising_agent: Option<AgentId>,
    pub(crate) orchestrator: Option<AgentId>,
    pub(crate) recipient: String,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) ask_body: Option<String>,
    #[serde(default)]
    pub(crate) aliases: Vec<AlertRoute>,
    // attempted without returned IDs is uncertain; retain command failures too.
    #[serde(default)]
    pub(crate) send_error: Option<String>,
    pub(crate) created_at_ms: u64,
    pub(crate) attempted: bool,
    pub(crate) message_id: Option<String>,
    pub(crate) chat_id: Option<String>,
}

impl OutboxRecord {
    pub(crate) fn route(&self) -> AlertRoute {
        AlertRoute {
            source: self.source.clone(),
            room: self.room,
            raising_agent: self.raising_agent,
            orchestrator: self.orchestrator,
        }
    }

    pub(crate) fn routes(&self) -> impl Iterator<Item = AlertRoute> + '_ {
        std::iter::once(self.route()).chain(self.aliases.iter().cloned())
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FeishuState {
    pub(crate) outbox: BTreeMap<String, OutboxRecord>,
}

impl FeishuState {
    pub(crate) fn is_empty(&self) -> bool {
        self.outbox.is_empty()
    }
}
