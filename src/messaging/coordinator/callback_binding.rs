//! Payload-free binding facts alongside correlation and deferred-stop events.
use super::{launch, schema, AgentId, BusState, CallbackEventKind, Method, ProviderCallback};
use crate::agents::providers::spool;

use super::provider_turns::reporting_processes;

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
            .and_then(|payload| agent.map(|agent| request.matches_callback_payload(payload, agent.provider)))),
        compactions = agent.map_or(0, |agent| agent.compactions.count),
    )
}

impl super::Worker {
    pub(super) fn bind_session_callback(
        &mut self,
        id: AgentId,
        state: &mut BusState,
        record: &spool::Record,
        session: String,
        source: Option<String>,
    ) -> Result<(), String> {
        let agent = state.agent(id).ok_or("Unknown callback agent")?.clone();
        let replacement = agent.provider.uses_claude_harness()
            && source.as_deref() == Some("startup")
            && agent.superseded_session_id.is_some()
            && agent.runtime_identity.session_id.is_none();
        let mut identity = agent.runtime_identity.clone();
        let pane = identity
            .pane_id
            .clone()
            .ok_or("Callback arrived before pane creation was recorded")?;
        let report = Method::PaneReportAgentSession(schema::PaneReportAgentSessionParams {
            pane_id: pane,
            // Use the existing provider adapter namespace:
            // a generic source is deliberately rejected by
            // the native session identity consumer.
            source: format!("herdr:{}", launch::provider_kind(agent.provider)),
            agent: launch::provider_kind(agent.provider).into(),
            seq: Some(record.sequence),
            agent_session_id: Some(session.clone()),
            agent_session_path: None,
            session_start_source: Some("startup".into()),
            reporter: reporting_processes(record),
        });
        state
            .set_agent_startup(id, None)
            .map_err(|e| e.to_string())?;
        identity.session_id = Some(session.clone());
        state
            .set_agent_runtime_identity(id, identity)
            .map_err(|e| e.to_string())?;
        if agent.session_reset_pending {
            // A reset agent that had no session yet binds its first one here.
            state
                .rebind_reset_session(id, session)
                .map_err(|e| e.to_string())?;
        }
        if record.manifest.provider.uses_claude_harness() && source.as_deref() == Some("startup") {
            state
                .initialize_claude_shells(id)
                .map_err(|e| e.to_string())?;
        }
        // Identical hooks share one spool file, so a hook retry counts once.
        if source.as_deref() == Some("compact") {
            state
                .record_compaction_callback(
                    id,
                    &record.manifest.launch_id,
                    record.sequence,
                    record.at_ms,
                )
                .map_err(|e| e.to_string())?;
        }
        if agent.current_request.is_none() {
            state
                .set_agent_error(
                    id,
                    if agent.hook_setup_confirmed {
                        None
                    } else {
                        Some(crate::messaging::model::AGENT_WAITING_FOR_TERMINAL.into())
                    },
                )
                .map_err(|e| e.to_string())?;
        }
        if replacement {
            self.save_callback_state(state.clone())?;
        }
        self.transport.request(report).map_err(|e| e.message)?;
        Ok(())
    }

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
