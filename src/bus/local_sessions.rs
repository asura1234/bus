//! Local Bus session selection above Herdr's native resume machinery.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const METADATA_VERSION: u64 = 1;
const ID_HEX_LEN: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalSession {
    pub(crate) id: String,
    pub(crate) root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResumeTarget {
    Id(String),
    Last,
}

#[derive(Clone, Debug)]
pub(crate) struct LocalSessionRegistry {
    base: PathBuf,
}

#[derive(Debug, Deserialize, Serialize)]
struct Metadata {
    version: u64,
    id: String,
    created_at_ms: u64,
}

impl LocalSessionRegistry {
    pub(crate) fn new(base: PathBuf) -> Self {
        Self { base }
    }

    pub(crate) fn create(&self) -> Result<LocalSession, String> {
        super::io::private_dir(&self.base).map_err(|error| error.to_string())?;
        let sessions = self.base.join("sessions");
        super::io::private_dir(&sessions).map_err(|error| error.to_string())?;
        let _lease = super::io::append_lock(&self.base.join("session-registry.lock"))
            .map_err(|error| format!("Could not lock the local Bus session registry: {error}"))?;

        for attempt in 0..1024_u64 {
            let seed = format!("{}:{}:{attempt}", super::io::now_ns(), std::process::id());
            let id = super::io::digest(seed.as_bytes())[..ID_HEX_LEN].to_owned();
            let root = sessions.join(&id);
            if root.exists() {
                continue;
            }
            super::io::private_dir(&root).map_err(|error| error.to_string())?;
            let metadata = Metadata {
                version: METADATA_VERSION,
                id: id.clone(),
                created_at_ms: super::io::now_ms(),
            };
            let encoded =
                serde_json::to_vec_pretty(&metadata).map_err(|error| error.to_string())?;
            super::io::atomic_write(&root.join("session.json"), &encoded)
                .map_err(|error| error.to_string())?;
            self.write_last_unlocked(&id)?;
            return Ok(LocalSession { id, root });
        }
        Err("Could not allocate a unique local Bus session ID".into())
    }

    pub(crate) fn resume(&self, target: ResumeTarget) -> Result<LocalSession, String> {
        let id = match target {
            ResumeTarget::Id(id) => {
                validate_id(&id)?;
                id
            }
            ResumeTarget::Last => self.read_last()?,
        };
        let session = self.load(&id)?;
        self.write_last(&id)?;
        Ok(session)
    }

    fn load(&self, id: &str) -> Result<LocalSession, String> {
        let root = self.base.join("sessions").join(id);
        let directory = std::fs::symlink_metadata(&root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!("Bus session '{id}' was not found")
            } else {
                format!("Could not inspect Bus session '{id}': {error}")
            }
        })?;
        if !directory.is_dir() || directory.file_type().is_symlink() {
            return Err(format!("Bus session '{id}' metadata is invalid"));
        }
        let path = root.join("session.json");
        let file = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            .ok_or_else(|| format!("Bus session '{id}' metadata is invalid"))?;
        if file.len() > 4096 {
            return Err(format!("Bus session '{id}' metadata is invalid"));
        }
        let metadata: Metadata = serde_json::from_slice(
            &std::fs::read(&path).map_err(|_| format!("Bus session '{id}' metadata is invalid"))?,
        )
        .map_err(|_| format!("Bus session '{id}' metadata is invalid"))?;
        if metadata.version != METADATA_VERSION || metadata.id != id {
            return Err(format!("Bus session '{id}' metadata is invalid"));
        }
        Ok(LocalSession {
            id: id.to_owned(),
            root,
        })
    }

    fn read_last(&self) -> Result<String, String> {
        let value = std::fs::read_to_string(self.base.join("last-session")).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "No local Bus session has been recorded yet".to_owned()
            } else {
                format!("Could not read the last local Bus session: {error}")
            }
        })?;
        let id = value.trim().to_owned();
        validate_id(&id).map_err(|_| "The last local Bus session record is invalid".to_owned())?;
        Ok(id)
    }

    fn write_last(&self, id: &str) -> Result<(), String> {
        validate_id(id)?;
        super::io::private_dir(&self.base).map_err(|error| error.to_string())?;
        let _lease = super::io::append_lock(&self.base.join("session-registry.lock"))
            .map_err(|error| format!("Could not lock the local Bus session registry: {error}"))?;
        self.write_last_unlocked(id)
    }

    fn write_last_unlocked(&self, id: &str) -> Result<(), String> {
        super::io::atomic_write(&self.base.join("last-session"), id.as_bytes())
            .map_err(|error| format!("Could not record the last local Bus session: {error}"))
    }
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.len() == ID_HEX_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(format!("Invalid Bus session ID '{id}'"))
    }
}

pub(crate) fn default_base_dir() -> Result<PathBuf, String> {
    Ok(std::env::home_dir()
        .ok_or("Home directory unavailable")?
        .join(".local/share/bus"))
}
