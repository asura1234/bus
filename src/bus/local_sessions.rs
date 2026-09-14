//! Local Bus session selection above Herdr's native resume machinery.

use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const METADATA_VERSION: u64 = 1;
const ID_HEX_LEN: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalSession {
    pub(crate) id: String,
    pub(crate) root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalSessionSummary {
    pub(crate) id: String,
    pub(crate) room_names: Vec<String>,
    pub(crate) last_activity_ms: u64,
    pub(crate) is_last: bool,
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

    pub(crate) fn list(&self) -> Result<Vec<LocalSessionSummary>, String> {
        let sessions_dir = self.base.join("sessions");
        let entries = match std::fs::read_dir(&sessions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("Could not list local Bus sessions: {error}")),
        };
        let last = self.read_last().ok();
        let mut summaries = Vec::new();
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if validate_id(&id).is_err() {
                continue;
            }
            let Ok(session) = self.load(&id) else {
                continue;
            };
            let Ok(Some(state)) = self.read_state(&session) else {
                continue;
            };
            let room_names = state
                .rooms()
                .map(|room| room.name.clone())
                .collect::<Vec<_>>();
            if room_names.is_empty() {
                continue;
            }
            let metadata = self.load_metadata(&id)?;
            let last_activity_ms = std::fs::metadata(session.root.join("state.json"))
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(epoch_millis)
                .unwrap_or(metadata.created_at_ms);
            summaries.push(LocalSessionSummary {
                is_last: last.as_deref() == Some(id.as_str()),
                id,
                room_names,
                last_activity_ms,
            });
        }
        summaries.sort_by(|left, right| {
            right
                .last_activity_ms
                .cmp(&left.last_activity_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(summaries)
    }

    pub(crate) fn is_empty(&self, id: &str) -> Result<bool, String> {
        validate_id(id)?;
        let session = self.load(id)?;
        Ok(self
            .read_state(&session)?
            .is_none_or(|state| state.rooms().next().is_none()))
    }

    pub(crate) fn discard_if_empty(&self, id: &str) -> Result<bool, String> {
        validate_id(id)?;
        if !self.is_empty(id)? {
            return Ok(false);
        }
        let _registry = super::io::append_lock(&self.base.join("session-registry.lock"))
            .map_err(|error| format!("Could not lock the local Bus session registry: {error}"))?;
        let session = self.load(id)?;
        if self
            .read_state(&session)?
            .is_some_and(|state| state.rooms().next().is_some())
        {
            return Ok(false);
        }
        let coordinator = match super::io::lock(&session.root.join("coordinator.lock")) {
            Ok(coordinator) => coordinator,
            Err(_) => return Ok(false),
        };
        drop(coordinator);
        std::fs::remove_dir_all(&session.root)
            .map_err(|error| format!("Could not discard empty Bus session '{id}': {error}"))?;
        if self.read_last().ok().as_deref() == Some(id) {
            if let Some(replacement) = self.list()?.first() {
                self.write_last_unlocked(&replacement.id)?;
            } else {
                match std::fs::remove_file(self.base.join("last-session")) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!(
                            "Could not clear the last local Bus session: {error}"
                        ));
                    }
                }
            }
        }
        Ok(true)
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
        self.load_metadata(id)?;
        Ok(LocalSession {
            id: id.to_owned(),
            root,
        })
    }

    fn load_metadata(&self, id: &str) -> Result<Metadata, String> {
        let path = self.base.join("sessions").join(id).join("session.json");
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
        Ok(metadata)
    }

    fn read_state(&self, session: &LocalSession) -> Result<Option<super::model::BusState>, String> {
        let path = session.root.join("state.json");
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(format!("Bus session '{}' state is invalid", session.id)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "Could not inspect Bus session '{}' state: {error}",
                    session.id
                ));
            }
        }
        super::store::JsonStore::new(path)
            .load()
            .map_err(|_| format!("Bus session '{}' state is invalid", session.id))
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

fn epoch_millis(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as u64)
}

fn relative_age(now_ms: u64, then_ms: u64) -> String {
    let elapsed = now_ms.saturating_sub(then_ms);
    const MINUTE: u64 = 60_000;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    if elapsed < MINUTE {
        "now".into()
    } else if elapsed < HOUR {
        format!("{}m ago", elapsed / MINUTE)
    } else if elapsed < DAY {
        format!("{}h ago", elapsed / HOUR)
    } else {
        format!("{}d ago", elapsed / DAY)
    }
}

fn safe_room_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn format_session_list(sessions: &[LocalSessionSummary], now_ms: u64) -> String {
    sessions
        .iter()
        .map(|session| {
            let rooms = session
                .room_names
                .iter()
                .map(|name| format!("[# {}]", safe_room_name(name)))
                .collect::<Vec<_>>()
                .join(" ");
            let last = if session.is_last { "  ← last" } else { "" };
            format!(
                "{}  {}  {}{}",
                session.id,
                rooms,
                relative_age(now_ms, session.last_activity_ms),
                last
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
