pub(crate) use crate::platform::fs::AtomicWriteStage as SaveStage;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) use super::journal::JournalOp;
use super::journal::{push_coalesced, replay, JournalDocument, JournalEntry};
use crate::messaging::model::BusState;

pub(crate) const STORE_VERSION: u64 = 1;

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
    /// The last journal entry this state already contains.
    #[serde(default)]
    journal_seq: u64,
}

/// What this store last read or wrote, so a save can trust its own file.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Fingerprint {
    fn of(path: &std::path::Path) -> Option<Self> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        metadata.is_file().then(|| Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Default)]
struct JournalState {
    /// The last sequence number used by a journal entry or a full save.
    seq: u64,
    /// Entries newer than the last full save, oldest first.
    entries: Vec<JournalEntry>,
    verified: Option<Fingerprint>,
}

pub(crate) struct JsonStore {
    path: PathBuf,
    journal: std::sync::Mutex<JournalState>,
    #[cfg(test)]
    fail_stage: std::sync::Mutex<Option<SaveStage>>,
}

impl JsonStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            journal: std::sync::Mutex::new(JournalState::default()),
            #[cfg(test)]
            fail_stage: std::sync::Mutex::new(None),
        }
    }

    fn journal_path(&self) -> PathBuf {
        self.path.with_extension("journal.json")
    }

    fn journal_state(&self) -> std::sync::MutexGuard<'_, JournalState> {
        self.journal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether journal entries still wait for a full save to fold them in.
    pub(crate) fn has_unfolded_journal(&self) -> bool {
        !self.journal_state().entries.is_empty()
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

    #[cfg(test)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The durable state: `state.json` plus every newer journal entry.
    pub(crate) fn load(&self) -> Result<Option<BusState>, StoreError> {
        let Some((mut state, base_seq)) = self.load_document()? else {
            return Ok(None);
        };
        let entries = self.load_journal()?;
        replay(&mut state, &entries, base_seq);
        let mut journal = self.journal_state();
        journal.seq = entries
            .iter()
            .map(|entry| entry.seq)
            .fold(base_seq, u64::max);
        journal.entries = entries
            .into_iter()
            .filter(|entry| entry.seq > base_seq)
            .collect();
        Ok(Some(state))
    }

    /// Exactly what the last full save wrote, without the journal.
    pub(crate) fn load_base(&self) -> Result<Option<BusState>, StoreError> {
        Ok(self.load_document()?.map(|(state, _)| state))
    }

    fn load_journal(&self) -> Result<Vec<JournalEntry>, StoreError> {
        let path = self.journal_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(StoreError::Io { path, source }),
        };
        let document = serde_json::from_slice::<JournalDocument>(&bytes).map_err(|source| {
            StoreError::Corrupt {
                path: path.clone(),
                source,
            }
        })?;
        if !document.supported() {
            return Err(StoreError::UnsupportedVersion {
                path,
                found: document.version,
            });
        }
        Ok(document.entries)
    }

    fn load_document(&self) -> Result<Option<(BusState, u64)>, StoreError> {
        let fingerprint = Fingerprint::of(&self.path);
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
        self.journal_state().verified = fingerprint;
        Ok(Some((document.state, document.journal_seq)))
    }

    pub(crate) fn save(&self, state: &BusState) -> Result<(), StoreError> {
        // Re-reading a large state on every save is most of its cost. A file
        // still exactly as this store last read or wrote it is known readable.
        let verified = self.journal_state().verified;
        if self.path.exists() && (verified.is_none() || Fingerprint::of(&self.path) != verified) {
            self.load_document()
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
            bytes: 0,
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Bus state path has no parent directory",
            ),
        })?;
        std::fs::create_dir_all(parent).map_err(|source| StoreError::SaveIo {
            path: parent.to_path_buf(),
            stage: SaveStage::Parent,
            bytes: 0,
            source,
        })?;
        let journal_seq = self.journal_state().seq;
        let bytes = serde_json::to_vec_pretty(&StoredDocument {
            version: STORE_VERSION,
            state: state.clone(),
            journal_seq,
        })
        .map_err(|source| StoreError::Corrupt {
            path: self.path.clone(),
            source,
        })?;
        crate::platform::fs::atomic_write_detailed_with_checkpoint(&self.path, &bytes, |stage| {
            self.injected_failure(stage)
        })
        .map_err(|error| StoreError::SaveIo {
            path: error.path,
            stage: error.stage,
            bytes: bytes.len(),
            source: error.source,
        })?;
        let mut journal = self.journal_state();
        journal.verified = Fingerprint::of(&self.path);
        // The saved state contains every entry up to `journal_seq`; loading
        // ignores them, so removing the file only keeps the directory tidy.
        journal.entries.retain(|entry| entry.seq > journal_seq);
        if journal.entries.is_empty() {
            let _ = std::fs::remove_file(self.journal_path());
        }
        Ok(())
    }

    /// Durably records `op`, already applied by the caller to its in-memory
    /// state, without rewriting `state.json`.
    pub(crate) fn journal(&self, op: JournalOp) -> Result<(), StoreError> {
        let mut journal = self.journal_state();
        let mut entries = journal.entries.clone();
        push_coalesced(
            &mut entries,
            JournalEntry {
                seq: journal.seq + 1,
                op,
            },
        );
        let path = self.journal_path();
        let bytes =
            serde_json::to_vec(&JournalDocument::new(entries.clone())).map_err(|source| {
                StoreError::Corrupt {
                    path: path.clone(),
                    source,
                }
            })?;
        crate::platform::fs::atomic_write_detailed_with_checkpoint(&path, &bytes, |stage| {
            self.injected_failure(stage)
        })
        .map_err(|error| StoreError::SaveIo {
            path: error.path,
            stage: error.stage,
            bytes: bytes.len(),
            source: error.source,
        })?;
        journal.seq += 1;
        journal.entries = entries;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/state_store_test.rs"]
mod tests;
