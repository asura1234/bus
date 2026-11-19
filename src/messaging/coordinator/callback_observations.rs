//! Durable observation ownership and bounded retry metadata, independent of provider parsing.
use super::{callbacks, AgentId, Path, Worker};
use serde::{Deserialize, Serialize};
#[derive(Default, Deserialize, Serialize)]
struct Retry {
    attempts: u8,
    next_at: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::messaging::coordinator) enum Claim {
    Attempt(u8),
    NotDue,
    Exhausted,
}

pub(in crate::messaging::coordinator) fn observation_paths_with_ext(
    path: &Path,
    ext: &str,
) -> Vec<std::path::PathBuf> {
    let mut paths = vec![path.to_path_buf()];
    let marker = path.with_extension(ext);
    if marker.exists() {
        paths.push(marker);
    }
    paths
}

pub(in crate::messaging::coordinator) fn claim_with_policy(
    path: &Path,
    now: u64,
    ext: &str,
    label: &str,
    max_attempts: u8,
    delay_ms: u64,
) -> Result<Claim, String> {
    let marker = path.with_extension(ext);
    let mut retry: Retry = match std::fs::read(&marker) {
        Ok(bytes) if bytes.len() <= 1024 => {
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?
        }
        Ok(_) => return Err(format!("{label} retry metadata is oversized")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Retry::default(),
        Err(error) => return Err(error.to_string()),
    };
    if retry.attempts >= max_attempts {
        return Ok(Claim::Exhausted);
    }
    if retry.next_at > now {
        return Ok(Claim::NotDue);
    }
    retry.attempts += 1;
    retry.next_at = now.saturating_add(delay_ms);
    crate::agents::providers::hook_json::atomic_write(
        &marker,
        &serde_json::to_vec(&retry).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(Claim::Attempt(retry.attempts))
}

impl Worker {
    pub(super) fn retire_superseded_observation(
        &mut self,
        id: AgentId,
        dir: &Path,
        path: &Path,
        record: &callbacks::Record,
        parsed: &callbacks::Parsed,
    ) -> Result<bool, String> {
        let session = match parsed {
            callbacks::Parsed::Session { session, source } if source.as_deref() == Some("new") => {
                session
            }
            callbacks::Parsed::Session { .. } => return Ok(false),
            callbacks::Parsed::Started { session, .. }
            | callbacks::Parsed::Final { session, .. }
            | callbacks::Parsed::CompletionCandidate { session, .. }
            | callbacks::Parsed::ReconcileSession { session }
            | callbacks::Parsed::BackgroundPending { session, .. }
            | callbacks::Parsed::Failure { session, .. }
            | callbacks::Parsed::Completed { session, .. }
            | callbacks::Parsed::Response { session, .. } => session,
            callbacks::Parsed::Ignore => return Ok(false),
        };
        if !matches!(
            record.manifest.provider,
            crate::agents::providers::ProviderKind::Grok
                | crate::agents::providers::ProviderKind::Kimi
                | crate::agents::providers::ProviderKind::Agy
        ) || self
            .state
            .agent(id)
            .and_then(|a| a.superseded_session_id.as_deref())
            != Some(session.as_str())
        {
            return Ok(false);
        }
        tracing::debug!(
            event = "bus.callback.ignored",
            reason = "superseded_session",
            "Retiring observation from the accepted reset's previous session"
        );
        let paths = if record.manifest.provider == crate::agents::providers::ProviderKind::Agy {
            observation_paths_with_ext(path, "agy-retry")
        } else {
            observation_paths_with_ext(path, "grok-retry")
        };
        self.acknowledge_spool(dir, paths)?;
        Ok(true)
    }
}
