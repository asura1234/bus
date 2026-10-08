use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use super::model::BusState;

pub(crate) const STORE_VERSION: u64 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SaveStage {
    Parent,
    TempCreate,
    TempWrite,
    TempSync,
    Replace,
    ParentSync,
}

impl SaveStage {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::TempCreate => "temp_create",
            Self::TempWrite => "temp_write",
            Self::TempSync => "temp_sync",
            Self::Replace => "replace",
            Self::ParentSync => "parent_sync",
        }
    }
}

#[derive(Debug)]
pub(crate) enum StoreError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    SaveIo {
        path: PathBuf,
        stage: SaveStage,
        source: std::io::Error,
        bytes: usize,
    },
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
    UnsupportedVersion {
        path: PathBuf,
        found: u64,
    },
    ExistingStateUnreadable {
        path: PathBuf,
        detail: String,
    },
    UnsafePath(PathBuf),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "Bus storage {}: {source}", path.display())
            }
            Self::SaveIo {
                path,
                stage,
                source,
                ..
            } => {
                write!(
                    formatter,
                    "Bus storage {} ({}): {source}",
                    path.display(),
                    stage.name()
                )
            }
            Self::Corrupt { path, source } => {
                write!(formatter, "Corrupt Bus state {}: {source}", path.display())
            }
            Self::UnsupportedVersion { path, found } => write!(
                formatter,
                "Unsupported Bus state version {found} in {}",
                path.display()
            ),
            Self::ExistingStateUnreadable { path, detail } => write!(
                formatter,
                "Refusing to replace unreadable Bus state {}: {detail}",
                path.display()
            ),
            Self::UnsafePath(path) => {
                write!(formatter, "Unsafe Bus state path: {}", path.display())
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl StoreError {
    pub(crate) fn diagnostic(&self) -> (&'static str, Option<i32>, usize) {
        match self {
            Self::SaveIo {
                stage,
                source,
                bytes,
                ..
            } => (stage.name(), source.raw_os_error(), *bytes),
            Self::Io { source, .. } => ("read", source.raw_os_error(), 0),
            Self::Corrupt { .. } => ("parse", None, 0),
            Self::UnsupportedVersion { .. } => ("version", None, 0),
            Self::ExistingStateUnreadable { .. } => ("preflight", None, 0),
            Self::UnsafePath(_) => ("path", None, 0),
        }
    }

    pub(crate) fn retryable_io(&self) -> bool {
        matches!(self, Self::Io { .. } | Self::SaveIo { .. })
    }
}

#[derive(Deserialize, Serialize)]
struct StoredDocument {
    version: u64,
    state: BusState,
}

pub(crate) struct JsonStore {
    path: PathBuf,
    #[cfg(test)]
    fail_stage: std::sync::Mutex<Option<SaveStage>>,
}

impl JsonStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            #[cfg(test)]
            fail_stage: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn fail_once_at(&self, stage: SaveStage) {
        *self.fail_stage.lock().unwrap() = Some(stage);
    }

    fn injected_failure(&self, stage: SaveStage) -> std::io::Result<()> {
        #[cfg(test)]
        {
            let mut fail_stage = self.fail_stage.lock().unwrap();
            if *fail_stage == Some(stage) {
                *fail_stage = None;
                return Err(std::io::Error::from_raw_os_error(28));
            }
        }
        let _ = stage;
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn load(&self) -> Result<Option<BusState>, StoreError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(StoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        let value = serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|source| {
            StoreError::Corrupt {
                path: self.path.clone(),
                source,
            }
        })?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| StoreError::Corrupt {
                path: self.path.clone(),
                source: serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "missing numeric Bus state version",
                )),
            })?;
        if version != STORE_VERSION {
            return Err(StoreError::UnsupportedVersion {
                path: self.path.clone(),
                found: version,
            });
        }
        let document = serde_json::from_value::<StoredDocument>(value).map_err(|source| {
            StoreError::Corrupt {
                path: self.path.clone(),
                source,
            }
        })?;
        Ok(Some(document.state))
    }

    pub(crate) fn save(&self, state: &BusState) -> Result<(), StoreError> {
        if self.path.exists() {
            self.load()
                .map_err(|error| StoreError::ExistingStateUnreadable {
                    path: self.path.clone(),
                    detail: error.to_string(),
                })?;
        }
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(StoreError::UnsafePath(self.path.clone()));
            }
        }
        let parent = self.path.parent().ok_or_else(|| StoreError::SaveIo {
            path: self.path.clone(),
            stage: SaveStage::Parent,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Bus state path has no parent directory",
            ),
            bytes: 0,
        })?;
        std::fs::create_dir_all(parent).map_err(|source| StoreError::SaveIo {
            path: parent.to_path_buf(),
            stage: SaveStage::Parent,
            source,
            bytes: 0,
        })?;
        let bytes = serde_json::to_vec_pretty(&StoredDocument {
            version: STORE_VERSION,
            state: state.clone(),
        })
        .map_err(|source| StoreError::Corrupt {
            path: self.path.clone(),
            source,
        })?;
        static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let stem = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state.json");
        let temp_path = parent.join(format!(".{stem}.tmp-{}-{sequence}", std::process::id()));
        let mut temp = self
            .injected_failure(SaveStage::TempCreate)
            .and_then(|()| crate::platform::create_private_state_file(&temp_path))
            .map_err(|source| StoreError::SaveIo {
                path: temp_path.clone(),
                stage: SaveStage::TempCreate,
                source,
                bytes: bytes.len(),
            })?;
        if let Err(source) = self
            .injected_failure(SaveStage::TempWrite)
            .and_then(|()| temp.write_all(&bytes))
        {
            drop(temp);
            let _ = std::fs::remove_file(&temp_path);
            return Err(StoreError::SaveIo {
                path: temp_path,
                stage: SaveStage::TempWrite,
                source,
                bytes: bytes.len(),
            });
        }
        if let Err(source) = self
            .injected_failure(SaveStage::TempSync)
            .and_then(|()| temp.sync_all())
        {
            drop(temp);
            let _ = std::fs::remove_file(&temp_path);
            return Err(StoreError::SaveIo {
                path: temp_path,
                stage: SaveStage::TempSync,
                source,
                bytes: bytes.len(),
            });
        }
        drop(temp);
        if let Err(source) = self
            .injected_failure(SaveStage::Replace)
            .and_then(|()| crate::platform::replace_file(&temp_path, &self.path))
        {
            let _ = std::fs::remove_file(&temp_path);
            return Err(StoreError::SaveIo {
                path: self.path.clone(),
                stage: SaveStage::Replace,
                source,
                bytes: bytes.len(),
            });
        }
        self.injected_failure(SaveStage::ParentSync)
            .and_then(|()| crate::platform::sync_parent_directory(parent))
            .map_err(|source| StoreError::SaveIo {
                path: parent.to_path_buf(),
                stage: SaveStage::ParentSync,
                source,
                bytes: bytes.len(),
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::*;
    use crate::bus::model::{
        AgentRuntimeIdentity, CallbackDisposition, CallbackRejection, Provider, ProviderCallback,
        RequestPhase, SubmissionOutcome,
    };

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bus-store-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn atomic_store_round_trips_versioned_state_and_ignores_interrupted_temp_file() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("state.json");
        let store = JsonStore::new(path.clone());
        let mut state = BusState::new();
        state.create_room("durable").expect("room");
        store.save(&state).expect("save");
        fs::write(dir.join("state.json.tmp-interrupted"), b"{not json").expect("interrupted temp");

        let loaded = store.load().expect("load").expect("state");
        assert_eq!(loaded, state);
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(path).expect("read")).expect("json");
        assert_eq!(document["version"], STORE_VERSION);
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn corrupt_or_unknown_version_state_is_explicit_and_never_overwritten() {
        let dir = temp_dir("corrupt");
        let path = dir.join("state.json");
        fs::write(&path, b"{broken").expect("corrupt");
        let store = JsonStore::new(path.clone());
        assert!(matches!(store.load(), Err(StoreError::Corrupt { .. })));
        assert!(matches!(
            store.save(&BusState::new()),
            Err(StoreError::ExistingStateUnreadable { .. })
        ));
        assert_eq!(fs::read(&path).expect("unchanged"), b"{broken");

        fs::write(&path, br#"{"version":999,"state":{}}"#).expect("version");
        assert!(matches!(
            store.load(),
            Err(StoreError::UnsupportedVersion { found: 999, .. })
        ));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn reload_preserves_uncertain_ownership_queue_and_consumed_callbacks() {
        let dir = temp_dir("uncertain");
        let store = JsonStore::new(dir.join("state.json"));
        let mut state = BusState::new();
        let room = state.create_room("durable").expect("room");
        let agent = state
            .create_agent(
                room,
                "builder",
                Provider::Codex,
                PathBuf::from("/repo"),
                Some("main".into()),
            )
            .expect("agent");
        state
            .set_agent_runtime_identity(
                agent,
                AgentRuntimeIdentity {
                    launch_id: Some("launch-codex".into()),
                    terminal_id: None,
                    pane_id: None,
                    session_id: None,
                },
            )
            .expect("identity");
        state.set_draft_text(room, "first").expect("first text");
        state
            .set_draft_recipients(room, [agent])
            .expect("first recipient");
        let first = state.submit_draft(room, 1).expect("first submit")[0];
        state.set_draft_text(room, "second").expect("second text");
        state
            .set_draft_recipients(room, [agent])
            .expect("second recipient");
        let second = state.submit_draft(room, 2).expect("second submit")[0];
        state
            .begin_submission(first, "launch-codex", 10)
            .expect("begin first");
        state
            .record_submission(
                first,
                SubmissionOutcome::Uncertain {
                    message: "response lost".into(),
                },
            )
            .expect("uncertain");
        let consumed = ProviderCallback::final_event(
            "consumed-before-save",
            11,
            agent,
            "launch-codex",
            "provider-session",
            "turn-1",
            "first",
            "unbound",
        );
        assert_eq!(
            state.accept_callback(consumed.clone()),
            CallbackDisposition::Rejected(CallbackRejection::UnboundFinal)
        );
        store.save(&state).expect("save");

        let mut loaded = store.load().expect("load").expect("state");
        assert_eq!(
            loaded.agent(agent).expect("agent").current_request,
            Some(first)
        );
        let request = loaded.request(first).expect("first request");
        assert_eq!(request.phase, RequestPhase::Submitting);
        assert!(request.uncertain_outcome);
        assert_eq!(loaded.next_queued_request(agent), None);
        assert_eq!(loaded.queued_requests(agent), &[second]);
        assert_eq!(
            loaded.accept_callback(consumed),
            CallbackDisposition::Rejected(CallbackRejection::DuplicateCallback)
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn saved_sound_orchestrator_compactions_notes_and_draft_reload() {
        use crate::bus::model::RoomKind;

        let dir = temp_dir("compat-fields");
        let store = JsonStore::new(dir.join("state.json"));
        let mut state = BusState::new();
        let master = state.ensure_master_room();
        let work = state.create_room("work").expect("room");
        state.set_room_notes(work, "keep-notes").expect("notes");
        state.set_draft_text(work, "keep-draft").expect("draft");
        state.set_room_sound(master, false).expect("master sound");
        state.set_room_sound(work, true).expect("work sound");
        let agent = state
            .create_agent(
                master,
                "orch",
                Provider::ClaudeCode,
                PathBuf::from("/repo"),
                None,
            )
            .expect("agent");
        state.bind_orchestrator(agent, work).expect("assign");
        state.record_compaction(agent, 50).expect("compaction");
        store.save(&state).expect("save");

        let mut loaded = store.load().expect("load").expect("state");
        assert_eq!(loaded.room(work).expect("work").notes, "keep-notes");
        assert_eq!(loaded.room(work).expect("work").draft.text, "keep-draft");
        assert!(!loaded.room(master).expect("master").sound_enabled());
        assert!(loaded.room(work).expect("work").sound_enabled());
        assert_eq!(loaded.agent(agent).expect("agent").orchestrates, Some(work));
        assert_eq!(loaded.agent(agent).expect("agent").compactions.count, 1);
        assert_eq!(
            loaded.agent(agent).expect("agent").compactions.last_at_ms,
            Some(50)
        );
        loaded.ensure_master_room();
        assert_eq!(loaded.agent(agent).expect("agent").orchestrates, Some(work));
        assert_eq!(
            loaded
                .rooms()
                .filter(|room| room.kind == RoomKind::Master)
                .count(),
            1
        );
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
