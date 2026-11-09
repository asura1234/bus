//! A launch-validated binary stamp; cold restore performs metadata I/O only.
use crate::agents::providers::hook_json::{self, HookContext};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct Stamp {
    canonical_path: PathBuf,
    size: u64,
    modified: SystemTime,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl Stamp {
    fn read(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() || path.file_name().is_none_or(|name| name != "grok") {
            return Err("Saved Grok executable must be an absolute alias named grok".into());
        }
        let canonical_path = path.canonicalize().map_err(|e| e.to_string())?;
        let metadata = canonical_path.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_file() {
            return Err("Saved Grok executable is not a file".into());
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        #[cfg(unix)]
        if metadata.mode() & 0o111 == 0 {
            return Err("Saved Grok executable is not executable".into());
        }
        Ok(Self {
            canonical_path,
            size: metadata.len(),
            modified: metadata.modified().map_err(|e| e.to_string())?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SavedExecutable {
    pub(super) alias: PathBuf,
    stamp: Stamp,
}

const CHANGED_EXECUTABLE: &str =
    "Saved Grok executable changed; relaunch to validate this installation";

impl SavedExecutable {
    pub(super) fn verify(&self) -> Result<(), String> {
        if Stamp::read(&self.alias)? != self.stamp {
            return Err(CHANGED_EXECUTABLE.into());
        }
        super::driver_fixture(&self.alias)?;
        Ok(())
    }
}

pub(super) fn validate(path: &Path) -> Result<SavedExecutable, String> {
    let stamp = Stamp::read(path)?;
    super::validate_executable(path)?;
    let saved = SavedExecutable {
        alias: path.to_owned(),
        stamp,
    };
    saved.verify()?;
    Ok(saved)
}

pub(super) fn load(hooks: &HookContext) -> Result<PathBuf, String> {
    let saved: SavedExecutable = serde_json::from_value(hook_json::read_resume_json(
        &hooks.spool.join(super::EXECUTABLE_FILE),
    )?)
    .map_err(|_| "Saved Grok identity stamp is missing or obsolete; relaunch this agent")?;
    match saved.verify() {
        Ok(()) => Ok(saved.alias),
        Err(error) if error == CHANGED_EXECUTABLE => {
            refresh_changed_executable(hooks, &saved.alias)
        }
        Err(error) => Err(error),
    }
}

/// A provider auto-update keeps the same `grok` alias and changes its bytes.
/// Probe that alias, then replace the saved stamp. A missing or non-executable
/// file still fails in `Stamp::read` with its existing message.
fn refresh_changed_executable(hooks: &HookContext, alias: &Path) -> Result<PathBuf, String> {
    super::validate_executable(alias)?;
    super::driver_fixture(alias)?;
    let saved = SavedExecutable {
        alias: alias.to_owned(),
        stamp: Stamp::read(alias)?,
    };
    let bytes = serde_json::to_vec(&saved).map_err(|error| error.to_string())?;
    hook_json::atomic_write(&hooks.spool.join(super::EXECUTABLE_FILE), &bytes)
        .map_err(|error| error.to_string())?;
    tracing::info!(
        event = "bus.grok.executable_upgraded",
        alias = %alias.display(),
        "Saved Grok executable changed; revalidated and refreshed its stamp"
    );
    Ok(saved.alias)
}
