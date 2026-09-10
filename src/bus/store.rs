use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use super::model::BusState;

pub(crate) const STORE_VERSION: u64 = 1;

#[derive(Debug)]
pub(crate) enum StoreError {
    Io {
        path: PathBuf,
        source: std::io::Error,
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

#[derive(Deserialize, Serialize)]
struct StoredDocument {
    version: u64,
    state: BusState,
}

pub(crate) struct JsonStore {
    path: PathBuf,
}

impl JsonStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
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
        let parent = self.path.parent().ok_or_else(|| StoreError::Io {
            path: self.path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Bus state path has no parent directory",
            ),
        })?;
        std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
            path: parent.to_path_buf(),
            source,
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
        let mut temp =
            crate::platform::create_private_state_file(&temp_path).map_err(|source| {
                StoreError::Io {
                    path: temp_path.clone(),
                    source,
                }
            })?;
        if let Err(source) = temp.write_all(&bytes).and_then(|()| temp.sync_all()) {
            drop(temp);
            let _ = std::fs::remove_file(&temp_path);
            return Err(StoreError::Io {
                path: temp_path,
                source,
            });
        }
        drop(temp);
        if let Err(source) = crate::platform::replace_file(&temp_path, &self.path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(StoreError::Io {
                path: self.path.clone(),
                source,
            });
        }
        crate::platform::sync_parent_directory(parent).map_err(|source| StoreError::Io {
            path: parent.to_path_buf(),
            source,
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
}
