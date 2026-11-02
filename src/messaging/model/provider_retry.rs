//! A native terminal error pauses delivery; it does not transfer retry ownership to the sender.
use super::{BusState, Provider, RequestId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderRetry {
    pub(crate) lead: RequestId,
    pub(crate) attempts: u32,
    pub(crate) retry_at_ms: u64,
    pub(crate) quota: bool,
}

impl BusState {
    pub(super) fn hold_provider_error(&mut self, id: RequestId, error: &str, now_ms: u64) -> bool {
        let Some(request) = self.requests.get(&id) else {
            return false;
        };
        let Some(owner) = self.agents.get(&request.agent_id) else {
            return false;
        };
        if !request.trusted_start_bound || owner.deletion_pending {
            return false;
        }
        let Some(quota) = transient(owner.provider, error) else {
            return false;
        };
        if owner.provider_retry.is_some() {
            return true;
        }
        let agent = owner.id;
        let attempts = request.provider_retry_attempts.saturating_add(1);
        let delay = if quota {
            300_000
        } else {
            (5_000u64 << attempts.saturating_sub(1).min(6)).min(300_000)
        };
        let retry_at_ms = now_ms.saturating_add(delay).saturating_add(agent.0 % 1000);
        tracing::info!(event = "bus.provider.hold", agent_id = agent.0, request_id = id.0,
            prompt_id = request.prompt.id.0, room_id = request.room_id.0, provider = ?owner.provider,
            session_id = ?request.provider_session_id, turn_id = ?request.provider_turn_id,
            attempts, retry_at_ms, quota, error, "Provider failure held for retry");
        let owner = self.agents.get_mut(&agent).expect("held agent");
        owner.provider_retry = Some(ProviderRetry {
            lead: id,
            attempts,
            retry_at_ms,
            quota,
        });
        owner.actionable_error = None;
        self.requests
            .get_mut(&id)
            .expect("held request")
            .turn_ended_at_ms = Some(now_ms);
        true
    }
}

/// Only a provider-owned terminal failure enters this classifier, never assistant text.
fn transient(provider: Provider, error: &str) -> Option<bool> {
    let text = error.to_ascii_lowercase();
    if [
        "authentication",
        "unauthorized",
        "permission",
        "invalid",
        "not found",
        "cancel",
        "interrupt",
        "401",
        "403",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        return None;
    }
    let quota = text.contains("rate_limit") || text.contains("429");
    let overload = text.contains("overload") || text.contains("529");
    let connection = [
        "connection",
        "timeout",
        "timed out",
        "server_error",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|s| text.contains(s));
    match provider {
        Provider::Codex | Provider::ClaudeCode if quota || overload || connection => Some(quota),
        Provider::Grok | Provider::Kimi if quota || overload => Some(quota),
        _ => None,
    }
}
