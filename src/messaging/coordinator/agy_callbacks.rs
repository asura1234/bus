//! Bounded durable retries and transcript-backed completion settlement for AGY workers.
use super::{
    callback, callbacks,
    grok_callbacks::{claim_with_policy, observation_paths_with_ext, Claim},
    AgentId, BusState, CallbackEventKind, CallbackRecordResult, ControlFlow, Path,
    ProviderCallback, Worker,
};
use crate::agents::providers::agy::{
    completion::{self as agy_completion, TurnResolution},
    hooks::{self as agy_hooks, StopVerdict},
};
use std::path::PathBuf;

const AGY_RETRY_EXT: &str = "agy-retry";
const AGY_RETRY_ATTEMPTS: u8 = 16;
const AGY_RETRY_DELAY_MS: u64 = 1000;

fn agy_observation_paths(path: &Path) -> Vec<PathBuf> {
    observation_paths_with_ext(path, AGY_RETRY_EXT)
}

impl Worker {
    fn owned_agy_turn(
        state: &BusState,
        id: AgentId,
        record: &callbacks::Record,
        session: &str,
    ) -> Option<String> {
        state
            .agent(id)
            .and_then(|a| a.current_request)
            .and_then(|r| state.request(r))
            .filter(|r| {
                r.trusted_start_bound
                    && r.expected_launch_id.as_deref() == Some(record.manifest.launch_id.as_str())
                    && r.provider_session_id.as_deref() == Some(session)
            })
            .and_then(|r| r.provider_turn_id.clone())
    }

