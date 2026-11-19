//! Preserve independently answered joined inputs before releasing a retirement hold.
use super::{
    check_callback_identity, provider_turn_key, AgentId, BusState, CallbackEventKind, PendingFinal,
    ProviderCallback, RequestPhase,
};
use crate::messaging::model::types::CompletedReply;
use std::collections::BTreeMap;

impl BusState {
    pub(crate) fn reconcile_retirement_answers(
        &mut self,
        agent: AgentId,
        evidence: &[ProviderCallback],
    ) {
        let Some(id) = self.interrupt_retirement_evidence_owner(agent) else {
            return;
        };
        let Some(lead) = self.request(id) else {
            return;
        };
        let Some(owner) = self.agent(agent) else {
            return;
        };
        let provider = owner.provider;
        let lead_turn = lead.provider_turn_id.clone();
        let origin = lead.native_delivery_identity.clone();
        let members = self.group_members(id);
        let mut claims: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for event in evidence {
            if event.provider_turn_id == lead_turn
                || !matches!(event.kind, CallbackEventKind::PromptStarted)
                || self.has_reported_final(&event.callback_id)
            {
                continue;
            }
            let mut start = event.clone();
            self.match_owned_claude_paste(&mut start);
            let Some(payload) = start.prompt_payload.as_deref() else {
                continue;
            };
            let mut matching = members.iter().filter_map(|id| {
                self.request(*id)
                    .filter(|member| {
                        member.steered
                            && !member.trusted_start_bound
                            && !member.settled()
                            && member.native_delivery_identity == origin
                            && member
                                .native_delivery_callback_boundary
                                .is_some_and(|boundary| start.sequence > boundary)
                            && check_callback_identity(member, &start).is_ok()
                            && member.matches_callback_payload(payload, provider)
                    })
                    .map(|_| *id)
            });
            if let Some(member) = matching.next().filter(|_| matching.next().is_none()) {
                claims.entry(member).or_default().push(start);
            }
        }
        for (id, starts) in &claims {
            let Some(start) = starts.first() else {
                continue;
            };
            if starts
                .iter()
                .any(|other| other.provider_turn_id != start.provider_turn_id)
                || claims.iter().any(|(other, starts)| {
                    other != id
                        && starts
                            .iter()
                            .any(|s| s.provider_turn_id == start.provider_turn_id)
                })
            {
                continue;
            }
            let Some(turn_key) = provider_turn_key(
                &start.launch_id,
                start.provider_session_id.as_deref(),
                start.provider_turn_id.as_deref(),
            ) else {
                continue;
            };
            if self.consumed_provider_turns.contains(&turn_key)
                || self.unrelated_provider_turns.contains(&turn_key)
            {
                continue;
            }
            let Some(member) = self.requests.get_mut(id) else {
                continue;
            };
            let Some(final_event) = evidence.iter().find(|event| {
                event.provider_turn_id == start.provider_turn_id
                    && matches!(event.kind, CallbackEventKind::Final { .. })
                    && member
                        .native_delivery_callback_boundary
                        .is_some_and(|boundary| event.sequence > boundary)
                    && check_callback_identity(member, event).is_ok()
            }) else {
                continue;
            };
            let CallbackEventKind::Final { text } = &final_event.kind else {
                continue;
            };
            member.provider_session_id = start.provider_session_id.clone();
            member.provider_turn_id = start.provider_turn_id.clone();
            member.provider_prompt_id = start.provider_prompt_id.clone();
            member.trusted_start_bound = true;
            member.awaiting_background = false;
            member.pending_final = None;
            member.phase = RequestPhase::Completed;
            member.uncertain_outcome = false;
            member.submission_error = None;
            member.failure_reason = None;
            member.completed_at_ms = Some(final_event.occurred_at_ms);
            member.turn_ended_at_ms = Some(final_event.occurred_at_ms);
            member
                .latency
                .provider_started_at_ms
                .get_or_insert(start.occurred_at_ms);
            member
                .latency
                .reply_captured_at_ms
                .get_or_insert(final_event.occurred_at_ms);
            member
                .completed_reply
                .get_or_insert_with(|| CompletedReply {
                    content: PendingFinal {
                        callback_id: final_event.callback_id.clone(),
                        text: text.clone(),
                        received_at_ms: final_event.occurred_at_ms,
                        provider_session_id: final_event.provider_session_id.clone(),
                        provider_turn_id: final_event.provider_turn_id.clone(),
                    },
                    group: Some(*id),
                });
            self.consumed_provider_turns.insert(turn_key);
            for start in starts {
                self.record_reported_final(start.callback_id.clone(), start.launch_id.clone());
            }
            self.record_reported_final(
                final_event.callback_id.clone(),
                final_event.launch_id.clone(),
            );
        }
        // Already-answered members can no longer be settled with the lead's
        // reply. It is now safe to apply an original-turn Stop still on disk.
        if let Some(lead) = self.request(id).filter(|lead| {
            lead.trusted_start_bound && !lead.settled() && lead.pending_final.is_none()
        }) {
            if let Some(final_event) = evidence.iter().find(|event| {
                event.provider_turn_id == lead_turn
                    && matches!(event.kind, CallbackEventKind::Final { .. })
                    && check_callback_identity(lead, event).is_ok()
            }) {
                self.accept_callback(final_event.clone());
            }
        }
    }
}
