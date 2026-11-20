//! Edge-triggered idle-worker reminders, with durable roster dedupe.
//!
//! Two notices share this path. The count reminder fires when spare workers
//! cross `idle_agents_alert`. The duration reminder fires once per idle
//! episode when one work-room agent sits idle longer than `idle_alert_minutes`.
use super::{
    AgentId, Author, BusState, ModelError, Request, RoomAgent, RoomId, RoomKind, RuntimeStatus,
};
use std::collections::BTreeSet;

/// Preserve the original alert's minimum idle interval.
pub(crate) const MIN_IDLE_FOR_ALERT_MS: u64 = 60_000;
const STAFFING_GRACE_MS: u64 = 2 * MIN_IDLE_FOR_ALERT_MS;
const IDLE_ALERT_ACTION: &str = ". Do not give them a new task. Delete the ones you no longer need (bus agent delete NAME --confirm). A new task gets a new agent (bus agent add).";
const MS_PER_MINUTE: u64 = 60_000;

impl BusState {
    /// Remind only a room's bound orchestrator on a strict crossing above the limit,
    /// and once per idle episode when a worker sits longer than `minutes`.
    /// The caller keeps the count crossing in memory; saved notices and the
    /// agent's episode start dedupe both reminders across restarts.
    pub(crate) fn notify_idle_agents(
        &mut self,
        limit: u32,
        minutes: u32,
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
                if self
                    .idle_workers(room, |agent| self.finished(agent, now_ms))
                    .len()
                    <= limit as usize
                {
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
            let labels = idle
                .iter()
                .map(|id| self.agent_message_label(*id))
                .collect::<Vec<_>>()
                .join(", ");
            let text = format!(
                "{} agents in room {} are idle (limit {}): {}{}",
                idle.len(),
                room.0,
                limit,
                labels,
                IDLE_ALERT_ACTION
            );
            let id = self.post_orchestrator_notice(room, orchestrator, text, now_ms)?;
            let request = self
                .requests
                .get_mut(&id)
                .ok_or(ModelError::UnknownRequest(id))?;
            request.prompt.idle_agent_ids = idle.into_iter().collect();
            changed = true;
        }
        // A count notice must not skip the duration pass: `||` would leave
        // that tick's episode clock unrecorded and save it on the next tick.
        let duration = self.notify_idle_duration(minutes, now_ms)?;
        Ok(changed || duration)
    }

    /// One notice per continuous idle episode, only to the work room's orchestrator.
    /// The episode clock is saved on the agent so a restart's Unavailable gap
    /// neither repeats a notice nor forgets time already idle.
    fn notify_idle_duration(&mut self, minutes: u32, now_ms: u64) -> Result<bool, ModelError> {
        let threshold_ms = u64::from(minutes).saturating_mul(MS_PER_MINUTE);
        let ids: Vec<_> = self.agents.keys().copied().collect();
        let mut changed = false;
        for id in ids {
            if self.update_idle_duration(id, minutes, threshold_ms, now_ms)? {
                changed = true;
            }
        }
        Ok(changed)
    }

    fn update_idle_duration(
        &mut self,
        id: AgentId,
        minutes: u32,
        threshold_ms: u64,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        if !self.sitting_idle(id) {
            return self.clear_idle_clock_on_activity(id);
        }
        let (saved, status_since, noticed) = {
            let agent = self.agent(id).ok_or(ModelError::UnknownAgent(id))?;
            (
                agent.sitting_idle_since_ms,
                agent.status_since_ms,
                agent.idle_duration_notice_since_ms,
            )
        };
        let (start, changed) = self.remember_episode_start(id, saved, status_since)?;
        if now_ms.saturating_sub(start) <= threshold_ms || noticed == Some(start) {
            return Ok(changed);
        }
        Ok(changed | self.post_idle_duration(id, minutes, start, now_ms)?)
    }

