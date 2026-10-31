//! Edge-triggered idle-worker reminders, with durable roster dedupe.
use super::{Author, BusState, ModelError, RoomAgent, RoomId, RoomKind, RuntimeStatus};
use std::collections::BTreeSet;

/// Preserve the original alert's minimum idle interval.
pub(crate) const MIN_IDLE_FOR_ALERT_MS: u64 = 60_000;
const STAFFING_GRACE_MS: u64 = 2 * MIN_IDLE_FOR_ALERT_MS;
const IDLE_ALERT_ACTION: &str = ". Give them parallel work or delete the ones you no longer need (bus agent delete NAME --confirm).";

impl BusState {
    /// Remind only a room's bound orchestrator on a strict crossing above the limit.
    /// The caller keeps the crossing in memory; saved notices dedupe rosters across restarts.
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
            let idle = self.idle_workers(room, |agent| self.finished(agent, now_ms));
            if idle.len() <= limit as usize {
                continue;
            }
            // post_bus_notice(None) falls back to the developer; never take that route.
            let Some(orchestrator) = self.notice_orchestrator(room, None) else {
                continue;
            };
            alerted.insert(room);
            if self.idle_alerted_before(room, &idle) {
                continue;
            }
            let text = format!(
                "{} agents in room {} are idle (limit {}): {}{}",
                idle.len(),
                room.0,
                limit,
                idle.join(", "),
                IDLE_ALERT_ACTION
            );
            self.post_bus_notice(room, Some(orchestrator), text, now_ms)?;
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

    fn finished(&self, agent: &RoomAgent, now_ms: u64) -> bool {
        if agent.shown_status() != RuntimeStatus::Idle
            || now_ms.saturating_sub(agent.status_since_ms) < STAFFING_GRACE_MS
        {
            return false;
        }
        let last = self
            .requests()
            .filter(|request| request.agent_id == agent.id)
            .last();
        last.is_none_or(|request| {
            request.failure_reason.as_deref() != Some("reply_not_captured")
                && now_ms.saturating_sub(request.prompt.submitted_at_ms) >= STAFFING_GRACE_MS
        })
    }

    fn idle_alerted_before(&self, room: RoomId, idle: &[String]) -> bool {
        let idle = idle.iter().cloned().collect::<BTreeSet<_>>();
        self.requests().any(|request| {
            request.room_id == room
                && request.prompt.author == Author::Bus
                && idle_alert_names(&request.prompt.text).is_some_and(|names| names == idle)
        })
    }
}

fn idle_alert_names(text: &str) -> Option<BTreeSet<String>> {
    let head = text.strip_suffix(IDLE_ALERT_ACTION)?;
    if !head.contains(" agents in room ") || !head.contains(" are idle (limit ") {
        return None;
    }
    let (_, names) = head.rsplit_once(": ")?;
    Some(names.split(", ").map(str::to_owned).collect())
}

fn may_be_idle(agent: &RoomAgent) -> bool {
    matches!(
        agent.shown_status(),
        RuntimeStatus::Idle | RuntimeStatus::Unavailable | RuntimeStatus::Launching
    )
}
