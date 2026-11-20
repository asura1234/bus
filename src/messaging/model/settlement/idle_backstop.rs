//! Bound a never-Working turn without treating a stale screen as an end.
use super::{AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase, RuntimeStatus};
use crate::messaging::model::{types::BoundTurnActivity, Provider};

// Larger than the full 17.9s measured real Codex resume, and the subsecond
// stale Idle gaps. A real Working observation disables this bound entirely.
const BOUND_IDLE_MS: u64 = 30_000;
const CODEX_IDLE_EVIDENCE_WAIT_MS: u64 = 300_000;

impl BusState {
    pub(crate) fn settle_ended_request(
        &mut self,
        agent: AgentId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        self.settle_ended_request_with_codex_progress(agent, now_ms, recorded_end, |_| None)
    }

    pub(crate) fn observe_bound_turn_activity(
        &mut self,
        agent: AgentId,
        status: RuntimeStatus,
        now_ms: u64,
    ) {
        let Some(owner) = self.agents.get(&agent) else {
            return;
        };
        let Some(id) = owner.current_request else {
            return;
        };
        let Some(request) = self.requests.get(&id).filter(|r| {
            r.trusted_start_bound
                && matches!(r.phase, RequestPhase::Active | RequestPhase::Submitting)
                && r.pending_final.is_none()
        }) else {
            return;
        };
        // Legacy states get a fresh window, never an immediate timeout based
        // on unknown pre-reload activity. Ordinary polls do not copy requests.
        let mut activity = request
            .bound_turn_activity
            .clone()
            .unwrap_or(BoundTurnActivity {
                started_at_ms: now_ms,
                idle_since_ms: None,
                working_seen: false,
            });
        if status == RuntimeStatus::Working {
            activity.working_seen = true;
        }
        if status == RuntimeStatus::Idle && !owner.dialog {
            activity
                .idle_since_ms
                .get_or_insert(now_ms.max(activity.started_at_ms));
        } else {
            activity.idle_since_ms = None;
        }
        if request.bound_turn_activity.as_ref() != Some(&activity) {
            let Some(request) = self.requests.get_mut(&id) else {
                return;
            };
            request.bound_turn_activity = Some(activity);
        }
    }

    pub(super) fn idle_bound_turn_candidate(
        &self,
        agent: AgentId,
        now_ms: u64,
    ) -> Option<RequestId> {
        let owner = self.agents.get(&agent)?;
        if owner.provider == Provider::Agy
            || owner.status != RuntimeStatus::Idle
            || owner.working_directory_error.is_some()
            || owner.dialog
            || owner.deletion_pending
            || owner.session_binding_invalidated
            || owner.missing_binary.is_some()
            || owner
                .provider_retry
                .as_ref()
                .is_some_and(|h| owner.current_request == Some(h.lead))
        {
            return None;
        }
        let request = self.requests.get(&owner.current_request?)?;
        if request.restart_resume.is_some()
            || !request.trusted_start_bound
            || request.awaiting_background
            || request.pending_final.is_some()
            || !matches!(
                request.phase,
                RequestPhase::Active | RequestPhase::Submitting
            )
        {
            return None;
        }
        let activity = request.bound_turn_activity.as_ref()?;
        if activity.working_seen {
            return None;
        }
        let since = activity
            .idle_since_ms?
            .max(owner.status_since_ms)
            .max(activity.started_at_ms);
        let deadline = since.saturating_add(BOUND_IDLE_MS);
        (now_ms >= deadline && owner.observed_at_ms >= deadline).then_some(request.id)
    }

    pub(super) fn reconcile_idle_bound_turn(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded_end: impl FnOnce(&Request) -> Option<RecordedTurnEnd>,
        codex_progress_at: impl FnOnce(&Request) -> Option<u64>,
    ) -> Option<RequestId> {
        let request = self.requests.get(&id)?;
        let codex = self.agents.get(&agent)?.provider == Provider::Codex;
        let normal_boundary = codex && self.settle_candidate(agent).is_some();
        let progress = if codex && !normal_boundary {
            codex_progress_at(request)
        } else {
            None
        };
        let recorded = recorded_end(request);
        if recorded.is_none() && codex {
            if normal_boundary {
                return self.settle_recorded_boundary(agent, now_ms, |_| None);
            }
            if !self.codex_idle_evidence_wait_elapsed(request, now_ms, progress) {
                return None;
            }
        }
        self.settle_idle_bound_turn(agent, id, now_ms, recorded)
    }

    fn codex_idle_evidence_wait_elapsed(
        &self,
        request: &Request,
        now_ms: u64,
        progress: Option<u64>,
    ) -> bool {
        let Some(owner) = self.agents.get(&request.agent_id) else {
            return false;
        };
        let Some(activity) = &request.bound_turn_activity else {
            return false;
        };
        let Some(idle_since) = activity.idle_since_ms else {
            return false;
        };
        let since = idle_since
            .max(owner.status_since_ms)
            .max(activity.started_at_ms)
            .max(progress.filter(|at| *at <= now_ms).unwrap_or(0));
        let deadline = since.saturating_add(CODEX_IDLE_EVIDENCE_WAIT_MS);
        now_ms >= deadline && owner.observed_at_ms >= deadline
    }

    pub(super) fn settle_idle_bound_turn(
        &mut self,
        agent: AgentId,
        id: RequestId,
        now_ms: u64,
        recorded: Option<RecordedTurnEnd>,
    ) -> Option<RequestId> {
        if recorded.is_some() {
            return self.consume_recorded_end(
                agent,
                id,
                now_ms,
                recorded,
                "idle-record",
                false,
                "",
                None,
            );
        }
        // A deadline is not proof the provider finished. Preserve explicit
        // replies and close only unanswered members with an uncertain result.
        let mut unanswered = self.group_members(id);
        unanswered.push(id);
        unanswered.retain(|id| self.requests.get(id).is_some_and(|r| !r.settled()));
        self.close_turn_with_reason(id, now_ms, "turn_end_unconfirmed".into())
            .ok()?;
        for id in unanswered {
            self.requests.get_mut(&id)?.uncertain_outcome = true;
        }
        if let Some(owner) = self.agents.get_mut(&agent) {
            let elapsed = if owner.provider == Provider::Codex {
                "the Codex liveness window"
            } else {
                "30 seconds of Idle"
            };
            owner.actionable_error = Some(format!("Turn end unconfirmed after {elapsed} with no accepted final or Working observation after Started. Provider records did not prove completion; unanswered requests have an uncertain outcome. Queue released. Inspect its terminal before resending."));
        }
        Some(id)
    }
}
