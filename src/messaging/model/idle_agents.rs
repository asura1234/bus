//! Edge-triggered idle-worker reminders, with episode state owned by the coordinator.
use super::{BusState, ModelError, RoomAgent, RoomId, RoomKind, RuntimeStatus};
use std::collections::BTreeSet;

/// Preserve the original alert's definition of an idle worker.
pub(crate) const MIN_IDLE_FOR_ALERT_MS: u64 = 60_000;

impl BusState {
    /// Remind only a room's bound orchestrator on a strict crossing above the limit.
    /// The caller keeps `alerted` in memory and commits it after saving any notices.
    pub(crate) fn notify_idle_agents(
        &mut self,
        limit: u32,
        now_ms: u64,
        alerted: &mut BTreeSet<RoomId>,
    ) -> Result<bool, ModelError> {
        let rooms: Vec<_> = self
            .rooms
            .values()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| room.id)
            .collect();
        alerted.retain(|room| rooms.contains(room));
        let mut changed = false;
        for room in rooms {
            if alerted.contains(&room) {
                // A lost status or a reset idle clock is not evidence of new work.
                // Preserve the original alert's conservative rearming rule.
                if self.idle_workers(room, may_be_idle).len() <= limit as usize {
                    alerted.remove(&room);
                }
                continue;
            }
            let idle = self.idle_workers(room, |agent| finished(agent, now_ms));
            if idle.len() <= limit as usize {
                continue;
            }
            // post_bus_notice(None) falls back to the developer; never take that route.
            let Some(orchestrator) = self.notice_orchestrator(room, None) else {
                continue;
            };
            let text = format!(
                "{} agents in room {} are idle (limit {}): {}. Give them parallel work \
                 or delete the ones you no longer need (bus agent delete NAME --confirm).",
                idle.len(),
                room.0,
                limit,
                idle.join(", ")
            );
            self.post_bus_notice(room, Some(orchestrator), text, now_ms)?;
            alerted.insert(room);
            changed = true;
        }
        Ok(changed)
    }

    /// Qualified workers in creation order, excluding the orchestrator and pending work.
    fn idle_workers(&self, room: RoomId, test: impl Fn(&RoomAgent) -> bool) -> Vec<String> {
        self.agents
            .values()
            .filter(|agent| {
                agent.room_id == room
                    && agent.orchestrates.is_none()
                    && !agent.deletion_pending
                    && agent.current_request.is_none()
                    && self.queued_requests(agent.id).is_empty()
                    && test(agent)
            })
            .map(|agent| self.agent_message_label(agent.id))
            .collect()
    }
}

fn finished(agent: &RoomAgent, now_ms: u64) -> bool {
    agent.shown_status() == RuntimeStatus::Idle
        && now_ms.saturating_sub(agent.status_since_ms) >= MIN_IDLE_FOR_ALERT_MS
}

fn may_be_idle(agent: &RoomAgent) -> bool {
    matches!(
        agent.shown_status(),
        RuntimeStatus::Idle | RuntimeStatus::Unavailable | RuntimeStatus::Launching
    )
}
