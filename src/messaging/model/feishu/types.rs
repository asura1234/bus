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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct IncomingReply {
    pub(crate) app_id: String,
    pub(crate) message_id: String,
    pub(crate) chat_id: String,
    pub(crate) sender_id: String,
    pub(crate) text: String,
    pub(crate) reply_to: Option<String>,
    pub(crate) root_id: Option<String>,
    pub(crate) create_time: u64,
}

impl IncomingReply {
    pub(crate) fn key(&self) -> String {
        format!("{}:{}", self.app_id, self.message_id)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub(crate) enum IngestOutcome {
    Delivered {
        message_id: PromptId,
        request_id: RequestId,
    },
    Parked {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InboxRecord {
    pub(crate) input: IncomingReply,
    pub(crate) code: Option<String>,
    pub(crate) outcome: IngestOutcome,
    pub(crate) received_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct FeishuState {
    pub(crate) outbox: BTreeMap<String, OutboxRecord>,
    #[serde(default)]
    pub(crate) inbox: BTreeMap<String, InboxRecord>,
}

impl FeishuState {
    pub(crate) fn is_empty(&self) -> bool {
        self.outbox.is_empty() && self.inbox.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InReplyTo {
    pub(crate) session: String,
    pub(crate) source: AlertSource,
    pub(crate) room: RoomId,
    pub(crate) raising_agent: Option<AgentId>,
    pub(crate) orchestrator: AgentId,
    pub(crate) app_id: String,
    pub(crate) feishu_message_id: String,
    pub(crate) outbound_message_id: Option<String>,
    pub(crate) chat_id: String,
    pub(crate) sender_open_id: String,
    pub(crate) create_time: u64,
}
