//! Bound idle settlement without treating a stale screen as an end.
use super::{
    AgentId, BusState, RecordedTurnEnd, Request, RequestId, RequestPhase, RuntimeStatus,
    TURN_SETTLE_MS,
};
use crate::messaging::model::{types::BoundTurnActivity, Provider};

// Larger than the full 17.9s measured real Codex resume, and the subsecond
// stale Idle gaps. Other providers keep their normal Working-to-Idle handling.
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
                codex_progress_at_ms: None,
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
        let inferred_end = (owner.provider == Provider::Codex)
            .then(|| self.settle_candidate(agent))
            .flatten()
            .and_then(|candidate| candidate.turn_ended_at_ms);
        let deadline = if let Some(ended_at) = inferred_end {
            ended_at.saturating_add(TURN_SETTLE_MS)
        } else if activity.working_seen {
            return None;
        } else {
            activity
                .idle_since_ms?
                .max(owner.status_since_ms)
                .max(activity.started_at_ms)
                .saturating_add(BOUND_IDLE_MS)
        };
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
        let recorded = recorded_end(request);
        if recorded.is_none() && self.agents.get(&agent)?.provider == Provider::Codex {
            let progress = codex_progress_at(request);
            // A fresh Working-to-Idle edge with no unanswered requests can
            // close after the usual callback grace without a Codex final.
            let latest_progress = request
                .bound_turn_activity
                .as_ref()
                .and_then(|activity| activity.codex_progress_at_ms)
                .max(progress.filter(|at| *at <= now_ms));
            if request.reply().is_some()
                && self
                    .settle_candidate(agent)
                    .and_then(|lead| lead.turn_ended_at_ms)
                    .is_some_and(|ended_at| latest_progress.is_none_or(|at| at <= ended_at))
                && self
                    .group_members(id)
                    .into_iter()
                    .all(|member| self.requests.get(&member).is_none_or(Request::settled))
            {
                return self.complete_silent_turn(agent, id, now_ms);
            }
            if !self.codex_idle_evidence_wait_elapsed(id, agent, now_ms, progress) {
                return None;
            }
        }
        self.settle_idle_bound_turn(agent, id, now_ms, recorded)
    }

    fn codex_idle_evidence_wait_elapsed(
        &mut self,
        id: RequestId,
        agent: AgentId,
        now_ms: u64,
        progress: Option<u64>,
    ) -> bool {
        let Some(owner) = self.agents.get(&agent) else {
            return false;
        };
        let Some(activity) = self
            .requests
            .get_mut(&id)
            .and_then(|request| request.bound_turn_activity.as_mut())
        else {
            return false;
        };
        let Some(idle_since) = activity.idle_since_ms else {
            return false;
        };
        activity.codex_progress_at_ms = activity
            .codex_progress_at_ms
            .max(progress.filter(|at| *at <= now_ms));
        let since = idle_since
            .max(owner.status_since_ms)
            .max(activity.started_at_ms)
            .max(activity.codex_progress_at_ms.unwrap_or(0));
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
        let codex = self.agents.get(&agent)?.provider == Provider::Codex;
        if codex {
            self.requests.get_mut(&id)?.turn_ended_at_ms = None;
        }
        for id in unanswered {
            self.requests.get_mut(&id)?.uncertain_outcome = true;
        }
        if let Some(owner) = self.agents.get_mut(&agent) {
            let elapsed = if codex {
                "the Codex liveness window"
            } else {
                "30 seconds of Idle with no accepted final or Working observation after Started"
            };
            owner.actionable_error = Some(format!("Turn end unconfirmed after {elapsed}. Provider records did not prove completion; unanswered requests have an uncertain outcome. Queue released. Inspect its terminal before resending."));
        }
        Some(id)
    }
}
