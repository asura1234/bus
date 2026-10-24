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
use files::append_lock;
pub(crate) use files::try_append_lock;

#[path = "spool/parsed_event.rs"]
mod parsed_event;
pub(crate) use parsed_event::{field, optional_text, Parsed};

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
    /// The hook ran under a provider that the launch's agent started itself (say a
    /// `claude -p` from its shell tool), which inherited the launch's environment.
    /// Such a callback is not the agent's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) nested_provider: bool,
}

pub(crate) fn initialize(dir: &Path, manifest: &Manifest) -> io::Result<()> {
    files::private_dir(dir)?;
    files::atomic_write(&dir.join("manifest.json"), &serde_json::to_vec(manifest)?)
}

pub(crate) fn boundary(dir: &Path) -> io::Result<u64> {
    let _lease = append_lock(&dir.join("append.lock"))?;
    // A staged hook predates this submission even if its publisher was busy.
    publish_pending(dir)?;
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
    nested_provider: bool,
) -> io::Result<()> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?;
    if manifest.launch_id != launch || manifest.provider != provider {
        return Err(io::Error::other("Bus callback launch identity mismatch"));
    }
    // Identical hooks share one spool file, so a hook retry counts once. A session
    // start also keys on its process chain: a restart resuming the same conversation
    // sends the exited process's bytes, and must not be folded into its record.
    let id =
        if !reporter.is_empty() && matches!(parse(provider, &value), Ok(Parsed::Session { .. })) {
            digest(&serde_json::to_vec(&(launch, &value, reporter))?)
        } else {
            digest(&serde_json::to_vec(&(launch, &value))?)
        };
    let record = Record {
        id,
        sequence: 0,
        at_ms: now_ms(),
        manifest,
        value,
        reporter: reporter.to_vec(),
        nested_provider,
    };
    // Each attempt has its own name: another publisher must never unlink a
    // duplicate retry's newer write. No shared lock precedes this durable copy.
    static NEXT_PENDING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let pending = loop {
        let candidate = dir.join(format!(
            "pending-{}-{}.json",
            std::process::id(),
            NEXT_PENDING.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        // A previous process with a recycled pid may have left this name behind.
        if !candidate.try_exists()? {
            break candidate;
        }
    };
    files::atomic_write(&pending, &serde_json::to_vec(&record)?)?;
    try_publish_pending(dir, Some(&pending))
}

pub(crate) fn records(dir: &Path) -> io::Result<Vec<(PathBuf, Record)>> {
    // Retry envelopes left by exited hook processes without blocking the poller.
    try_publish_pending(dir, None)?;
    read_records(dir, "event-")
}

/// Keep the original envelope for inspection outside the active event scan.
/// The coordinator calls this only after its rejection state is durable.
pub(crate) fn retain_rejected(dir: &Path, path: &Path, routing_key: RoutingKey) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("event-"))
        .ok_or_else(|| io::Error::other("Invalid Bus callback filename"))?;
    std::fs::rename(path, dir.join(format!("rejected-{name}")))?;
    crate::platform::sync_parent_directory(dir)?;
    tracing::warn!(
        event = "bus.callback.rejected",
        reason = "invalid_payload",
        agent_id = routing_key.0,
        callback_id = name.trim_end_matches(".json"),
        "Callback retained outside the active spool for inspection"
    );
    Ok(())
}

fn try_publish_pending(dir: &Path, own: Option<&Path>) -> io::Result<()> {
    let _lease = match try_append_lock(&dir.join("append.lock")) {
        Ok(lease) => lease,
        // The event is already durable. A later hook, poll or boundary retries it.
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(error) => return Err(error),
    };
    if let Some(path) = own {
        // Never let a later hook allocate a sequence ahead of a completed stage.
        // Leave the backlog to the consumer instead of draining it in a hook.
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.path() != path
                && entry.file_name().to_string_lossy().starts_with("pending-")
                && entry.path().extension().is_some_and(|e| e == "json")
            {
                return Ok(());
            }
        }
        // Hooks do not drain a backlog within their provider's five-second budget.
        // A consumer may already have published this envelope while we staged it.
        match std::fs::read(path) {
            Ok(bytes) => publish_record(dir, path, serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    } else {
        publish_pending(dir)
    }
}

// Caller holds append.lock; sequence allocation and duplicate publication remain
// serialized, with the same event wire format and acknowledgement path as before.
fn publish_pending(dir: &Path) -> io::Result<()> {
    // Millisecond capture timestamps can tie across completed hooks. Use the
    // files' full-resolution write order, not arbitrary pid/counter filenames.
    let mut pending = read_records(dir, "pending-")?
        .into_iter()
        .map(|(path, record)| Ok((std::fs::metadata(&path)?.modified()?, path, record)))
        .collect::<io::Result<Vec<_>>>()?;
    pending.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (_, staged, record) in pending {
        publish_record(dir, &staged, record)?;
    }
    Ok(())
}

fn publish_record(dir: &Path, staged: &Path, mut record: Record) -> io::Result<()> {
    // A malformed staged record must not turn its id into an arbitrary path.
    if record.id.is_empty() || !record.id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("Invalid Bus callback id"));
    }
    let path = dir.join(format!("event-{}.json", record.id));
    if path.exists() || dir.join(format!("rejected-{}.json", record.id)).exists() {
        tracing::debug!(event = "bus.callback.duplicate", callback_id = %record.id, "Callback already spooled");
    } else {
        record.sequence = read_counter(dir)?
            .checked_add(1)
            .ok_or_else(|| io::Error::other("Bus callback sequence overflow"))?;
        // Reserve durably before publishing. Gaps after a crash are harmless.
        files::atomic_write(
            &dir.join("sequence"),
            record.sequence.to_string().as_bytes(),
        )?;
        files::atomic_write(&path, &serde_json::to_vec(&record)?)?;
        tracing::info!(event = "bus.callback.spooled", callback_id = %record.id,
                agent_id = record.manifest.routing_key.0, provider = ?record.manifest.provider,
                launch_id = %record.manifest.launch_id, sequence = record.sequence,
                kind = parse(record.manifest.provider, &record.value).map_or("invalid", |p| p.kind()),
                "Provider callback persisted");
    }
    // A failed publication keeps its durable input for the next retry.
    std::fs::remove_file(staged)
}

fn read_records(dir: &Path, prefix: &str) -> io::Result<Vec<(PathBuf, Record)>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with(prefix)
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
