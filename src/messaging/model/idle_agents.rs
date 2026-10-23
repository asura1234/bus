//! Durable, code-driven notice when a work room collects idle workers.
use super::{BusState, ModelError, RoomAgent, RoomId, RoomKind, RuntimeStatus};

/// How long a worker must stay idle, with no status change, before it counts.
pub(crate) const MIN_IDLE_FOR_ALERT_MS: u64 = 60_000;

impl BusState {
    /// One Bus notice per episode when a work room reaches `limit` idle
    /// workers. The room's latch clears only once its workers are really
    /// below the limit: a worker idle under the minimum, or whose status is
    /// unknown (every agent is Unavailable right after a restart, then
    /// Launching), still holds the episode open, so restarts never re-alert.
    pub(crate) fn notify_idle_agents(
        &mut self,
        limit: u32,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        let rooms: Vec<_> = self
            .rooms
            .values()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| (room.id, room.idle_alert_sent))
            .collect();
        let limit = limit as usize;
        let mut changed = false;
        for (room, sent) in rooms {
            let latch = if sent {
                self.workers(room, may_be_idle).len() >= limit
            } else {
                let idle = self.workers(room, |agent| finished(agent, now_ms));
                if idle.len() >= limit {
                    let text = format!(
                        "You have {} idle agents ({}). Either put them to work in parallel \
                         on the open PRs, or close the ones you no longer need to save \
                         resources. Your call.",
                        idle.len(),
                        idle.join(", ")
                    );
                    let orchestrator = self.notice_orchestrator(room, None);
                    self.post_bus_notice(room, orchestrator, text, now_ms)?;
                }
                idle.len() >= limit
            };
            if latch != sent {
                self.rooms
                    .get_mut(&room)
                    .ok_or(ModelError::UnknownRoom(room))?
                    .idle_alert_sent = latch;
                changed = true;
            }
        }
        Ok(changed)
    }

    /// Names of `room`'s workers that pass `test`, in creation order.
    /// Orchestrators live in MASTER, so never count here.
    fn workers(&self, room: RoomId, test: impl Fn(&RoomAgent) -> bool) -> Vec<String> {
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

/// Finished its work: shown Idle (an open dialog shows Blocked), with no
/// status change for at least `MIN_IDLE_FOR_ALERT_MS`.
fn finished(agent: &RoomAgent, now_ms: u64) -> bool {
    agent.shown_status() == RuntimeStatus::Idle
        && now_ms.saturating_sub(agent.status_since_ms) >= MIN_IDLE_FOR_ALERT_MS
}

/// Idle for any time, or not known to be busy.
fn may_be_idle(agent: &RoomAgent) -> bool {
    matches!(
        agent.shown_status(),
        RuntimeStatus::Idle | RuntimeStatus::Unavailable | RuntimeStatus::Launching
    )
}
