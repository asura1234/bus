//! Bounded durable retries for Grok observations. Native completion remains
//! pending until an authenticated post-gate committed-final adapter is verified.
use super::{
    callback, callbacks, AgentId, BusState, CallbackEventKind, CallbackRecordResult, Path,
    RuntimeStatus, Worker,
};
use crate::agents::providers::grok::completion::{self, CompletionResolution};
use serde::{Deserialize, Serialize};
use std::ops::ControlFlow;

#[derive(Default, Deserialize, Serialize)]
struct Retry {
    attempts: u8,
    next_at: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Claim {
    Attempt(u8),
    NotDue,
    Exhausted,
}

pub(super) fn observation_paths(path: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let marker = path.with_extension("grok-retry");
    if marker.exists() {
        paths.push(marker);
    }
    paths
}

fn claim(path: &Path, now: u64) -> Result<Claim, String> {
    let marker = path.with_extension("grok-retry");
    let mut retry: Retry = match std::fs::read(&marker) {
        Ok(bytes) if bytes.len() <= 1024 => {
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("Grok retry metadata is oversized".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Retry::default(),
        Err(error) => return Err(error.to_string()),
    };
    if retry.attempts >= 8 {
        return Ok(Claim::Exhausted);
    }
    if retry.next_at > now {
        return Ok(Claim::NotDue);
    }
    retry.attempts += 1;
    retry.next_at = now.saturating_add(2000);
    crate::agents::providers::hook_json::atomic_write(
        &marker,
        &serde_json::to_vec(&retry).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(Claim::Attempt(retry.attempts))
}

fn claim_idle_observation(state: &BusState, id: AgentId, path: &Path) -> Result<Claim, String> {
    if !state
        .agent(id)
        .is_some_and(|a| a.status == RuntimeStatus::Idle)
    {
        tracing::debug!(
            event = "bus.callback.deferred",
            reason = "grok_not_idle",
            "Grok completion awaits settled idle"
        );
        return Ok(Claim::NotDue);
    }
    let claim = claim(path, crate::messaging::storage::io::now_ms())?;
    match claim {
        Claim::NotDue => tracing::debug!(
            event = "bus.callback.deferred",
            reason = "grok_retry_not_due",
            "Grok observation retained until retry deadline"
        ),
        Claim::Exhausted => tracing::warn!(
            event = "bus.callback.expired",
            reason = "grok_retry_budget_exhausted",
            attempts = 8,
            "Grok observation retired without committed final proof"
        ),
        Claim::Attempt(attempt) => tracing::debug!(
            event = "bus.callback.reconciling",
            reason = "grok_candidate",
            attempt,
            "Checking Grok completion evidence"
        ),
    }
    Ok(claim)
}

impl Worker {
    pub(super) fn retire_superseded_grok_observation(
        &mut self,
        id: AgentId,
        dir: &Path,
        path: &Path,
        record: &callbacks::Record,
        parsed: &callbacks::Parsed,
    ) -> Result<bool, String> {
        let session = match parsed {
            callbacks::Parsed::CompletionCandidate { session, .. }
            | callbacks::Parsed::ReconcileSession { session } => session,
            _ => return Ok(false),
        };
        if record.manifest.provider != crate::agents::providers::ProviderKind::Grok
            || self
                .state
                .agent(id)
                .and_then(|a| a.superseded_session_id.as_deref())
                != Some(session.as_str())
        {
            return Ok(false);
        }
        tracing::debug!(
            event = "bus.callback.ignored",
            reason = "grok_superseded_session",
            "Retiring observation from the accepted reset's previous session"
        );
        self.acknowledge_spool(dir, observation_paths(path))?;
        Ok(true)
    }

    pub(super) fn grok_completion_callback(
        &self,
        id: AgentId,
        state: BusState,
        path: &Path,
        record: &callbacks::Record,
        session: String,
        candidate_turn: Option<String>,
    ) -> Result<CallbackRecordResult, String> {
        let candidate = candidate_turn.is_some();
        let _span = tracing::debug_span!(
            "grok_observation",
            agent_id = id.0,
            sequence = record.sequence,
            candidate
        )
        .entered();
        let turn = candidate_turn.or_else(|| {
            state
                .agent(id)
                .and_then(|a| a.current_request)
                .and_then(|r| state.request(r))
                .filter(|r| {
                    r.trusted_start_bound
                        && r.expected_launch_id.as_deref()
                            == Some(record.manifest.launch_id.as_str())
                        && r.provider_session_id.as_deref() == Some(session.as_str())
                })
                .and_then(|r| r.provider_turn_id.clone())
        });
        let Some(turn) = turn else {
            tracing::debug!(
                event = "bus.callback.ignored",
                reason = "grok_no_owned_turn",
                "Idle observation has no matching Bus request"
            );
            return Ok(ControlFlow::Continue((
                state,
                None,
                observation_paths(path),
            )));
        };
        match claim_idle_observation(&state, id, path)? {
            Claim::NotDue => return Ok(ControlFlow::Break(())),
            Claim::Exhausted => {
                return Ok(ControlFlow::Continue((
                    state,
                    None,
                    observation_paths(path),
                )))
            }
            Claim::Attempt(_) => {}
        }
        let kind = match self.resolve_grok_observation(id, &state, record, &session, &turn)? {
            CompletionResolution::Pending => {
                tracing::debug!(
                    event = "bus.callback.deferred",
                    reason = "grok_attestation_pending",
                    "No committed session/turn final proof; Stop and idle alone cannot settle"
                );
                return Ok(ControlFlow::Break(()));
            }
            CompletionResolution::Final {
                session: s,
                turn: t,
                text,
            } if s == session && t == turn => CallbackEventKind::Final { text },
            CompletionResolution::Cancelled {
                session: s,
                turn: t,
                reason,
            } if candidate && s == session && t == turn => CallbackEventKind::Error {
                message: format!("Grok turn cancelled: {reason}"),
                provider_error: Some(reason),
            },
            _ if !candidate => {
                tracing::debug!(
                    event = "bus.callback.deferred",
                    reason = "grok_idle_outcome_not_final",
                    "Session idle observation cannot certify this outcome"
                );
                return Ok(ControlFlow::Break(()));
            }
            _ => return Err("Grok completion identity differs from its observation".into()),
        };
        tracing::debug!(
            event = "bus.callback.resolved",
            reason = "grok_attested_outcome",
            "Grok outcome has matching session/turn evidence"
        );
        let remove = observation_paths(path);
        Ok(ControlFlow::Continue((
            state,
            Some(callback(record, id, session, turn, None, kind)),
            remove,
        )))
    }

    pub(super) fn resolve_grok_observation(
        &self,
        id: AgentId,
        state: &BusState,
        record: &callbacks::Record,
        session: &str,
        turn: &str,
    ) -> Result<CompletionResolution, String> {
        if session.is_empty() || turn.is_empty() {
            return Err("Grok reconciliation requires an owned session and turn".into());
        }
        let idle = state
            .agent(id)
            .is_some_and(|a| a.status == RuntimeStatus::Idle);
        #[cfg(feature = "dev-tools")]
        {
            // Only the private driver's owned process context authorizes the
            // synthetic producer. No native JSON field enables it.
            let root = std::env::var_os("BUS_DATA_DIR").map(std::path::PathBuf::from);
            let scratch = root
                .as_deref()
                .filter(|root| *root == self.data_dir && self.dev_enabled)
                .filter(|_| {
                    std::env::var("BUS_TUI_FAKE_AGENT").as_deref() == Ok("1")
                        && std::env::var_os("BUS_TUI_OWNER").is_some()
                })
                .and_then(Path::parent);
            let attestation = match record
                .value
                .get("transcriptPath")
                .and_then(serde_json::Value::as_str)
            {
                Some(transcript) => crate::agents::providers::grok::synthetic::attest(
                    scratch,
                    Path::new(transcript),
                    session,
                    turn,
                )?,
                None => None,
            };
            if attestation.is_some() {
                return completion::resolve_attested_completion(
                    session,
                    turn,
                    idle,
                    attestation.as_ref(),
                );
            }
        }
        // Native Stop stays a candidate. An idle Notification has no turn
        // outcome and must never be passed off as a native final marker.
        tracing::debug!(
            event = "bus.callback.deferred",
            reason = "grok_native_adapter_pending",
            "Native committed-final adapter awaits authenticated evidence"
        );
        if record
            .value
            .get("hookEventName")
            .and_then(serde_json::Value::as_str)
            == Some("stop")
        {
            completion::resolve_candidate(&record.value, idle)
        } else {
            Ok(CompletionResolution::Pending)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grok_reconciliation_retry_budget_survives_reopen_and_never_spins() {
        let root = crate::utils::test_temp::unique_temp_path("grok-reconcile-retry");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("event-owned.json");
        std::fs::write(&path, "retained observation").unwrap();
        for i in 0..8 {
            assert_eq!(claim(&path, i * 2000).unwrap(), Claim::Attempt(i as u8 + 1));
            assert_eq!(
                claim(&path, i * 2000 + 1).unwrap(),
                if i == 7 {
                    Claim::Exhausted
                } else {
                    Claim::NotDue
                }
            );
        }
        assert_eq!(claim(&path, u64::MAX).unwrap(), Claim::Exhausted);
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "retained observation"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
