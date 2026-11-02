//! A known native quota failure pauses delivery, but must leave a guarded recovery probe.
use super::{io, schema, Provider, RoomAgent, RuntimeStatus, Worker};
use crate::agents::providers::kimi::usage;
use crate::messaging::model::AgentRuntimeIdentity;
use std::collections::BTreeMap;

// Probe before the normal two-minute unattempted-queue deadline. This is a
// delivery backoff, not an inferred reset time for Kimi's five-hour window.
const RETRY_AFTER_MS: u64 = 60_000;

pub(super) struct Retry {
    identity: AgentRuntimeIdentity,
    next_attempt_ms: u64,
}

impl Worker {
    fn kimi_quota_now_ms(&self) -> u64 {
        #[cfg(test)]
        if let Some(now) = self.test_kimi_quota_now_ms {
            return now;
        }
        io::now_ms()
    }

    pub(super) fn observe_kimi_quotas(&mut self, infos: &[schema::AgentInfo]) {
        let now = self.kimi_quota_now_ms();
        let limited: BTreeMap<_, _> = self
            .state
            .agents()
            .filter(|agent| agent.provider == Provider::Kimi)
            .filter(|agent| infos.iter().any(|info| matches_quota(agent, info)))
            .map(|agent| (agent.id, agent.runtime_identity.clone()))
            .collect();
        self.kimi_quota_retries
            .retain(|id, retry| limited.get(id) == Some(&retry.identity));
        for (id, identity) in limited {
            self.kimi_quota_retries.entry(id).or_insert_with(|| {
                tracing::info!(
                    event = "bus.kimi.quota_limited",
                    agent_id = id.0,
                    retry_after_ms = RETRY_AFTER_MS,
                    "Kimi quota holds delivery until a guarded queued probe"
                );
                Retry {
                    identity,
                    next_attempt_ms: now.saturating_add(RETRY_AFTER_MS),
                }
            });
        }
    }

    pub(super) fn take_kimi_quota_probe(&mut self, agent: &RoomAgent) -> bool {
        if agent.provider != Provider::Kimi
            || agent.status != RuntimeStatus::Unavailable
            || agent.missing_binary.is_some()
            || self.state.next_queued_request(agent.id).is_none()
        {
            return false;
        }
        let now = self.kimi_quota_now_ms();
        let Some(retry) = self.kimi_quota_retries.get_mut(&agent.id).filter(|retry| {
            retry.identity == agent.runtime_identity && now >= retry.next_attempt_ms
        }) else {
            return false;
        };
        retry.next_attempt_ms = now.saturating_add(RETRY_AFTER_MS);
        tracing::info!(
            event = "bus.kimi.quota_probe",
            agent_id = agent.id.0,
            "Attempting queued Kimi work through the normal guarded native submit"
        );
        true
    }
}

fn matches_quota(agent: &RoomAgent, info: &schema::AgentInfo) -> bool {
    info.agent.as_deref() == Some("kimi")
        && info.interactive_ready
        && !info.launch_pending
        && agent.runtime_identity.terminal_id.as_deref() == Some(info.terminal_id.as_str())
        && agent.runtime_identity.pane_id.as_deref() == Some(info.pane_id.as_str())
        && agent
            .runtime_identity
            .session_id
            .as_deref()
            .is_none_or(|session| {
                info.agent_session
                    .as_ref()
                    .is_some_and(|identity| identity.value == session)
            })
        && info
            .provider_error
            .as_deref()
            .is_some_and(usage::is_quota_error)
}

#[cfg(test)]
#[path = "tests/kimi_quota_test.rs"]
mod tests;
