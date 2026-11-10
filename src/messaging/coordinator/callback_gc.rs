//! Callback captures belong to a roster owner, not to its historical identity.
use super::BusState;
use crate::agents::providers::{spool::Manifest, ProviderKind};
use std::{
    fs,
    io::{self, Read},
    path::Path,
};

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// Only call with committed state, under the coordinator's data-directory lease.
/// Collection is best effort: failure must not undo an already durable deletion.
pub(super) fn collect(data_dir: &Path, state: &BusState) {
    let root = data_dir.join("callbacks");
    let entries = match real_directory(&root).and_then(|()| fs::read_dir(&root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(event = "bus.callbacks.collection_failed", path = %root.display(),
                %error, "Callback collection kept an inaccessible directory");
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(event = "bus.callbacks.collection_failed", %error,
                    "Callback directory entry could not be read");
                continue;
            }
        };
        let path = entry.path();
        let result = orphan(&entry, state).and_then(|manifest| {
            let Some(manifest) = manifest else {
                return Ok(());
            };
            if manifest.provider == ProviderKind::Kimi {
                preserve_kimi_home(data_dir, &path)?;
            }
            fs::remove_dir_all(&path)?;
            crate::platform::sync_parent_directory(&root)?;
            tracing::info!(event = "bus.callbacks.collected", agent_id = manifest.routing_key.0,
                launch_id = %manifest.launch_id, "Deleted owner's callback capture removed");
            Ok(())
        });
        if let Err(error) = result {
            tracing::warn!(event = "bus.callbacks.collection_failed", path = %path.display(),
                %error, "Callback collection retained uncertain or unremovable data");
        }
    }
}

fn real_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "Callback collection requires a real directory",
        ));
    }
    Ok(())
}

fn orphan(entry: &fs::DirEntry, state: &BusState) -> io::Result<Option<Manifest>> {
    if !entry.file_type()?.is_dir() {
        return Ok(None);
    }
    let name = entry.file_name();
    // A current launch wins even over a damaged or contradictory manifest.
    if state
        .agents()
        .any(|agent| agent.runtime_identity.launch_id.as_deref() == name.to_str())
    {
        return Ok(None);
    }
    let path = entry.path().join("manifest.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(io::Error::other(
            "Callback manifest exceeds the collection limit",
        ));
    }
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("Callback collection could not validate the manifest"))?;
    if Some(manifest.launch_id.as_str()) != name.to_str()
        || state
            .agent(super::AgentId(manifest.routing_key.0))
            .is_some()
    {
        return Ok(None);
    }
    Ok(Some(manifest))
}

/// Kimi's private home includes native conversations, not just callback data.
/// Move it intact, including its auth links, without traversing or copying them.
fn preserve_kimi_home(data_dir: &Path, capture: &Path) -> io::Result<()> {
    let home = capture.join("kimi-home");
    match fs::symlink_metadata(&home) {
        Ok(_) => real_directory(&home)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    let homes = data_dir.join("provider-homes");
    // Check each component so an existing parent link cannot redirect the move.
    crate::platform::fs::private_dir(&homes)?;
    let kimi = homes.join("kimi");
    crate::platform::fs::private_dir(&kimi)?;
    let destination = kimi.join(
        capture
            .file_name()
            .ok_or_else(|| io::Error::other("Missing callback launch name"))?,
    );
    match fs::symlink_metadata(&destination) {
        Ok(_) => return Err(io::Error::other("Retained Kimi home already exists")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(&home, &destination)?;
    crate::platform::sync_parent_directory(&kimi)?;
    crate::platform::sync_parent_directory(capture)?;
    tracing::info!(event = "bus.callbacks.kimi_home_retained", path = %destination.display(),
        "Deleted agent's native Kimi home retained outside callback storage");
    Ok(())
}
