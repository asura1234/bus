//! One restart outcome notice for each work room's orchestrator.
use super::{AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase, RoomKind};
use crate::utils::restart_record::{notice_text, RestartLog};

impl BusState {
    /// Called only after native restore has proved that a different terminal
    /// owns the saved conversation. A coordinator reconnect alone is no end.
    pub(crate) fn mark_restored_turn(&mut self, agent: AgentId) {
        let Some(owner) = self.agents.get(&agent) else {
            return;
        };
        let Some(id) = owner.current_request else {
            return;
        };
        if owner
            .provider_retry
            .as_ref()
            .is_some_and(|hold| hold.lead == id)
        {
            return;
        }
        if let Some(request) = self.requests.get_mut(&id) {
            request.interrupted_by_restore = true;
        }
    }

    /// Runs after durable callbacks. A replaced process cannot finish an old
    /// execution, but its already-recorded answer/error wins over interruption.
    /// Only submitted work closes; pending messages and provider retry holds stay.
    pub(crate) fn settle_restored_turn(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        let id = owner.current_request?;
        if owner.deletion_pending
            || owner.session_binding_invalidated
            || owner
                .provider_retry
                .as_ref()
                .is_some_and(|hold| hold.lead == id)
            || !self.requests.get(&id)?.interrupted_by_restore
        {
            return None;
        }
        self.complete_pending_final(agent, now_ms).ok()?;
        if self.agents.get(&agent)?.current_request != Some(id) {
            return Some(id);
        }
        let recorded = recorded_end(self.requests.get(&id)?);
        if recorded.is_some() {
            self.consume_recorded_end(agent, id, now_ms, recorded, "restored-turn", false, "", "")?;
        }
        let owner = self.agents.get(&agent)?;
        if owner.current_request != Some(id)
            || owner
                .provider_retry
                .as_ref()
                .is_some_and(|hold| hold.lead == id)
        {
            return Some(id);
        }
        // A replied anchor can still own unclaimed steering. Its answer is
        // immutable; only the remaining submitted requests are interrupted.
        let answered = self.requests.get(&id).filter(|r| r.settled()).cloned();
        self.close_turn_with_reason(id, now_ms, super::INTERRUPTED.into())
            .ok()?;
        if let Some(mut answered) = answered {
            answered.phase = RequestPhase::Completed;
            answered.pending_final = None;
            self.requests.insert(id, answered);
        }
        Some(id)
    }

    /// Posts safe successful-restart metadata to each live work room's orchestrator.
    /// Failed outcomes and arbitrary diagnostic reasons go only to the client log.
    /// `Ok(false)` means there was nothing to deliver. A post error leaves the
    /// caller holding the outcome so it can retry.
    pub(crate) fn announce_restart(
        &mut self,
        record: &RestartLog,
        now_ms: u64,
    ) -> Result<bool, String> {
        let rooms: Vec<_> = self
            .rooms()
            .filter(|room| room.kind == RoomKind::Work && !room.deletion_pending)
            .map(|room| room.id)
            .collect();
        let text = notice_text(record);
        if text.is_none() || !record.reason.is_empty() {
            tracing::warn!(
                event = "bus.restart.diagnostic",
                outcome = %record.outcome,
                reason = %record.reason,
                who = %record.who,
                old_sha = %record.old_sha,
                new_sha = %record.new_sha,
                old_binary = %record.old_binary,
                new_binary = %record.new_binary,
                recorded_at_ms = record.at_ms,
                room_ids = ?rooms.iter().map(|room| room.0).collect::<Vec<_>>(),
            );
        }
        let Some(text) = text else {
            return Ok(false);
        };
        let mut posted = false;
        for room in rooms {
            let Some(orchestrator) = self.notice_orchestrator(room, None) else {
                continue;
            };
            self.post_bus_notice(room, Some(orchestrator), text.clone(), now_ms)
                .map_err(|error| error.to_string())?;
            posted = true;
        }
        Ok(posted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::model::{Author, Provider};

    #[test]
    fn restart_notice_reaches_each_work_room_orchestrator() {
        let mut state = BusState::default();
        let master = state.ensure_master_room();
        let alpha = state.create_room("alpha").unwrap();
        let beta = state.create_room("beta").unwrap();
        let quiet = state.create_room("quiet").unwrap();
        let alpha_orch = state
            .create_agent(
                master,
                "alpha-orch",
                Provider::ClaudeCode,
                "/repo".into(),
                None,
            )
            .unwrap();
        let beta_orch = state
            .create_agent(
                master,
                "beta-orch",
                Provider::ClaudeCode,
                "/repo".into(),
                None,
            )
            .unwrap();
        state.bind_orchestrator(alpha_orch, alpha).unwrap();
        state.bind_orchestrator(beta_orch, beta).unwrap();
        let text = "Restarted old -> new (by agent:9).";
        assert!(state
            .announce_restart(
                &crate::utils::restart_record::RestartLog {
                    at_ms: 10,
                    who: "agent:9".into(),
                    old_sha: "old".into(),
                    new_sha: "new".into(),
                    old_binary: "/old".into(),
                    new_binary: "/new".into(),
                    outcome: "restarted".into(),
                    reason: String::new()
                },
                10
            )
            .unwrap());
        let texts = |agent| {
            state
                .requests()
                .filter(|request| request.agent_id == agent)
                .map(|request| request.prompt.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(alpha_orch), vec![text.to_owned()]);
        assert_eq!(texts(beta_orch), vec![text.to_owned()]);
        assert!(state.requests().all(|request| request.room_id != quiet));
        assert!(state.requests().all(|request| request.room_id != master));
        assert!(state
            .requests()
            .all(|request| request.prompt.author == Author::Bus));
    }
}
