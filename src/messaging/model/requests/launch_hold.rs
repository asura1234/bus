//! Queue stall while a provider launch or hook-consent wait is still viable.
use super::super::{AgentId, BusState, LaunchOutcome, AGENT_STARTUP_FAILED};

/// Consent and launch waits. They are not evidence the provider refused delivery.
///
/// Hook confirmation is checked before terminal and session identity, so those
/// later waits belong to an idle agent whose hooks are already confirmed.
fn launch_consent_wait(reason: &str) -> bool {
    matches!(reason, "hook_setup_unconfirmed" | "agent_launching")
}

impl BusState {
    /// Idle delivery failure, or nothing while a launch or consent wait is still viable.
    ///
    /// `hook_setup_unconfirmed` and `agent_launching` are not a refused submit.
    /// The queue keeps its position until the provider exits or the launch
    /// attempt is recorded as failed or cancelled. The startup watcher records
    /// an elapsed launch deadline as that failure; this stall must not invent
    /// one while the attempt is still pending. An idle agent with confirmed
    /// hooks still expires as `not_submitted`.
    pub(super) fn queued_stall_reason(&self, agent_id: AgentId) -> Option<String> {
        let (reason, exited) = {
            let agent = self.agents.get(&agent_id)?;
            let reason = agent
                .delivery_rejection
                .clone()
                .or_else(|| agent.wait_reason().map(str::to_owned))
                .unwrap_or_else(|| "not_submitted".into());
            (reason, agent.exit_snapshot.is_some())
        };
        if !launch_consent_wait(&reason) {
            return Some(reason);
        }
        if exited {
            return Some("provider_exited".into());
        }
        let attempt = self.current_launch_attempt(agent_id)?;
        match attempt.outcome {
            LaunchOutcome::Failed => Some(
                attempt
                    .cause
                    .clone()
                    .filter(|cause| !cause.is_empty())
                    .unwrap_or_else(|| AGENT_STARTUP_FAILED.into()),
            ),
            LaunchOutcome::Cancelled => Some("launch_cancelled".into()),
            LaunchOutcome::Pending | LaunchOutcome::Usable => None,
        }
    }
}
