//! Transcript evidence for Claude Code 2.1.316's tool-progress cancel path.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const VERSION: &str = "2.1.316";
const MAX_TAIL: u64 = 2 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
pub(crate) struct TranscriptAnchor {
    path: PathBuf,
    device: u64,
    inode: u64,
    session: String,
    prompt: String,
}

#[derive(Deserialize, Serialize)]
struct StoredAnchor<A> {
    terminal: String,
    request: u64,
    anchor: A,
}

impl TranscriptAnchor {
    pub(crate) fn from_hook(path: &Path, session: &str, prompt: &str) -> Option<Self> {
        if !path.is_absolute()
            || session.is_empty()
            || prompt.is_empty()
            || path.file_name()?.to_str()? != format!("{session}.jsonl")
        {
            return None;
        }
        let metadata = fs::symlink_metadata(path).ok()?;
        let (device, inode) = file_identity(&metadata)?;
        metadata.is_file().then(|| Self {
            path: path.into(),
            device,
            inode,
            session: session.into(),
            prompt: prompt.into(),
        })
    }

    pub(crate) fn save(
        &self,
        root: &Path,
        launch_id: &str,
        terminal: &str,
        request: u64,
    ) -> std::io::Result<()> {
        let path =
            anchor_path(root, launch_id).ok_or_else(|| std::io::Error::other("Invalid launch"))?;
        crate::platform::fs::guarded_atomic_write(
            &path,
            &serde_json::to_vec(&StoredAnchor {
                terminal: terminal.into(),
                request,
                anchor: self,
            })
            .map_err(std::io::Error::other)?,
        )
    }

    pub(crate) fn load(
        root: &Path,
        launch_id: &str,
        session: &str,
        prompt_id: &str,
        terminal: &str,
        request: u64,
    ) -> Option<Self> {
        let file = open_transcript(&anchor_path(root, launch_id)?)?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || metadata.len() > 8192 {
            return None;
        }
        let stored: StoredAnchor<Self> = serde_json::from_reader(file.take(8192)).ok()?;
        (stored.terminal == terminal
            && stored.request == request
            && stored.anchor.session == session
            && stored.anchor.prompt == prompt_id)
            .then_some(stored.anchor)
    }

    pub(crate) fn watermark(&self, session: &str) -> Option<CancelWatermark<'_>> {
        if self.session != session {
            return None;
        }
        let mut file = open_transcript(&self.path)?;
        let metadata = file.metadata().ok()?;
        if !metadata.is_file() || file_identity(&metadata) != Some((self.device, self.inode)) {
            return None;
        }
        let offset = metadata.len();
        let prefix_len = usize::try_from(offset.min(512)).ok()?;
        let mut prefix = vec![0; prefix_len];
        file.seek(SeekFrom::Start(offset - prefix_len as u64))
            .ok()?;
        file.read_exact(&mut prefix).ok()?;
        prefix.ends_with(b"\n").then_some(CancelWatermark {
            anchor: self,
            file,
            offset,
            prefix,
            checked: None,
        })
    }
}

fn anchor_path(root: &Path, launch_id: &str) -> Option<PathBuf> {
    let mut components = Path::new(launch_id).components();
    let std::path::Component::Normal(launch) = components.next()? else {
        return None;
    };
    if components.next().is_some() || launch != std::ffi::OsStr::new(launch_id) {
        return None;
    }
    let directory = root.join("callbacks").join(launch);
    fs::symlink_metadata(&directory)
        .ok()?
        .is_dir()
        .then(|| directory.join("retirement-anchor"))
}

pub(crate) struct CancelWatermark<'a> {
    anchor: &'a TranscriptAnchor,
    file: File,
    offset: u64,
    prefix: Vec<u8>,
    checked: Option<(u64, bool)>,
}

/// This value can only be constructed from a fresh linked provider cancel row.
pub(crate) struct ToolProgressCancellation;

struct Row {
    offset: u64,
    value: Value,
}

