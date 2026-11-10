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
            if manifest.provider == ProviderKind::Kimi && has_native_home(&path)? {
                tracing::info!(event = "bus.callbacks.native_home_retained",
                    agent_id = manifest.routing_key.0, launch_id = %manifest.launch_id,
                    "Kimi capture retained at its native session home path");
                return Ok(());
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

/// A deleted owner can leave a moved native session running. Kimi's home and
/// session index contain absolute paths, so even relocation would break it.
/// Retain uncertain home entries too, without following their links.
fn has_native_home(capture: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(capture.join("kimi-home")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
