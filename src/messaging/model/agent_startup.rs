//! Startup recovery and closure of never-submitted queued work.
use super::{AgentId, BusState, ModelError, RequestPhase};

impl BusState {
    pub(crate) fn set_agent_startup(
        &mut self,
        id: AgentId,
        startup: Option<super::AgentStartup>,
    ) -> Result<(), ModelError> {
        let agent = self
            .agents
            .get_mut(&id)
            .ok_or(ModelError::UnknownAgent(id))?;
        if startup.is_none()
            && agent
                .startup
                .as_ref()
                .is_some_and(|s| s.reported || s.recovery.is_some())
        {
            // Like closing a dialog, readiness gives held work a fresh idle
            // interval even if the runtime remained Idle throughout the hold.
            agent.status_since_ms = agent.status_since_ms.max(agent.observed_at_ms);
        }
        if startup.is_none() && agent.startup.is_some() {
            // Startup snapshots remain as files. The now-running provider must
            // get its own fresh snapshot if it exits later on this same terminal.
            agent.exit_snapshot = None;
        }
        let attempts = startup
            .as_ref()
            .or(agent.startup.as_ref())
            .and_then(|s| match &s.recovery {
                Some(super::StartupRecovery::CodexBootstrap { attempts, .. }) => Some(*attempts),
                _ => None,
            })
            .unwrap_or(0);
        agent.startup = startup;
        for request in self.queues.get(&id).into_iter().flatten() {
            if let Some(request) = self.requests.get_mut(request) {
                let added = attempts.saturating_sub(request.startup_retry_attempts);
                request.provider_retry_attempts =
                    request.provider_retry_attempts.saturating_add(added);
                request.startup_retry_attempts = request.startup_retry_attempts.max(attempts);
            }
        }
        Ok(())
    }

    /// Close sender-authored work that never reached provider input, preserving its diagnosis.
    pub(crate) fn abandon_failed_startup_queue(&mut self, id: AgentId, now_ms: u64) {
        let Some(failure) = self
            .agents
            .get(&id)
            .and_then(|agent| agent.startup.as_ref())
            .and_then(|startup| startup.failure.as_ref())
        else {
            return;
        };
        let detail = failure.notice();
        let reason = if self
            .agents
            .get(&id)
            .and_then(|a| a.startup.as_ref())
            .is_some_and(|s| {
                matches!(
                    s.recovery,
                    Some(super::StartupRecovery::CodexBootstrap {
                        exhausted: true,
                        ..
                    })
                )
            }) {
            super::CODEX_BOOTSTRAP_FAILED
        } else {
            super::AGENT_STARTUP_FAILED
        };
        let continuations: Vec<_> = self
            .queued_requests(id)
            .iter()
            .copied()
            .filter(|id| {
                self.requests
                    .get(id)
                    .is_some_and(|r| r.restart_continuation)
            })
            .collect();
        for request in continuations {
            let members: Vec<_> = std::iter::once(request)
                .chain(self.group_members(request))
                .collect();
            let _ = self.recover_idle_request(request, now_ms);
            for member in members {
                if let Some(request) = self
                    .requests
                    .get_mut(&member)
                    .filter(|r| r.reply().is_none())
                {
                    request.submission_error = Some(detail.clone());
                }
            }
            if let Some(queue) = self.queues.get_mut(&id) {
                queue.retain(|id| *id != request);
            }
        }
        if let Some(queue) = self.queues.get_mut(&id) {
            queue.retain(|request_id| {
                let request = self
                    .requests
                    .get_mut(request_id)
                    .expect("queued request exists");
                if request.bus_generated {
                    return true;
                }
                request.phase = RequestPhase::Abandoned;
                request.failure_reason = Some(reason.into());
                request.submission_error = Some(detail.clone());
                request.completed_at_ms = Some(now_ms);
                false
            });
        }
    }
}