    /// Keeps a saved start across a restart's later `status_since_ms`.
    /// A message newer than that start opens a new episode at the later of
    /// the message and the status change.
    fn remember_episode_start(
        &mut self,
        id: AgentId,
        saved: Option<u64>,
        status_since_ms: u64,
    ) -> Result<(u64, bool), ModelError> {
        let message = self.latest_message_at(id);
        if let Some(saved) = saved.filter(|saved| message.is_none_or(|at| at <= *saved)) {
            return Ok((saved, false));
        }
        let computed = message.map_or(status_since_ms, |at| status_since_ms.max(at));
        if saved == Some(computed) {
            return Ok((computed, false));
        }
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .sitting_idle_since_ms = Some(computed);
        Ok((computed, true))
    }

    fn latest_message_at(&self, id: AgentId) -> Option<u64> {
        self.requests()
            .filter(|request| request.agent_id == id)
            .map(message_at)
            .max()
    }

    fn sitting_idle(&self, id: AgentId) -> bool {
        let facts = self.agent(id).map(|agent| {
            (
                agent.orchestrates.is_some(),
                agent.deletion_pending,
                agent.current_request.is_some(),
                agent.shown_status(),
                agent.room_id,
            )
        });
        let Some((orchestrator, deleting, current, shown, room)) = facts else {
            return false;
        };
        if orchestrator
            || deleting
            || current
            || shown != RuntimeStatus::Idle
            || !self.queued_requests(id).is_empty()
        {
            return false;
        }
        self.rooms
            .get(&room)
            .is_some_and(|room| room.kind == RoomKind::Work && !room.deletion_pending)
    }

    fn clear_idle_clock_on_activity(&mut self, id: AgentId) -> Result<bool, ModelError> {
        if !self.resets_idle_clock(id) {
            return Ok(false);
        }
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if agent.sitting_idle_since_ms.is_none() {
            return Ok(false);
        }
        agent.sitting_idle_since_ms = None;
        Ok(true)
    }

    fn resets_idle_clock(&self, id: AgentId) -> bool {
        let shown = self
            .agent(id)
            .map(|agent| (agent.current_request.is_some(), agent.shown_status()));
        let Some((busy, shown)) = shown else {
            return false;
        };
        busy || !self.queued_requests(id).is_empty()
            || matches!(shown, RuntimeStatus::Working | RuntimeStatus::Blocked)
    }

    fn post_idle_duration(
        &mut self,
        id: AgentId,
        minutes: u32,
        since: u64,
        now_ms: u64,
    ) -> Result<bool, ModelError> {
        let (room, name) = {
            let agent = self.agent(id).ok_or(ModelError::UnknownAgent(id))?;
            (agent.room_id, agent.name.clone())
        };
        let text = format!(
            "{name} (agent {}) has been sitting idle longer than {}.",
            id.0,
            idle_minutes_label(minutes)
        );
        let Some(orchestrator) = self.notice_orchestrator(room, Some(id)) else {
            return Ok(false);
        };
        self.post_informational_notice(room, orchestrator, id, text, now_ms)?;
        self.agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?
            .idle_duration_notice_since_ms = Some(since);
        Ok(true)
    }

    /// Qualified workers in creation order, excluding the orchestrator and pending work.
    fn idle_workers(&self, room: RoomId, test: impl Fn(&RoomAgent) -> bool) -> Vec<AgentId> {
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
            .map(|agent| agent.id)
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

    fn idle_alerted_before(&self, room: RoomId, idle: &[AgentId]) -> bool {
        let idle = idle.iter().copied().collect::<BTreeSet<_>>();
        self.requests().any(|request| {
            request.room_id == room
                && request.prompt.author == Author::Bus
                && request.prompt.idle_agent_ids == idle
        })
    }
}

fn message_at(request: &Request) -> u64 {
    request
        .completed_at_ms
        .map_or(request.prompt.submitted_at_ms, |completed| {
            completed.max(request.prompt.submitted_at_ms)
        })
}

fn idle_minutes_label(minutes: u32) -> String {
    match minutes {
        1 => "1 minute".to_owned(),
        _ => format!("{minutes} minutes"),
    }
}