impl CancelWatermark<'_> {
    pub(crate) fn has_cancellation(&mut self) -> bool {
        let Some(length) = self.file.metadata().ok().map(|metadata| metadata.len()) else {
            return false;
        };
        if let Some((_, evidence)) = self.checked.filter(|(checked, _)| *checked == length) {
            return evidence;
        }
        let evidence = self.cancellation().is_some();
        self.checked = Some((length, evidence));
        evidence
    }

    pub(crate) fn cancellation(&mut self) -> Option<ToolProgressCancellation> {
        let end = self.file.metadata().ok()?.len();
        let start = end.saturating_sub(MAX_TAIL);
        let metadata = fs::symlink_metadata(&self.anchor.path).ok()?;
        if self.offset < start
            || self.offset >= end
            || !metadata.is_file()
            || file_identity(&metadata) != Some((self.anchor.device, self.anchor.inode))
        {
            return None;
        }
        self.file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::new();
        (&mut self.file)
            .take(end - start)
            .read_to_end(&mut tail)
            .ok()?;
        let watermark = usize::try_from(self.offset - start).ok()?;
        if !tail.get(..watermark)?.ends_with(&self.prefix) || !tail.ends_with(b"\n") {
            return None;
        }
        let mut offset = start;
        let mut rows = Vec::new();
        for line in tail.split_inclusive(|byte| *byte == b'\n') {
            let at = offset;
            offset += line.len() as u64;
            if at == start && start != 0 {
                continue;
            }
            if let Ok(value) = serde_json::from_slice(line) {
                rows.push(Row { offset: at, value });
            } else if at >= self.offset {
                return None;
            }
        }
        let main = |row: &&Row| {
            field(&row.value, "sessionId") == Some(self.anchor.session.as_str())
                && row.value.get("isSidechain").and_then(Value::as_bool) == Some(false)
        };
        let cancel = rows.iter().rfind(main)?;
        if cancel.offset < self.offset
            || field(&cancel.value, "version") != Some(VERSION)
            || field(&cancel.value, "type") != Some("user")
            || field(&cancel.value, "promptId") != Some(self.anchor.prompt.as_str())
            || field(&cancel.value, "interruptedMessageId").is_none()
            || field(&cancel.value, "promptSource").is_some()
            || field(&cancel.value, "sourceToolAssistantUUID").is_some()
            || field(&cancel.value, "uuid").is_none()
        {
            return None;
        }
        let mut parent = field(&cancel.value, "parentUuid")?;
        let mut tool_result = None;
        let mut progressed = false;
        for row in rows.iter().filter(main).rev().skip(1) {
            let value = &row.value;
            // Any newer typed turn in the main transcript makes the cancel ambiguous.
            if field(value, "type") == Some("user") && field(value, "promptSource").is_some() {
                return (field(value, "uuid") == Some(parent)
                    && field(value, "version") == Some(VERSION)
                    && field(value, "promptId") == Some(self.anchor.prompt.as_str())
                    && field(value, "promptSource") == Some("typed")
                    && field(value, "turnOrigin") == Some("human")
                    && row.offset < self.offset
                    && progressed)
                    .then_some(ToolProgressCancellation);
            }
            if field(value, "uuid") != Some(parent) {
                continue;
            }
            if field(value, "version") != Some(VERSION)
                || field(value, "promptId").is_some_and(|id| id != self.anchor.prompt)
                || field(value, "interruptedMessageId").is_some()
            {
                return None;
            }
            if row.offset < self.offset {
                if field(value, "type") == Some("user") {
                    tool_result = field(value, "sourceToolAssistantUUID").and_then(|uuid| {
                        Some((uuid, block_id(value, "tool_result", "tool_use_id")?))
                    });
                } else if field(value, "type") == Some("assistant") {
                    progressed |= tool_result.take().is_some_and(|(uuid, id)| {
                        uuid == parent && block_id(value, "tool_use", "id") == Some(id)
                    });
                }
            }
            parent = field(value, "parentUuid")?;
        }
        None
    }
}

fn field<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key)?.as_str().filter(|text| !text.is_empty())
}

fn block_id<'a>(row: &'a Value, kind: &str, key: &str) -> Option<&'a str> {
    row.get("message")?
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|block| {
            (field(block, "type") == Some(kind))
                .then(|| field(block, key))
                .flatten()
        })
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[cfg(unix)]
fn open_transcript(path: &Path) -> Option<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()
}

#[cfg(not(unix))]
fn open_transcript(_path: &Path) -> Option<File> {
    None
}
