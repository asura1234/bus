//! Bus-owned names and native source identifiers stay at the room boundary.

use crate::agents::providers::ProviderKind;

use super::model::{AgentId, Provider, RoomId};

pub(crate) const fn provider_kind(provider: Provider) -> ProviderKind {
    match provider {
        Provider::Codex => ProviderKind::Codex,
        Provider::ClaudeCode => ProviderKind::ClaudeCode,
        Provider::Cursor => ProviderKind::Cursor,
    }
}

pub(crate) fn managed_name(room: RoomId, agent: AgentId) -> String {
    format!("bus-r{}-a{}", room.0, agent.0)
}

pub(crate) fn managed_source(provider: Provider) -> String {
    format!("herdr:{}", provider_kind(provider).label())
}
