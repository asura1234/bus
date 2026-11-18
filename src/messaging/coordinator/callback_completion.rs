use super::{
    callbacks, AgentId, BusState, CallbackEventKind, CallbackRecordResult, ControlFlow, Path,
    ProviderCallback, ProviderKind, Worker,
};
use crate::agents::providers::agy::{
    completion::{self as agy_completion, TurnResolution},
    hooks::{self as agy_hooks, StopVerdict},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Default, Deserialize, Serialize)]
struct AgyRetry {
    attempts: u8,
    next_at: u64,
}

enum AgyClaim {
    Attempt,
    NotDue,
    Exhausted,
}

fn agy_observation_paths(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let marker = path.with_extension("agy-retry");
    if marker.exists() {
        paths.push(marker);
    }
    paths
}

fn claim_agy_retry(path: &Path, now_ms: u64) -> Result<AgyClaim, String> {
    let marker = path.with_extension("agy-retry");
    let mut retry: AgyRetry = match std::fs::read(&marker) {
        Ok(bytes) if bytes.len() <= 1024 => {
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("AGY retry metadata is oversized".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => AgyRetry::default(),
        Err(error) => return Err(error.to_string()),
    };
    if retry.attempts >= 12 {
        return Ok(AgyClaim::Exhausted);
    }
    if retry.next_at > now_ms {
        return Ok(AgyClaim::NotDue);
    }
    retry.attempts += 1;
    retry.next_at = now_ms.saturating_add(250);
    crate::agents::providers::hook_json::atomic_write(
        &marker,
        &serde_json::to_vec(&retry).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(AgyClaim::Attempt)
}

pub(super) fn started_callback(
    record: &callbacks::Record,
    id: AgentId,
    session: String,
    turn: String,
    prompt: String,
) -> ProviderCallback {
    super::callback(
        record,
        id,
        session,
        turn,
        Some(prompt),
        CallbackEventKind::PromptStarted,
    )
}

impl Worker {
    pub(super) fn started_request_callback(
        &mut self,
        id: AgentId,
        state: &mut BusState,
        record: &callbacks::Record,
        session: String,
        turn: String,
        prompt: String,
    ) -> Result<ProviderCallback, String> {
        let started = started_callback(record, id, session.clone(), turn.clone(), prompt);
        if record.manifest.provider == ProviderKind::Kimi
            && state
                .agent(id)
                .and_then(|agent| agent.current_request)
                .and_then(|request| state.request(request))
                .is_some_and(|request| request.provider_turn_id.as_deref() != Some(&turn))
        {
            // The wire is authoritative even when Stop was lost or reordered.
            if let Some(callback) =
                self.committed_kimi_completion_callback(id, state, record, session, Some(&started))?
            {
                self.apply_request_callback(id, state, callback)?;
            }
        } else if record.manifest.provider == ProviderKind::Agy
            && state
                .agent(id)
                .and_then(|agent| agent.current_request)
                .and_then(|request| state.request(request))
                .is_some_and(|request| request.provider_turn_id.as_deref() != Some(&turn))
        {
            if let Some(callback) =
                self.committed_agy_completion_callback(id, state, record, &session)?
            {
                self.apply_request_callback(id, state, callback)?;
            }
        }
        Ok(started)
    }

    fn committed_kimi_completion_callback(
        &self,
        id: AgentId,
        state: &mut BusState,
        record: &callbacks::Record,
        session: String,
        started: Option<&ProviderCallback>,
    ) -> Result<Option<ProviderCallback>, String> {
        Ok(match self.recorded_kimi_end(state, id)? {
            Some(crate::messaging::model::RecordedTurnEnd::Final {
                turn_id,
                text,
                prompts,
            }) => {
                state.bind_recorded_turn_inputs(id, &turn_id, &prompts);
                let mut callback = super::callback(
                    record,
                    id,
                    session,
                    turn_id.clone(),
                    None,
                    CallbackEventKind::Final { text },
                );
                callback.callback_id = format!("{}:kimi-wire:{turn_id}", record.id);
                Some(callback)
            }
            Some(crate::messaging::model::RecordedTurnEnd::Error(error)) => {
                if let Some(started) = started {
                    state.settle_kimi_failure_before_start(started, error);
                }
                None
            }
            _ => None,
        })
    }

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

    fn committed_agy_completion_callback(
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
            return Ok(None);
        };
        let transcript = agy_completion::resolve_transcript_path(raw_path, session)?;
        match agy_completion::read_turn_completion(&transcript, &turn)? {
            TurnResolution::Final {
                turn_id,
                text,
                prompts,
            } => {
                state.bind_recorded_turn_inputs(id, &turn_id, &prompts);
                let mut callback = super::callback(
                    record,
                    id,
                    session.to_owned(),
                    turn_id.clone(),
                    None,
                    CallbackEventKind::Final { text },
                );
                callback.callback_id = format!("{}:agy-transcript:{turn_id}", record.id);
                Ok(Some(callback))
            }
            TurnResolution::Pending => Ok(None),
        }
    }

    fn agy_completion_callback(
        &mut self,
        id: AgentId,
        mut state: BusState,
        path: &Path,
        record: &callbacks::Record,
        session: String,
    ) -> Result<CallbackRecordResult, String> {
        let verdict = agy_hooks::stop_verdict(&record.value)?;
        let Some(turn) = Self::owned_agy_turn(&state, id, record, &session) else {
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
                let callback = super::callback(
                    record,
                    id,
                    session,
                    turn,
                    None,
                    CallbackEventKind::BackgroundPending,
                );
                Ok(ControlFlow::Continue((
                    state,
                    Some(callback),
                    agy_observation_paths(path),
                )))
            }
            StopVerdict::Cancelled => {
                let callback = super::callback(
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
                    Some(callback),
                    agy_observation_paths(path),
                )))
            }
            StopVerdict::Failed { message, error } => {
                let callback = super::callback(
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
                    Some(callback),
                    agy_observation_paths(path),
                )))
            }
            StopVerdict::Completed => {
                if let Some(callback) =
                    self.committed_agy_completion_callback(id, &mut state, record, &session)?
                {
                    return Ok(ControlFlow::Continue((
                        state,
                        Some(callback),
                        agy_observation_paths(path),
                    )));
                }
                match claim_agy_retry(path, crate::messaging::storage::io::now_ms())? {
                    AgyClaim::Attempt | AgyClaim::NotDue => Ok(ControlFlow::Break(())),
                    AgyClaim::Exhausted => Ok(ControlFlow::Continue((
                        state,
                        None,
                        agy_observation_paths(path),
                    ))),
                }
            }
        }
    }

    pub(super) fn completion_callback(
        &mut self,
        id: AgentId,
        mut state: BusState,
        path: &Path,
        record: &callbacks::Record,
        session: String,
        candidate_turn: Option<String>,
    ) -> Result<CallbackRecordResult, String> {
        if record.manifest.provider == ProviderKind::Kimi {
            // Stop carries no turn id. Read the committed owned turn before
            // processing a queued prompt's Started callback, which otherwise
            // rebinds the old lead and makes its final impossible to match.
            let callback =
                self.committed_kimi_completion_callback(id, &mut state, record, session, None)?;
            return Ok(ControlFlow::Continue((
                state,
                callback,
                vec![path.to_path_buf()],
            )));
        }
        if record.manifest.provider == ProviderKind::Agy {
            return self.agy_completion_callback(id, state, path, record, session);
        }
        self.grok_completion_callback(id, state, path, record, session, candidate_turn)
    }
}
