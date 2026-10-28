//! A definite refusal has its own durable clock, independent of provider work.
use super::{
    rejection_reason, BusState, Request, RequestId, RequestPhase, SubmissionOutcome,
    QUEUED_STALL_MS,
};

impl BusState {
    pub(super) fn record_queue_rejection(&mut self, id: RequestId, outcome: &SubmissionOutcome) {
        let SubmissionOutcome::DefinitelyRejected { message } = outcome else {
            return;
        };
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
                    crate::messaging::model::types::QueuedRejection {
                        at_ms,
                        reason: rejection_reason(message).into(),
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
        (now_ms.saturating_sub(rejected.at_ms) >= QUEUED_STALL_MS)
            .then_some(rejected.reason.as_str())
    }
}
