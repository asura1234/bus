//! One restart outcome notice for each work room's orchestrator.
use super::{
    AgentId, BusState, PendingFinal, RecordedTurnEnd, Request, RequestId, RequestPhase, RoomKind,
};
use crate::utils::restart_record::{notice_text, RestartLog};

impl BusState {
    pub(crate) fn note_native_rebind(&mut self, agent: AgentId) {
        if let Some(owner) = self.agents.get_mut(&agent) {
            owner.restore_pending_receipt = true;
        }
    }

    pub(crate) fn acknowledge_native_rebinds(&mut self) {
        for owner in self.agents.values_mut() {
            owner.restore_pending_receipt = false;
        }
    }

    /// Native terminal replacement or a successful restart receipt proves the
    /// execution was interrupted. A coordinator reconnect alone is no end.
    pub(crate) fn mark_restored_turn(&mut self, agent: AgentId) {
        let Some(owner) = self.agents.get(&agent) else {
            return;
        };
        let held = owner.provider_retry.as_ref().map(|hold| hold.lead);
        let answered_anchor = owner.current_request.filter(|id| {
            self.requests
                .get(id)
                .is_some_and(|r| r.settled() && r.pending_final.is_some())
        });
        // Callback consumption can re-anchor steering or reveal independent
        // Kimi roots. Mark the submitted snapshot, never later queued delivery.
        let submitted: Vec<_> = self
            .unsettled_requests()
            .filter(|r| {
                r.agent_id == agent
                    && matches!(r.phase, RequestPhase::Submitting | RequestPhase::Active)
            })
            .map(|r| r.id)
            .chain(answered_anchor)
            .collect();
        for id in submitted {
            if let Some(request) = self
                .requests
                .get_mut(&id)
                .filter(|r| !held.is_some_and(|lead| r.id == lead || r.group == Some(lead)))
            {
                request.interrupted_by_restore = true;
            }
        }
    }

    /// Runs after durable callbacks. A replaced process cannot finish an old
    /// execution, but its already-recorded answer/error wins over interruption.
    /// Only submitted work closes; pending messages and provider retry holds stay.
    pub(crate) fn settle_restored_turn(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        mut recorded_end: impl FnMut(&Request) -> Option<RecordedTurnEnd>,
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
        self.recover_restored_answers(agent, id, now_ms, &mut recorded_end)?;
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
            answered.pending_final = None;
            self.requests.insert(id, answered);
        }
        Some(id)
    }

    fn recover_restored_answers(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded_end: &mut impl FnMut(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<()> {
        let lead = self.requests.get(&id)?.clone();
        let members = self.group_members(id);
        let recorded = recorded_end(&lead);
        if let Some(RecordedTurnEnd::Final {
            turn_id, prompts, ..
        }) = &recorded
        {
            self.bind_recorded_turn_inputs(agent, turn_id, prompts);
            self.complete_pending_final(agent, now_ms).ok()?;
        }
        for member in &members {
            let Some(member) = self.requests.get(member).filter(|r| !r.settled()).cloned() else {
                continue;
            };
            self.recover_steered_answer(&lead, &member, &members, recorded_end(&member), now_ms);
        }
        // A consumed final on an answered anchor is not a missing reply.
        // Recover every independent member before delivering/closing the lead.
        if !self.requests.get(&id)?.settled() {
            match recorded {
                Some(RecordedTurnEnd::Final {
                    turn_id,
                    text,
                    prompts,
                }) => {
                    let answer = PendingFinal {
                        callback_id: format!("restored-turn:{turn_id}"),
                        text,
                        received_at_ms: now_ms,
                        provider_session_id: lead.provider_session_id.clone(),
                        provider_turn_id: Some(turn_id),
                    };
                    self.deliver_recorded_final(agent, id, answer, &prompts, now_ms);
                }
                Some(other) => {
                    self.consume_recorded_end(
                        agent,
                        id,
                        now_ms,
                        Some(other),
                        "restored-turn",
                        false,
                        "",
                        "",
                    );
                }
                None => {}
            }
        }
        Some(())
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
