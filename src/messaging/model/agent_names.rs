//! Agent names are unique among current agents throughout a Bus session.
use super::{state::normalized_name, AgentId, Author, BusState, ModelError};
use std::collections::BTreeSet;

impl BusState {
    pub(super) fn available_agent_name(
        &self,
        name: &str,
        except: Option<AgentId>,
    ) -> Result<String, ModelError> {
        let name = normalized_name(name)?;
        if super::is_developer_recipient(&name) {
            return Err(ModelError::ReservedAgentName);
        }
        if let Some(agent) = self
            .agents()
            .find(|agent| Some(agent.id) != except && agent.name == name)
        {
            return Err(ModelError::AgentNameTaken {
                name,
                agent: agent.id,
                room: agent.room_id,
            });
        }
        Ok(name)
    }

    /// Repair old snapshots after journal replay, before serving any client.
    /// Reserve every original name first so generated names cannot steal one.
    pub(crate) fn upgrade_agent_names(&mut self, now_ms: u64) -> Result<(), ModelError> {
        let mut reserved: BTreeSet<_> = self.agents().map(|a| a.name.clone()).collect();
        let mut seen = BTreeSet::new();
        let mut renamed = Vec::new();
        // BTreeMap order preserves the lowest-ID owner's original name.
        for agent in self.agents.values_mut() {
            if seen.insert(agent.name.clone()) {
                continue;
            }
            let old = agent.name.clone();
            let base = format!("{old}-{}", agent.id.0);
            let mut name = base.clone();
            let mut suffix = 2;
            while !reserved.insert(name.clone()) {
                name = format!("{base}-{suffix}");
                suffix += 1;
            }
            // Old request formats can lack the send-time snapshot. Capture
            // the same identity their renderer used before this roster edit.
            for request in self.requests.values_mut() {
                if request.prompt.author == Author::Agent(agent.id)
                    && request.sender_identity.is_none()
                {
                    request.sender_identity = Some((old.clone(), agent.room_id));
                }
            }
            agent.name = name.clone();
            renamed.push((agent.room_id, agent.id, old, name));
        }
        for (room, id, old, name) in renamed {
            self.post_bus_notice(
                room,
                None,
                format!("Agent renamed from {old:?} to {name:?} (agent {}) because names must be unique across all rooms.", id.0),
                now_ms,
            )?;
        }
        Ok(())
    }
}
