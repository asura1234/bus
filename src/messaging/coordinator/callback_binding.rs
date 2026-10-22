//! Payload-free binding facts alongside correlation and deferred-stop events.
use super::{BusState, CallbackEventKind, ProviderCallback};

pub(super) fn span(state: &BusState, callback: &ProviderCallback) -> tracing::Span {
    let agent = state.agent(callback.agent_id);
    let request = agent
        .and_then(|agent| agent.current_request)
        .and_then(|id| state.request(id));
    // Never format the callback or request: both carry private prompt/reply text.
    // This is a raw comparison; model correlation may additionally unwrap an
    // owned Claude paste or match a steered group member.
    tracing::debug_span!(
        "bus.callback.binding",
        kind = match callback.kind {
            CallbackEventKind::PromptStarted => "started",
            CallbackEventKind::Final { .. } => "final",
            CallbackEventKind::BackgroundPending => "background_pending",
            CallbackEventKind::Error { .. } => "error",
        },
        trusted_start_bound = request.is_some_and(|request| request.trusted_start_bound),
        submission_boundary = ?request.and_then(|request| request.submission_boundary),
        expected_session_id = ?request.and_then(|request| request.provider_session_id.as_deref()),
        callback_session_id = ?callback.provider_session_id,
        expected_turn_id = ?request.and_then(|request| request.provider_turn_id.as_deref()),
        callback_turn_id = ?callback.provider_turn_id,
        expected_prompt_id = ?request.and_then(|request| request.provider_prompt_id.as_deref()),
        callback_prompt_id = ?callback.provider_prompt_id,
        raw_payload_matches = ?request.and_then(|request| callback.prompt_payload.as_deref()
            .map(|payload| request.matches_callback_payload(payload))),
        compactions = agent.map_or(0, |agent| agent.compactions.count),
    )
}

impl super::Worker {
    /// The provider session the native server attributes to `pane`.
    pub(super) fn native_session(&mut self, pane: &str) -> Result<Option<String>, String> {
        match self
            .transport
            .request(super::Method::AgentGet(super::schema::AgentTarget {
                target: pane.to_owned(),
            }))
            .map_err(|e| e.message)?
        {
            super::ResponseResult::AgentInfo { agent } => Ok(agent.agent_session.map(|s| s.value)),
            _ => Err("Unexpected native agent response".into()),
        }
    }
}
