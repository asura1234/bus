//! Reconcile an owned Kimi failure before the next native turn binds.
use super::{check_callback_identity, BusState, CallbackEventKind, ProviderCallback};
use crate::messaging::model::{Provider, RecordedTurnEnd};

impl BusState {
    /// Consume the old owned error before a later Started can replace its
    /// turn. Only an attested submitted member escapes the failed group.
    pub(crate) fn settle_kimi_failure_before_start(
        &mut self,
        callback: &ProviderCallback,
        error: String,
    ) {
        let Some(owner) = self.agents.get(&callback.agent_id) else {
            return;
        };
        if owner.provider != Provider::Kimi
            || !matches!(callback.kind, CallbackEventKind::PromptStarted)
        {
            return;
        }
        let Some(lead) = owner.current_request.and_then(|id| self.requests.get(&id)) else {
            return;
        };
        if !lead.trusted_start_bound
            || check_callback_identity(lead, callback).is_err()
            || callback.provider_turn_id.is_none()
            || callback.provider_turn_id == lead.provider_turn_id
        {
            return;
        }
        let boundary = lead.submission_boundary;
        let status_revision = owner.status_revision;
        let already_held = owner
            .provider_retry
            .as_ref()
            .is_some_and(|hold| hold.lead == lead.id);
        let lead = lead.id;
        let next = self.group_members(lead).into_iter().find(|id| {
            self.requests.get(id).is_some_and(|member| {
                member.steered
                    && !member.trusted_start_bound
                    && !member.settled()
                    && check_callback_identity(member, callback).is_ok()
                    && callback.prompt_payload.as_deref().is_some_and(|payload| {
                        member.matches_callback_payload(payload, Provider::Kimi)
                    })
            })
        });
        // A legacy group can contain more submitted inputs whose native
        // Started events have not arrived yet. Keep them with the new anchor.
        let pending: Vec<_> = self
            .group_members(lead)
            .into_iter()
            .filter(|id| {
                next.is_some()
                    && self.requests.get(id).is_some_and(|member| {
                        member.steered && !member.trusted_start_bound && !member.settled()
                    })
            })
            .collect();
        for id in &pending {
            if let Some(member) = self.requests.get_mut(id) {
                member.group = next.filter(|next| next != id);
            }
        }
        let settled = if already_held {
            Some(lead)
        } else {
            self.settle_ended_request(callback.agent_id, callback.occurred_at_ms, |_| {
                Some(RecordedTurnEnd::Error(error))
            })
        };
        if settled.is_some() {
            for id in pending {
                if let Some(member) = self.requests.get_mut(&id) {
                    member.submission_boundary = boundary;
                    member.submission_status_revision = status_revision;
                }
            }
            if let Some(owner) = self.agents.get_mut(&callback.agent_id) {
                owner.current_request = next;
            }
        } else {
            for id in pending {
                if let Some(member) = self.requests.get_mut(&id) {
                    member.group = Some(lead);
                }
            }
        }
    }
}
