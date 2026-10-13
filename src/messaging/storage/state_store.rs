pub(crate) use crate::platform::fs::AtomicWriteStage as SaveStage;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub(crate) use super::journal::JournalOp;
use super::journal::{push_coalesced, replay, JournalDocument, JournalEntry};
use crate::messaging::model::{Archived, BusState, History, HISTORY_RETENTION_MS};
use std::sync::Arc;

/// Version 2 keeps settled history in `history.jsonl`; a version 1 file, which
/// still holds it inline, loads as is and is archived from on.
pub(crate) const STORE_VERSION: u64 = 2;
const INLINE_HISTORY_VERSION: u64 = 1;

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

/// The version is checked before this is parsed, so it is not kept here.
#[derive(Deserialize)]
struct StoredDocument {
    state: BusState,
    /// The last journal entry this state already contains.
    #[serde(default)]
    journal_seq: u64,
}

/// [`StoredDocument`] as written: it borrows the state instead of copying it.
#[derive(Serialize)]
struct StoredDocumentRef<'a> {
    version: u64,
    state: &'a BusState,
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
    /// The history `history.jsonl` holds, as of the last load or append.
    history: std::sync::Mutex<Arc<History>>,
    /// The file holds lines the last load dropped; `compact_history` rewrites it.
    history_stale: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_stage: std::sync::Mutex<Option<SaveStage>>,
    /// Completed `state.json` writes, so tests can count saves.
    #[cfg(test)]
    writes: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    fail_history: std::sync::atomic::AtomicBool,
}

