//! Read-only ownership of a returned draft, including its original bytes.
use super::{
    has_ack, marker, preserved_recovery, read_regular, read_request, try_lock, valid_token,
    Operation, PreservedDraft, MAX_BYTES, MAX_RECORD_BYTES, PRESERVED,
};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

pub(crate) struct MarkedDraft {
    // Exclude native callbacks from loading the proof through repair.
    _lock: File,
    record: PathBuf,
    recovery: PathBuf,
    marked: String,
    marker_start: usize,
}

impl MarkedDraft {
    pub(crate) fn open(dir: &Path) -> io::Result<Self> {
        let lock = try_lock(dir)?;
        let record = dir.join(PRESERVED);
        let saved: PreservedDraft =
            serde_json::from_slice(&read_regular(&record, MAX_RECORD_BYTES)?)?;
        if !valid_token(&saved.token) {
            return Err(io::Error::other("Invalid preserved draft token"));
        }
        if read_request(dir)?.is_some_and(|request| {
            request.token != saved.token
                || request.operation != Operation::Cancelled
                    && request.expires_ms > crate::utils::time::now_ms()
                    && !has_ack(dir, &request)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Editor handoff is still active",
            ));
        }
        let recovery = preserved_recovery(dir, &saved.token);
        let mut marked =
            String::from_utf8(read_regular(&recovery, MAX_BYTES)?).map_err(io::Error::other)?;
        let marker_start = marked.len();
        marked.push_str(&marker(&saved.token));
        Ok(Self {
            _lock: lock,
            record,
            recovery,
            marked,
            marker_start,
        })
    }

    pub(crate) fn marked(&self) -> &str {
        &self.marked
    }

    pub(crate) fn original(&self) -> &str {
        &self.marked[..self.marker_start]
    }

    pub(crate) fn marker_len(&self) -> usize {
        self.marked.len() - self.marker_start
    }

    /// Called only after the exact original draft is visible again.
    pub(crate) fn retire(self) -> io::Result<()> {
        fs::remove_file(&self.record)?;
        fs::remove_file(&self.recovery)
    }
}
