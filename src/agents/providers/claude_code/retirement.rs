//! Transcript evidence for Claude Code 2.1.316's tool-progress cancel path.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const VERSION: &str = "2.1.316";
const MAX_TAIL: u64 = 2 * 1024 * 1024;
const MAX_NOVELTY_BYTES: u64 = 16 * 1024 * 1024;

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
        if offset > MAX_NOVELTY_BYTES {
            return None;
        }
        let mut prefix = Vec::new();
        (&mut file).take(offset).read_to_end(&mut prefix).ok()?;
        (prefix.len() as u64 == offset && prefix.ends_with(b"\n")).then(|| CancelWatermark {
            anchor: self,
            file,
            offset,
            prefix_digest: Sha256::digest(&prefix).into(),
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
    prefix_digest: [u8; 32],
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
        if end > MAX_NOVELTY_BYTES
            || self.offset < start
            || self.offset >= end
            || !metadata.is_file()
            || file_identity(&metadata) != Some((self.anchor.device, self.anchor.inode))
        {
            return None;
        }
        self.file.seek(SeekFrom::Start(0)).ok()?;
        let mut transcript = Vec::new();
        (&mut self.file)
            .take(end)
            .read_to_end(&mut transcript)
            .ok()?;
        let watermark = usize::try_from(self.offset).ok()?;
        if transcript.len() as u64 != end
            || Sha256::digest(transcript.get(..watermark)?)[..] != self.prefix_digest
            || !transcript.ends_with(b"\n")
        {
            return None;
        }
        let mut split = usize::try_from(start).ok()?;
        if split != 0 {
            split += transcript[split..].iter().position(|byte| *byte == b'\n')? + 1;
        }
        let (history, tail) = transcript.split_at(split);
        let mut offset = split as u64;
        let mut rows = Vec::new();
        for line in tail.split_inclusive(|byte| *byte == b'\n') {
            rows.push(Row {
                offset,
                value: serde_json::from_slice(line).ok()?,
            });
            offset += line.len() as u64;
        }
        let main = |row: &&Row| {
            field(&row.value, "sessionId") == Some(self.anchor.session.as_str())
                && row.value.get("isSidechain").and_then(Value::as_bool) == Some(false)
        };
        let cancel = rows.iter().rfind(main)?;
        let uuid = field(&cancel.value, "uuid")?;
        let message_id = field(&cancel.value, "interruptedMessageId")?;
        if cancel.offset < self.offset
            || field(&cancel.value, "version") != Some(VERSION)
            || field(&cancel.value, "type") != Some("user")
            || field(&cancel.value, "promptId") != Some(self.anchor.prompt.as_str())
            || field(&cancel.value, "promptSource").is_some()
            || field(&cancel.value, "sourceToolAssistantUUID").is_some()
            || uuid == message_id
            || !novel_cancellation_ids(history, &rows, cancel, uuid, message_id)
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
                    && progressed
                    && self.file.metadata().ok()?.len() == end)
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

/// Claude 2.1.316 fills interruptedMessageId from the in-flight API stream.
/// That stream may leave no assistant row with its message.id. Bind the
/// novel ID to the anchored parent chain of this fresh cancellation row.
fn novel_cancellation_ids(
    history: &[u8],
    rows: &[Row],
    cancel: &Row,
    uuid: &str,
    message_id: &str,
) -> bool {
    let conflicts = |row: &Value| {
        field(row, "uuid") == Some(uuid)
            || [
                "uuid",
                "parentUuid",
                "leafUuid",
                "id",
                "promptId",
                "sessionId",
                "session_id",
                "requestId",
                "turnId",
                "sourceToolAssistantUUID",
                "interruptedMessageId",
            ]
            .iter()
            .any(|key| field(row, key) == Some(message_id))
            || row.get("message").and_then(|message| field(message, "id")) == Some(message_id)
    };
    !rows
        .iter()
        .any(|row| row.offset != cancel.offset && conflicts(&row.value))
        && history
            .split_inclusive(|byte| *byte == b'\n')
            .all(|line| serde_json::from_slice::<Value>(line).is_ok_and(|row| !conflicts(&row)))
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
