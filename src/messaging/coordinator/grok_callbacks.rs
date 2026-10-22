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

fn claim(path: &Path, now: u64) -> Result<bool, String> {
    let marker = path.with_extension("grok-retry");
    let mut retry: Retry = match std::fs::read(&marker) {
        Ok(bytes) if bytes.len() <= 1024 => {
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err("Grok retry metadata is oversized".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Retry::default(),
        Err(error) => return Err(error.to_string()),
    };
    if retry.attempts >= 8 || retry.next_at > now {
        return Ok(false);
    }
    retry.attempts += 1;
    retry.next_at = now.saturating_add(2000);
    crate::agents::providers::hook_json::atomic_write(
        &marker,
        &serde_json::to_vec(&retry).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

impl Worker {
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
            return Ok(ControlFlow::Break(()));
        };
        let kind = match self.resolve_grok_observation(id, &state, path, record, &session, &turn)? {
            CompletionResolution::Pending => return Ok(ControlFlow::Break(())),
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
            _ if !candidate => return Ok(ControlFlow::Break(())),
            _ => return Err("Grok completion identity differs from its observation".into()),
        };
        let mut remove = vec![path.to_path_buf()];
        if path.with_extension("grok-retry").exists() {
            remove.push(path.with_extension("grok-retry"));
        }
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
        path: &Path,
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
        if !idle || !claim(path, crate::messaging::storage::io::now_ms())? {
            return Ok(CompletionResolution::Pending);
        }
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
    fn grok_g_reconciliation_retry_budget_survives_reopen_and_never_spins() {
        let root = crate::utils::test_temp::unique_temp_path("grok-reconcile-retry");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("event-owned.json");
        std::fs::write(&path, "retained observation").unwrap();
        for i in 0..8 {
            assert!(claim(&path, i * 2000).unwrap());
            assert!(!claim(&path, i * 2000 + 1).unwrap());
        }
        assert!(!claim(&path, u64::MAX).unwrap());
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "retained observation"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