    pub(super) fn committed_agy_completion_callback(
        &self,
        id: AgentId,
        state: &mut BusState,
        record: &callbacks::Record,
        session: &str,
    ) -> Result<Option<ProviderCallback>, String> {
        let Some(turn) = Self::owned_agy_turn(state, id, record, session) else {
            return Ok(None);
        };
        let Some(raw_path) = record
            .value
            .get("transcriptPath")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            tracing::debug!(
                event = "bus.callback.deferred",
                reason = "agy_transcript_missing",
                agent_id = id.0,
                session_id = %session,
                turn_id = %turn,
                "AGY callback lacks transcriptPath"
            );
            return Ok(None);
        };
        let transcript = match agy_completion::resolve_transcript_path(raw_path, session) {
            Ok(path) => path,
            Err(error) => {
                tracing::warn!(
                    event = "bus.callback.rejected",
                    reason = "agy_transcript_invalid_path",
                    agent_id = id.0,
                    session_id = %session,
                    turn_id = %turn,
                    %error,
                    "AGY transcriptPath validation failed"
                );
                state
                    .set_agent_error(id, Some(error))
                    .map_err(|e| e.to_string())?;
                return Ok(None);
            }
        };
        let resolution = match agy_completion::read_turn_completion(&transcript, &turn) {
            Ok(res) => res,
            Err(error) => {
                tracing::warn!(
                    event = "bus.callback.rejected",
                    reason = "agy_transcript_unreadable",
                    agent_id = id.0,
                    session_id = %session,
                    turn_id = %turn,
                    %error,
                    "Cannot read AGY transcript for owned turn"
                );
                state
                    .set_agent_error(id, Some(error))
                    .map_err(|e| e.to_string())?;
                return Ok(None);
            }
        };
        match resolution {
            TurnResolution::Final {
                turn_id,
                text,
                prompts,
            } => {
                state.bind_recorded_turn_inputs(id, &turn_id, &prompts);
                let mut cb = callback(
                    record,
                    id,
                    session.to_owned(),
                    turn_id.clone(),
                    None,
                    CallbackEventKind::Final { text },
                );
                cb.callback_id = format!("{}:agy-transcript:{turn_id}", record.id);
                Ok(Some(cb))
            }
            TurnResolution::Pending => Ok(None),
        }
    }

    pub(super) fn agy_completion_callback(
        &mut self,
        id: AgentId,
        mut state: BusState,
        path: &Path,
        record: &callbacks::Record,
        session: String,
    ) -> Result<CallbackRecordResult, String> {
        let verdict = agy_hooks::stop_verdict(&record.value)?;
        let Some(turn) = Self::owned_agy_turn(&state, id, record, &session) else {
            tracing::debug!(
                event = "bus.callback.ignored",
                reason = "agy_no_owned_turn",
                agent_id = id.0,
                session_id = %session,
                "AGY Stop has no owned Bus turn"
            );
            if !matches!(verdict, StopVerdict::BackgroundPending) {
                self.own_turns.remove(&id);
            }
            return Ok(ControlFlow::Continue((
                state,
                None,
                agy_observation_paths(path),
            )));
        };
        match verdict {
            StopVerdict::BackgroundPending => {
                let cb = callback(
                    record,
                    id,
                    session,
                    turn,
                    None,
                    CallbackEventKind::BackgroundPending,
                );
                Ok(ControlFlow::Continue((
                    state,
                    Some(cb),
                    agy_observation_paths(path),
                )))
            }
            StopVerdict::Cancelled => {
                let cb = callback(
                    record,
                    id,
                    session,
                    turn,
                    None,
                    CallbackEventKind::Error {
                        message: "AGY turn cancelled; no reply was published.".into(),
                        provider_error: Some("cancelled by user".into()),
                    },
                );
                Ok(ControlFlow::Continue((
                    state,
                    Some(cb),
                    agy_observation_paths(path),
                )))
            }
            StopVerdict::Failed { message, error } => {
                let cb = callback(
                    record,
                    id,
                    session,
                    turn,
                    None,
                    CallbackEventKind::Error {
                        message,
                        provider_error: Some(error),
                    },
                );
                Ok(ControlFlow::Continue((
                    state,
                    Some(cb),
                    agy_observation_paths(path),
                )))
            }
            StopVerdict::Completed => {
                if let Some(cb) =
                    self.committed_agy_completion_callback(id, &mut state, record, &session)?
                {
                    return Ok(ControlFlow::Continue((
                        state,
                        Some(cb),
                        agy_observation_paths(path),
                    )));
                }
                if state
                    .agent(id)
                    .and_then(|a| a.actionable_error.as_ref())
                    .is_some()
                {
                    return Ok(ControlFlow::Continue((
                        state,
                        None,
                        agy_observation_paths(path),
                    )));
                }
                let claim = claim_with_policy(
                    path,
                    crate::messaging::storage::io::now_ms(),
                    AGY_RETRY_EXT,
                    "AGY",
                    AGY_RETRY_ATTEMPTS,
                    AGY_RETRY_DELAY_MS,
                )?;
                match claim {
                    Claim::Attempt(attempt) => {
                        tracing::debug!(
                            event = "bus.callback.deferred",
                            reason = "agy_transcript_pending",
                            agent_id = id.0,
                            launch_id = %record.manifest.launch_id,
                            session_id = %session,
                            turn_id = %turn,
                            attempt,
                            "AGY Stop retained until transcript_full.jsonl flushes final reply"
                        );
                        Ok(ControlFlow::Break(()))
                    }
                    Claim::NotDue => {
                        tracing::debug!(
                            event = "bus.callback.deferred",
                            reason = "agy_retry_not_due",
                            agent_id = id.0,
                            session_id = %session,
                            turn_id = %turn,
                            "AGY Stop retained until retry deadline"
                        );
                        Ok(ControlFlow::Break(()))
                    }
                    Claim::Exhausted => {
                        tracing::warn!(
                            event = "bus.callback.expired",
                            reason = "agy_retry_budget_exhausted",
                            agent_id = id.0,
                            launch_id = %record.manifest.launch_id,
                            session_id = %session,
                            turn_id = %turn,
                            attempts = AGY_RETRY_ATTEMPTS,
                            "AGY Stop retired without committed transcript final"
                        );
                        Ok(ControlFlow::Continue((
                            state,
                            None,
                            agy_observation_paths(path),
                        )))
                    }
                }
            }
        }
    }
}
