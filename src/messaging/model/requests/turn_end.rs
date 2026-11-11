//! Evidence that a provider turn has ended, independent of its reply.
use super::super::{BusState, Provider, Request, RequestPhase, RuntimeStatus};

impl BusState {
    /// Whether `request`'s agent took the message and finished its turn: the
    /// agent was seen Working or Blocked, or its provider reported the turn,
    /// after Bus submitted the message, and has transitioned to Idle since
    /// its latest provider progress. Re-reading the pre-Started Idle frame
    /// does not prove completion. A captured provider final also counts.
    pub(crate) fn turn_ended(&self, request: &Request) -> bool {
        if request.phase == RequestPhase::Completed {
            return true;
        }
        // Kimi can briefly paint an empty, idle composer while its bound
        // turn is still executing tools. Only its owned terminal wire (or a
        // captured reply) can establish that the turn actually finished.
        if self
            .agents
            .get(&request.agent_id)
            .is_some_and(|agent| agent.provider == Provider::Kimi)
        {
            let lead = request
                .group
                .and_then(|lead| self.requests.get(&lead))
                .unwrap_or(request);
            return lead.trusted_start_bound && lead.turn_ended_at_ms.is_some();
        }
        // An uncertain write without a matching provider start may be a
        // startup dialog or unrelated activity, not work on this message.
        if request.uncertain_outcome && !request.trusted_start_bound {
            return false;
        }
        // Background work without a captured answer will wake
        // the turn with the real reply, for its joined messages too.
        let lead = request
            .group
            .and_then(|lead| self.requests.get(&lead))
            .unwrap_or(request);
        if lead.awaiting_background {
            return false;
        }
        // Screen activity alone cannot confirm an uncertain submission;
        // a confirmed write may finish without a captured start hook.
        if !matches!(
            request.phase,
            RequestPhase::Submitting | RequestPhase::Active
        ) {
            return false;
        }
        // A visible dialog (Codex reports Idle while it waits on an approval)
        // means the turn is waiting on an answer, not finished.
        self.agents.get(&request.agent_id).is_some_and(|agent| {
            // A retry hold was opened by an owned terminal error, not an
            // inferred Idle frame. Keep that committed end authoritative.
            let held_end = request.turn_ended_at_ms.is_some()
                && agent
                    .provider_retry
                    .as_ref()
                    .is_some_and(|hold| hold.lead == lead.id);
            !agent.dialog
                && (held_end
                    || agent.status != RuntimeStatus::Idle
                    || !lead.trusted_start_bound
                    || lead
                        .progress_at_ms
                        .is_some_and(|progress| agent.status_since_ms > progress))
                && (request.turn_ended_at_ms.is_some()
                    || (agent.busy_revision > request.submission_status_revision
                        && agent.status == RuntimeStatus::Idle
                        && agent.status_revision > agent.busy_revision))
        })
    }
}