impl JsonStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            journal: std::sync::Mutex::new(JournalState::default()),
            history: std::sync::Mutex::new(Arc::default()),
            history_stale: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_stage: std::sync::Mutex::new(None),
            #[cfg(test)]
            writes: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            fail_history: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The append-only archive of settled requests and old notices.
    pub(crate) fn history_path(&self) -> PathBuf {
        self.path.with_file_name("history.jsonl")
    }

    /// Makes the next history append fail, as a full disk would.
    #[cfg(test)]
    pub(crate) fn fail_history_once(&self) {
        self.fail_history
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Rewrites `history.jsonl` without the entries the last load dropped
    /// (expired, of deleted rooms or agents, duplicated, or a torn last line).
    /// True when it rewrote the file. Only the coordinator, which owns the
    /// data directory, may call it.
    pub(crate) fn compact_history(&self) -> Result<bool, StoreError> {
        if !self
            .history_stale
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Ok(false);
        }
        let history = self.history_lock().clone();
        let bytes = history_lines(history.items())?;
        let path = self.history_path();
        crate::platform::fs::atomic_write_detailed_with_checkpoint(&path, &bytes, |stage| {
            self.injected_failure(stage)
        })
        .map_err(|error| StoreError::SaveIo {
            path: error.path,
            stage: error.stage,
            bytes: bytes.len(),
            source: error.source,
        })?;
        self.history_stale
            .store(false, std::sync::atomic::Ordering::Release);
        Ok(true)
    }

    fn history_lock(&self) -> std::sync::MutexGuard<'_, Arc<History>> {
        self.history
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads `history.jsonl`, keeping what `state` can still show: entries
    /// younger than the retention, of rooms and agents that still exist, and
    /// not also live in `state` (a crash between an append and its save).
    /// A torn last line is skipped; any other unreadable line is corruption.
    fn load_history(&self, state: &BusState) -> Result<Arc<History>, StoreError> {
        let path = self.history_path();
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(source) => return Err(StoreError::Io { path, source }),
        };
        let cutoff = super::io::now_ms().saturating_sub(HISTORY_RETENTION_MS);
        let mut history = History::default();
        let mut stale = false;
        let lines: Vec<&[u8]> = bytes.split(|byte| *byte == b'\n').collect();
        for (index, line) in lines.iter().enumerate() {
            let last = index + 1 == lines.len();
            if line.is_empty() {
                continue;
            }
            // An append writes whole lines and syncs before state.json changes,
            // so a last line without its newline was never committed.
            if last {
                stale = true;
                continue;
            }
            let item = match serde_json::from_slice::<Archived>(line) {
                Ok(item) => item,
                Err(source) => {
                    return Err(StoreError::Corrupt {
                        path: path.clone(),
                        source,
                    })
                }
            };
            let kept = item.settled_at_ms() >= cutoff
                && state.room(item.room()).is_some()
                && match &item {
                    Archived::Request(request) => {
                        state.agent(request.agent_id).is_some()
                            && state.request(request.id).is_none()
                    }
                    Archived::Notice { room, prompt } => {
                        state.notices(*room).all(|live| live.id != prompt.id)
                    }
                };
            stale |= !kept || history.contains(&item);
            if kept {
                history.insert(item);
            }
        }
        let history = Arc::new(history);
        *self.history_lock() = Arc::clone(&history);
        self.history_stale
            .store(stale, std::sync::atomic::Ordering::Release);
        Ok(history)
    }

    /// Makes the archive entries `state` added since the last append durable,
    /// before `state.json` stops holding them.
    fn append_history(&self, state: &BusState) -> Result<(), StoreError> {
        let known = self.history_lock().clone();
        if Arc::ptr_eq(&known, state.history()) {
            return Ok(());
        }
        let bytes = history_lines(state.history().added_since(&known))?;
        if !bytes.is_empty() {
            self.append_history_bytes(&bytes)?;
        }
        *self.history_lock() = Arc::clone(state.history());
        Ok(())
    }

    fn append_history_bytes(&self, bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.history_path();
        let failed = |stage: SaveStage, source: std::io::Error| StoreError::SaveIo {
            path: path.clone(),
            stage,
            bytes: bytes.len(),
            source,
        };
        #[cfg(test)]
        if self
            .fail_history
            .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            return Err(failed(
                SaveStage::TempWrite,
                std::io::Error::from_raw_os_error(28),
            ));
        }
        let created = !path.exists();
        let mut options = std::fs::OpenOptions::new();
        options.create(true).read(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&path)
            .map_err(|source| failed(SaveStage::TempCreate, source))?;
        // A failed or interrupted append can leave part of a line; it was never
        // committed, and appending after it would bury it mid-file.
        let complete =
            complete_length(&mut file).map_err(|source| failed(SaveStage::TempCreate, source))?;
        file.set_len(complete)
            .map_err(|source| failed(SaveStage::TempWrite, source))?;
        let written = std::io::Write::write_all(&mut file, bytes)
            .map_err(|source| failed(SaveStage::TempWrite, source))
            .and_then(|()| {
                file.sync_all()
                    .map_err(|source| failed(SaveStage::TempSync, source))
            });
        if let Err(error) = written {
            let _ = file.set_len(complete).and_then(|()| file.sync_all());
            return Err(error);
        }
        if created {
            if let Some(parent) = path.parent() {
                crate::platform::sync_parent_directory(parent)
                    .map_err(|source| failed(SaveStage::ParentSync, source))?;
            }
        }
        Ok(())
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

    /// How many times this store wrote `state.json`.
    #[cfg(test)]
    pub(crate) fn writes(&self) -> u64 {
        self.writes.load(std::sync::atomic::Ordering::Relaxed)
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
        state.adopt_version_one_guards(super::io::now_ms());
        let history = self.load_history(&state)?;
        let state = state.with_history(history);
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
        if version != STORE_VERSION && version != INLINE_HISTORY_VERSION {
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
        let _timer = crate::messaging::perf::STORE_SAVE.start();
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
        self.append_history(state)?;
        let journal_seq = self.journal_state().seq;
        // Compact: the file is rewritten whole on every save, and every
        // reader parses it with serde, which ignores whitespace.
        let bytes = serde_json::to_vec(&StoredDocumentRef {
            version: STORE_VERSION,
            state,
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
        #[cfg(test)]
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        let _timer = crate::messaging::perf::STORE_JOURNAL.start();
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

/// The length of `file` up to and including its last newline: everything
/// after it is a line an interrupted append left incomplete.
fn complete_length(file: &mut std::fs::File) -> std::io::Result<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let length = file.metadata()?.len();
    let mut end = length;
    let mut chunk = vec![0; 64 * 1024];
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        let size = usize::try_from(end - start).unwrap_or(chunk.len());
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut chunk[..size])?;
        if let Some(newline) = chunk[..size].iter().rposition(|byte| *byte == b'\n') {
            return Ok(start + newline as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

/// One JSON line per archived item.
fn history_lines(items: impl Iterator<Item = Archived>) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    for item in items {
        serde_json::to_writer(&mut bytes, &item).map_err(|source| StoreError::Corrupt {
            path: PathBuf::from("history.jsonl"),
            source,
        })?;
        bytes.push(b'\n');
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "tests/state_store_test.rs"]
mod tests;
