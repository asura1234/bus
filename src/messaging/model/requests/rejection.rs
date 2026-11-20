//! Track definite composer refusals and when the affected queue entries expire.
use super::super::Provider;
use super::{
    rejection_reason, BusState, Request, RequestId, RequestPhase, RuntimeStatus, SubmissionOutcome,
    QUEUED_STALL_MS,
};

impl BusState {
    pub(super) fn record_queue_rejection(&mut self, id: RequestId, outcome: &SubmissionOutcome) {
        let SubmissionOutcome::DefinitelyRejected { message } = outcome else {
            return;
        };
        // A turn/dialog/startup race is a legitimate wait. Only the observed
        // composer failure needs a deadline independent of unrelated work.
        // A not-idle refusal is remembered so the stall cannot rename it
        // `not_submitted` after the agent-level rejection is cleared.
        let reason = rejection_reason(message);
        if reason != "prompt_not_shown" && reason != "agent_not_idle" {
            return;
        }
        let Some(request) = self.requests.get(&id) else {
            return;
        };
        let at_ms = self
            .agent(request.agent_id)
            .map_or(request.prompt.submitted_at_ms, |agent| {
                agent.observed_at_ms.max(request.prompt.submitted_at_ms)
            });
        let members = self.group_members(id);
        for id in std::iter::once(id).chain(members) {
            if let Some(request) = self.requests.get_mut(&id) {
                request.queued_rejection.get_or_insert_with(|| {
                    crate::messaging::model::QueuedRejection {
                        at_ms,
                        reason: reason.into(),
                    }
                });
            }
        }
    }

    pub(crate) fn expired_queue_rejection<'a>(
        &self,
        request: &'a Request,
        now_ms: u64,
    ) -> Option<&'a str> {
        if request.phase != RequestPhase::Queued
            || self
                .agent(request.agent_id)
                .is_some_and(|agent| agent.delivery_rejection.as_deref() == Some("composer_busy"))
        {
            return None;
        }
        let rejected = request.queued_rejection.as_ref()?;
        // Busy means "try again when idle", not "the send failed". The latest
        // agent-level refusal wins over an older composer deadline. Close only
        // once the target is gone, and name that refusal.
        let delivery_not_idle = self
            .agent(request.agent_id)
            .is_some_and(|agent| agent.delivery_rejection.as_deref() == Some("agent_not_idle"));
        if rejected.reason == "agent_not_idle" || delivery_not_idle {
            let alive = self
                .agent(request.agent_id)
                .is_some_and(|agent| agent.exit_snapshot.is_none() && !agent.deletion_pending);
            return (!alive).then_some("agent_not_idle");
        }
        let mut since = rejected.at_ms;
        if let Some(agent) = self
            .agent(request.agent_id)
            .filter(|agent| agent.provider == Provider::Agy)
        {
            // A refused AGY follow-up can wait for the next turn. Give it a
            // fresh delivery window after the live turn or dialog releases it.
            if matches!(
                agent.activity_status(),
                RuntimeStatus::Working | RuntimeStatus::Blocked
            ) || agent.current_request.is_some_and(|id| id != request.id)
            {
                return None;
            }
            since = since
                .max(agent.status_since_ms)
                .max(self.last_progress_ms(agent.id));
        }
        (now_ms.saturating_sub(since) >= QUEUED_STALL_MS).then_some(rejected.reason.as_str())
    }
}
