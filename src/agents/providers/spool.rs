//! Durable, provider-neutral callback envelopes.
//!
//! Routing is opaque here. Messaging supplies the key and validates its owner,
//! launch, session and sequence before applying any parsed observation.
use super::ProviderKind;
use crate::utils::time::{digest, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io,
    path::{Path, PathBuf},
};

#[path = "spool/files.rs"]
mod files;
pub(crate) use super::parse;
pub(crate) use files::append_lock;

#[path = "spool/parsed_event.rs"]
mod parsed_event;
pub(crate) use parsed_event::{field, Parsed};

/// Maximum raw hook input; retain the existing two-MiB capture limit.
pub(crate) const MAX_CALLBACK_BYTES: u64 = 2 * 1024 * 1024;

/// A caller-owned routing value, never interpreted by a provider adapter.
/// Its numeric JSON representation matches the existing spool's `AgentId`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct RoutingKey(pub(crate) u64);

/// Launch-local capture metadata. These fields are facts, not ownership proof.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Manifest {
    /// Preserve the on-disk spelling without importing messaging's identity.
    #[serde(rename = "agent_id")]
    pub(crate) routing_key: RoutingKey,
    pub(crate) provider: ProviderKind,
    pub(crate) launch_id: String,
}

/// A raw hook and the launch metadata captured with it.
///
/// The JSON field names, payload and numeric types match the existing spool.
/// Parsing does not mutate `value`; the coordinator owns acknowledgement after
/// persisting its state and any response/transcript companion selection.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Record {
    pub(crate) id: String,
    pub(crate) sequence: u64,
    pub(crate) at_ms: u64,
    pub(crate) manifest: Manifest,
    pub(crate) value: Value,
    /// The hook process and its ancestors, nearest first, for a session start.
    /// Records spooled by an older Bus have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) reporter: Vec<crate::platform::ProcessInstance>,
}

pub(crate) fn initialize(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    files::private_dir(dir)?;
    files::atomic_write(&dir.join("manifest.json"), &serde_json::to_vec(manifest)?)
}

pub(crate) fn boundary(dir: &Path) -> io::Result<u64> {
    let _lease = append_lock(&dir.join("append.lock"))?;
    read_counter(dir)
}

fn read_counter(dir: &Path) -> io::Result<u64> {
    match std::fs::read_to_string(dir.join("sequence")) {
        Ok(value) => value.parse().map_err(io::Error::other),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

pub(crate) fn append(
    dir: &Path,
    launch: &str,
    provider: ProviderKind,
    value: Value,
    reporter: &[crate::platform::ProcessInstance],
) -> io::Result<()> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
    if manifest.launch_id != launch || manifest.provider != provider {
        return Err(io::Error::other("Bus callback launch identity mismatch"));
    }
    let _lease = append_lock(&dir.join("append.lock"))?;
    // Identical hooks share one spool file, so a hook retry counts once. A session
    // start also keys on its process chain: a restart resuming the same conversation
    // sends the exited process's bytes, and must not be folded into its record.
    let id =
        if !reporter.is_empty() && matches!(parse(provider, &value), Ok(Parsed::Session { .. })) {
            digest(&serde_json::to_vec(&(launch, &value, reporter))?)
        } else {
            digest(&serde_json::to_vec(&(launch, &value))?)
        };
    let path = dir.join(format!("event-{id}.json"));
    if path.exists() {
        tracing::debug!(event = "bus.callback.duplicate", callback_id = %id, "Callback already spooled");
        return Ok(());
    }
    let sequence = read_counter(dir)?
        .checked_add(1)
        .ok_or_else(|| io::Error::other("Bus callback sequence overflow"))?;
    // Reserve durably before publishing. Gaps after a crash are harmless.
    files::atomic_write(&dir.join("sequence"), sequence.to_string().as_bytes())?;
    let record = Record {
        id,
        sequence,
        at_ms: now_ms(),
        manifest,
        value,
        reporter: reporter.to_vec(),
    };
    files::atomic_write(&path, &serde_json::to_vec(&record)?)?;
    tracing::info!(event = "bus.callback.spooled", callback_id = %record.id,
        agent_id = record.manifest.routing_key.0, provider = ?provider, launch_id = launch,
        sequence, kind = parse(provider, &record.value).map_or("invalid", |p| p.kind()),
        "Provider callback persisted");
    Ok(())
}

pub(crate) fn records(dir: &Path) -> io::Result<Vec<(PathBuf, Record)>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("event-")
            || entry.path().extension().is_none_or(|e| e != "json")
        {
            continue;
        }
        if !entry.file_type()?.is_file() || entry.metadata()?.len() > MAX_CALLBACK_BYTES + 4096 {
            return Err(io::Error::other("Invalid Bus callback file"));
        }
        result.push((
            entry.path(),
            serde_json::from_slice::<Record>(&std::fs::read(entry.path())?)?,
        ));
    }
    result.sort_by_key(|(_, record)| record.sequence);
    Ok(result)
}
