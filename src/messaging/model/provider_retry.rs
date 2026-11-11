//! A native terminal error pauses delivery; it does not transfer retry ownership to the sender.
use super::{
    callbacks::provider_turn_key, AgentId, BusState, CallbackDisposition, CallbackEventKind,
    CallbackRejection, Provider, ProviderCallback, RequestId, RequestPhase, RuntimeStatus,
};
use serde::{Deserialize, Serialize};
mod reset;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProviderRetry {
    pub(crate) lead: RequestId,
    pub(crate) attempts: u32,
    pub(crate) retry_at_ms: u64,
    pub(crate) quota: bool,
    #[serde(default)]
    pub(crate) queued: bool,
}

impl BusState {
    pub(super) fn retain_rejected_provider_retry(&mut self, id: RequestId) {
        let Some(request) = self
            .requests
            .get(&id)
            .filter(|r| r.provider_retry_attempts > 0)
        else {
            return;
        };
        let Some(owner) = self.agents.get_mut(&request.agent_id) else {
            return;
        };
        owner.provider_retry = Some(ProviderRetry {
            lead: id,
            attempts: request.provider_retry_attempts,
            retry_at_ms: owner.observed_at_ms.saturating_add(5_000),
            quota: false,
            queued: true,
        });
    }

    pub(super) fn apply_error_callback(
        &mut self,
        request_id: RequestId,
        callback: ProviderCallback,
        turn_key: Option<String>,
    ) -> CallbackDisposition {
        let CallbackEventKind::Error {
            message,
            provider_error,
        } = callback.kind
        else {
            return CallbackDisposition::Rejected(CallbackRejection::WrongTurn);
        };
        let bound = self
            .requests
            .get(&request_id)
            .is_some_and(|r| r.trusted_start_bound);
        let message = match (bound, provider_error) {
            // A provider that names its error (Claude's StopFailure) closes the
            // turn with the same retryable reason settlement gives Codex's.
            (true, Some(error)) => {
                if self.hold_provider_error(request_id, &error, callback.occurred_at_ms) {
                    return CallbackDisposition::AcceptedError;
                }
                let (reason, message) = super::settlement::provider_error_outcome(&error);
                let at_ms = callback.occurred_at_ms;
                match self.close_turn_with_reason(request_id, at_ms, reason) {
                    Ok(closed) => closed.then_some(message),
                    Err(_) => {
                        return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest)
                    }
                }
            }
            (true, None) => {
                self.note_steered_provider_error(request_id, &message);
                if self
                    .abandon_current_request(request_id, callback.occurred_at_ms)
                    .is_err()
                {
                    return CallbackDisposition::Rejected(CallbackRejection::NoActiveRequest);
                }
                Some(message)
            }
            // The request stays open, so the error must not say it closed.
            (false, Some(error)) => Some(format!(
                "Provider turn failed: {error}; inspect its terminal."
            )),
            (false, None) => Some(message),
        };
        if bound {
            self.consumed_provider_turns.extend(turn_key);
        }
        let message = message.and_then(|message| self.request_failure_error(request_id, message));
        if let Some(agent) = self.agents.get_mut(&callback.agent_id) {
            if let Some(message) = message {
                agent.actionable_error = Some(message);
            }
        }
        CallbackDisposition::AcceptedError
    }

    pub(crate) fn defer_provider_retry(&mut self, agent: AgentId, reset_at_ms: u64) -> bool {
        let Some(owner) = self.agents.get_mut(&agent) else {
            return false;
        };
        let Some(hold) = owner.provider_retry.as_mut().filter(|h| h.quota) else {
            return false;
        };
        let due = reset_at_ms.saturating_add(2_000);
        if due <= hold.retry_at_ms {
            return false;
        }
        hold.retry_at_ms = due;
        true
    }

    /// A native queued prompt may start after the failed turn. Re-anchor only
    /// that exact owned input; the failed delivery remains independently held.
    pub(super) fn accept_retry_input(
        &mut self,
        callback: &ProviderCallback,
    ) -> Option<CallbackDisposition> {
        let owner = self.agents.get(&callback.agent_id)?;
        // Kimi reconciles committed failures before Started, including an
        // already-held failure. Keep its independent inputs on that one path.
        if owner.provider == Provider::Kimi {
            return None;
        }
        let hold = owner.provider_retry.as_ref()?;
        if owner.current_request.is_some_and(|id| id != hold.lead) {
            // Independently attributed native work keeps the ordinary continuation contract.
            return None;
        }
        let anchor = self.requests.get(&hold.lead)?;
        let current_turn = owner
            .current_request
            .and_then(|id| self.requests.get(&id))
            .and_then(|r| r.provider_turn_id.as_ref());
        if callback.provider_turn_id.as_ref() == current_turn {
            return None;
        }
        if !matches!(callback.kind, CallbackEventKind::PromptStarted)
            || anchor.expected_launch_id.as_deref() != Some(&callback.launch_id)
            || callback.provider_session_id != anchor.provider_session_id
            || anchor
                .submission_boundary
                .is_none_or(|at| callback.sequence <= at)
            || owner.current_request.is_some_and(|id| {
                id != hold.lead && self.requests.get(&id).is_some_and(|r| !r.settled())
            })
        {
            return Some(CallbackDisposition::Rejected(CallbackRejection::WrongTurn));
        }
        let member = self.group_members(hold.lead).into_iter().find(|id| {
            let r = &self.requests[id];
            !r.settled()
                && r.steered
                && !r.trusted_start_bound
                && callback
                    .prompt_payload
                    .as_deref()
                    .is_some_and(|text| r.matches_callback_payload(text, owner.provider))
        });
        let Some(member) = member else {
            return Some(CallbackDisposition::Rejected(CallbackRejection::WrongTurn));
        };
        self.retire_retry_turn(hold.lead);
        let request = self.requests.get_mut(&member)?;
        request.group = None;
        request.phase = RequestPhase::Active;
        request.provider_session_id = callback.provider_session_id.clone();
        request.provider_turn_id = callback.provider_turn_id.clone();
        request.provider_prompt_id = callback.provider_prompt_id.clone();
        request.trusted_start_bound = true;
        request.uncertain_outcome = false;
        request.submission_error = None;
        request.turn_ended_at_ms = None;
        request.progress_at_ms = Some(callback.occurred_at_ms);
        self.agents.get_mut(&callback.agent_id)?.current_request = Some(member);
        Some(CallbackDisposition::AcceptedSteering)
    }

    fn retire_retry_turn(&mut self, id: RequestId) {
        let Some(lead) = self.requests.get(&id) else {
            return;
        };
        if let Some(key) = provider_turn_key(
            lead.expected_launch_id.as_deref().unwrap_or_default(),
            lead.provider_session_id.as_deref(),
            lead.provider_turn_id.as_deref(),
        ) {
            self.retried_provider_turns.insert(key.clone());
            self.consumed_provider_turns.insert(key);
        }
    }

    fn prepare_retry_request(&mut self, id: RequestId, attempts: u32, now_ms: u64) {
        self.retire_retry_turn(id);
        let request = self.requests.get_mut(&id).expect("held member");
        request.phase = RequestPhase::Queued;
        request.provider_retry_attempts = request.provider_retry_attempts.max(attempts);
        request.expected_launch_id = None;
        request.submission_boundary = None;
        request.submission_status_revision = 0;
        request.provider_session_id = None;
        request.provider_turn_id = None;
        request.provider_prompt_id = None;
        request.trusted_start_bound = false;
        request.uncertain_outcome = false;
        request.submission_error = None;
        request.failure_reason = None;
        request.queued_rejection = None;
        request.pending_final = None;
        request.completed_at_ms = None;
        request.group = None;
        request.steered = false;
        request.submitted_payload = None;
        request.foreign_turn_settled_at_ms = None;
        request.progress_at_ms = Some(now_ms);
        request.turn_ended_at_ms = None;
        request.awaiting_background = false;
        request.latency.clear_attempt();
        let payload = self.render_request_payload(id).expect("retained prompt");
        self.requests.get_mut(&id).expect("held member").submitted_payload = Some(format!(
            "Continue the existing task after a transient provider failure; check completed effects before repeating actions.\n\n{payload}"
        ));
    }

    pub(crate) fn retry_provider_requests(&mut self, now_ms: u64) -> bool {
        let holds: Vec<_> = self
            .agents
            .values()
            .filter_map(|a| a.provider_retry.clone().map(|h| (a.id, h)))
            .collect();
        let mut changed = false;
        for (agent, hold) in holds {
            let owner = &self.agents[&agent];
            if owner.deletion_pending || owner.interrupt_pending {
                continue;
            }
            let mut pending = self.group_members(hold.lead);
            pending.insert(0, hold.lead);
            pending.retain(|id| self.requests.get(id).is_some_and(|r| !r.settled()));
            if pending.is_empty() {
                let owner = self.agents.get_mut(&agent).expect("held agent");
                owner.provider_retry = None;
                if owner.current_request == Some(hold.lead) {
                    owner.current_request = None;
                }
                changed = true;
                continue;
            }
            if hold.queued {
                continue;
            }
            let Some(lead) = self.requests.get(&hold.lead) else {
                continue;
            };
            if now_ms < hold.retry_at_ms
                || owner.status != RuntimeStatus::Idle
                || owner.dialog
                || owner.session_binding_invalidated
                || owner.missing_binary.is_some()
                || owner.working_directory_error.is_some()
                || owner.current_request.is_some_and(|id| id != hold.lead)
                || owner.runtime_identity.launch_id != lead.expected_launch_id
                || owner.runtime_identity.session_id != lead.provider_session_id
                || pending
                    .iter()
                    .any(|id| self.requests[id].steered && !self.requests[id].trusted_start_bound)
            {
                continue;
            }
            let next_lead = pending[0];
            for id in &pending {
                self.prepare_retry_request(*id, hold.attempts, now_ms);
            }
            if let Some(anchor) = self.requests.get_mut(&hold.lead).filter(|r| r.settled()) {
                anchor.phase = RequestPhase::Completed;
            }
            let queue = self.queues.entry(agent).or_default();
            queue.retain(|id| !pending.contains(id));
            queue.splice(0..0, pending);
            // Time spent in the provider hold is not a refused delivery opportunity.
            for id in self.queued_requests(agent).to_vec() {
                if let Some(request) = self.requests.get_mut(&id) {
                    request.progress_at_ms = Some(now_ms);
                    if let Some(rejected) = request.queued_rejection.as_mut() {
                        rejected.at_ms = now_ms;
                    }
                }
            }
            let owner = self.agents.get_mut(&agent).expect("held agent");
            owner.current_request = None;
            owner.provider_retry = Some(ProviderRetry {
                lead: next_lead,
                queued: true,
                ..hold
            });
            owner.delivery_rejection = None;
            changed = true;
        }
        changed
    }

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
        if request.settled()
            && self
                .group_members(id)
                .iter()
                .all(|member| self.requests[member].settled())
        {
            return false;
        }
        let Some(quota) = transient(owner.provider, error) else {
            if let Some(hold) = owner.provider_retry.clone().filter(|h| h.lead != id) {
                let agent = owner.id;
                self.retire_retry_turn(id);
                self.requests
                    .get_mut(&id)
                    .expect("owned native input")
                    .group = Some(hold.lead);
                let (reason, message) = super::settlement::provider_error_outcome(error);
                if self
                    .close_turn_with_reason(hold.lead, now_ms, reason)
                    .is_ok()
                {
                    self.agents
                        .get_mut(&agent)
                        .expect("held agent")
                        .actionable_error = Some(message);
                    return true;
                }
            }
            return false;
        };
        if owner.provider_retry.as_ref().is_some_and(|h| h.lead == id) {
            return true;
        }
        let agent = owner.id;
        if owner.interrupt_pending {
            return self.close_interrupted(agent, now_ms).is_ok();
        }
        let existing = owner.provider_retry.clone();
        let attempts = request
            .provider_retry_attempts
            .saturating_add(1)
            .max(existing.as_ref().map_or(0, |h| h.attempts));
        let backoff = (5_000u64 << attempts.saturating_sub(1).min(6)).min(300_000);
        let delay = if quota { 300_000 } else { backoff };
        let reset_at_ms = quota.then(|| reset::deadline(error, now_ms)).flatten();
        let retry_at_ms = reset_at_ms
            .map_or_else(
                || now_ms.saturating_add(delay),
                |at| at.saturating_add(2_000).max(now_ms.saturating_add(backoff)),
            )
            .saturating_add(agent.0 % 1000);
        tracing::info!(event = "bus.provider.hold", agent_id = agent.0, request_id = id.0,
            prompt_id = request.prompt.id.0, room_id = request.room_id.0, provider = ?owner.provider,
            session_id = ?request.provider_session_id, turn_id = ?request.provider_turn_id,
            attempts, retry_at_ms, reset_at_ms, quota, error, "Provider failure held for retry");
        let owner = self.agents.get_mut(&agent).expect("held agent");
        owner.provider_retry = Some(ProviderRetry {
            lead: existing.as_ref().map_or(id, |h| h.lead),
            attempts,
            retry_at_ms: existing
                .as_ref()
                .map_or(retry_at_ms, |h| h.retry_at_ms.max(retry_at_ms)),
            quota: quota || existing.as_ref().is_some_and(|h| h.quota),
            queued: false,
        });
        if let Some(hold) = existing {
            owner.current_request = Some(hold.lead);
            self.requests.get_mut(&id).expect("held native input").group = Some(hold.lead);
        }
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
    if provider == Provider::Kimi && crate::agents::providers::kimi::quota_exhausted(error) {
        return None;
    }
    let text = error.to_ascii_lowercase();
    // Kimi's native connection errors can wrap a fetch/Proxy AbortError.
    // That nested cancellation does not mean the user interrupted the turn.
    let kimi_connection = provider == Provider::Kimi
        && ["oauthconnectionerror:", "apiconnectionerror:"]
            .iter()
            .any(|category| text.starts_with(category));
    if text.contains("cancel") && !kimi_connection
        || [
            "authentication",
            "unauthorized",
            "permission",
            "invalid",
            "not found",
            "cancelled by user",
            "canceled by user",
            "user cancelled",
            "user canceled",
            "interrupt",
            "401",
            "403",
        ]
        .iter()
        .any(|s| text.contains(s))
    {
        return None;
    }
    let codex_quota = provider == Provider::Codex
        && [
            "usage_limit_reached",
            "usage_limit_exceeded",
            "usagelimitexceeded",
            "ratelimitexceeded",
        ]
        .iter()
        .any(|code| text.contains(code));
    let quota = text.contains("rate_limit") || text.contains("429") || codex_quota;
    let overload = text.contains("overload") || text.contains("529");
    let http_server = [
        "http ",
        "http status ",
        "status ",
        "status code ",
        "api error:",
    ]
    .iter()
    .filter_map(|&prefix| text.split_once(prefix))
    .map(|(_, tail)| {
        tail.trim_start()
            .split(|ch: char| !ch.is_ascii_digit())
            .next()
            .unwrap_or_default()
    })
    .any(|code| code.len() == 3 && code.starts_with('5'));
    let connection = http_server
        || [
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
        Provider::Codex | Provider::ClaudeCode | Provider::ClaudeDs
            if quota || overload || connection =>
        {
            Some(quota)
        }
        Provider::Grok | Provider::Kimi if quota || overload || connection => Some(quota),
        _ => None,
    }
}
