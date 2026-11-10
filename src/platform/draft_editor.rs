//! Exact, durable draft exchange through the provider's external-editor file.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

mod manual;

const DIRECTORY: &str = "BUS_DRAFT_EDITOR_DIR";
const FALLBACK: &str = "BUS_DRAFT_EDITOR_FALLBACK";
const REQUEST: &str = "request.json";
const ACK: &str = "ack.json";
const PRESERVED: &str = "preserved.json";
pub(crate) const MAX_BYTES: u64 = 2 * 1024 * 1024;
// JSON may encode a single input byte as six bytes (for example, \u0000).
const MAX_RECORD_BYTES: u64 = MAX_BYTES * 6 + 4096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Operation {
    Idle,
    Swap,
    Restore,
    Verify,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Request {
    token: String,
    sequence: u64,
    expires_ms: u64,
    operation: Operation,
    /// Input installed by Swap, or an attempted input allowed during Restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verify_text: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Acknowledgement {
    request: Request,
    error: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct PreservedDraft {
    token: String,
    attempt: String,
}

pub(crate) struct Exchange {
    dir: PathBuf,
    recovery: PathBuf,
    marker: String,
    request: Mutex<Request>,
}

pub(crate) fn directory(pane: &str) -> PathBuf {
    let name: String = pane
        .chars()
        .take(96)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    crate::utils::paths::data_dir()
        .join("private/drafts")
        .join(format!(
            "{name}-{}",
            &crate::utils::time::digest(pane.as_bytes())[..16]
        ))
}

/// Preserve the developer's editor, including when a pane inherits our wrapper.
pub(crate) fn configure(
    cmd: &mut portable_pty::CommandBuilder,
    pane: &str,
    extra: &[(String, String)],
) -> io::Result<()> {
    let value = |key: &str| {
        extra
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .or_else(|| {
                cmd.get_env(key)
                    .map(|value| value.to_string_lossy().into_owned())
            })
    };
    let inherited = value(FALLBACK).filter(|value| !value.trim().is_empty() && !wrapper(value));
    let fallback = [value("VISUAL"), value("EDITOR")]
        .into_iter()
        .flatten()
        .find_map(|value| {
            if value.trim().is_empty() {
                None
            } else if wrapper(&value) {
                inherited.clone()
            } else {
                Some(value)
            }
        })
        .or(inherited)
        .unwrap_or_else(|| if cfg!(windows) { "notepad.exe" } else { "vi" }.into());
    let exe = crate::platform::fs::running_executable()?;
    #[cfg(not(windows))]
    let executable = format!("'{}'", exe.to_string_lossy().replace('\'', "'\\''"));
    #[cfg(windows)]
    let executable = format!("\"{}\"", exe.display());
    let editor = format!("{executable} --bus-callback draft-editor");
    cmd.env(DIRECTORY, directory(pane));
    cmd.env(FALLBACK, fallback);
    cmd.env("VISUAL", &editor);
    cmd.env("EDITOR", editor);
    Ok(())
}

fn wrapper(value: &str) -> bool {
    value.contains("--bus-callback") && value.contains("draft-editor")
}

impl Exchange {
    pub(crate) fn begin(dir: PathBuf, timeout: Duration) -> io::Result<Self> {
        Self::begin_inner(dir, timeout)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn begin_preserving(dir: PathBuf, timeout: Duration) -> io::Result<Self> {
        let mut exchange = Self::begin(dir, timeout)?;
        exchange.recovery = preserved_recovery(
            &exchange.dir,
            &exchange
                .request
                .lock()
                .map_err(|_| io::Error::other("Draft exchange lock poisoned"))?
                .token,
        );
        Ok(exchange)
    }

    pub(crate) fn has_preserved_draft(dir: &Path) -> bool {
        dir.join(PRESERVED).exists()
    }

    /// Resume a durable developer backup, never a Bus-paste withdrawal proof.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn begin_restoration(dir: PathBuf, timeout: Duration) -> io::Result<Self> {
        private_dir(&dir)?;
        let _lock = lock(&dir)?;
        let saved: PreservedDraft =
            serde_json::from_slice(&read_regular(&dir.join(PRESERVED), MAX_RECORD_BYTES)?)?;
        if !valid_token(&saved.token) || saved.attempt.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Invalid preserved draft record"));
        }
        let recovery = preserved_recovery(&dir, &saved.token);
        read_regular(&recovery, MAX_BYTES)?;
        let previous = read_request(&dir)?;
        if previous.as_ref().is_some_and(|request| {
            request.operation != Operation::Cancelled
                && request.expires_ms > crate::utils::time::now_ms()
                && !has_ack(&dir, request)
        }) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Editor handoff is still active",
            ));
        }
        let request = Request {
            token: saved.token.clone(),
            sequence: previous.map_or(0, |request| request.sequence.saturating_add(1)),
            expires_ms: crate::utils::time::now_ms()
                .saturating_add(timeout.as_millis().min(u128::from(u64::MAX)) as u64),
            operation: Operation::Restore,
            verify_text: Some(saved.attempt),
        };
        write_json(&dir.join(REQUEST), &request)?;
        Ok(Self {
            dir,
            recovery,
            marker: marker(&saved.token),
            request: Mutex::new(request),
        })
    }

    fn begin_inner(dir: PathBuf, timeout: Duration) -> io::Result<Self> {
        private_dir(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let _lock = lock(&dir)?;
        if Self::has_preserved_draft(&dir) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "A developer draft needs restoration",
            ));
        }
        let orphaned = fs::read_dir(&dir)?.any(|entry| {
            entry.is_ok_and(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                (name.starts_with("draft-") || name.starts_with("preserved-"))
                    && name.ends_with(".txt")
            })
        });
        if read_request(&dir)?.is_some_and(|request| {
            (request.operation != Operation::Cancelled || !orphaned)
                && (request.expires_ms > crate::utils::time::now_ms()
                    || request.operation != Operation::Idle && !has_ack(&dir, &request))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "A draft exchange already owns this pane",
            ));
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (name.starts_with("draft-") || name.starts_with("preserved-"))
                && name.ends_with(".txt")
            {
                let recovered = entry.path().with_extension("recovered");
                if recovered.exists() {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!(
                            "A previous draft still needs recovery: {}",
                            entry.path().display()
                        ),
                    ));
                }
                fs::rename(entry.path(), &recovered)?;
                tracing::warn!(event = "bus.draft.recovery_quarantined", path = %recovered.display(), "Preserved an orphaned draft without blocking later input");
            }
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let token = format!(
            "{:x}-{:x}-{:x}",
            std::process::id(),
            crate::utils::time::now_ns(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let request = Request {
            token: token.clone(),
            sequence: 0,
            expires_ms: crate::utils::time::now_ms()
                .saturating_add(timeout.as_millis().min(u128::from(u64::MAX)) as u64),
            operation: Operation::Idle,
            verify_text: None,
        };
        write_json(&dir.join(REQUEST), &request)?;
        Ok(Self {
            recovery: recovery(&dir, &token),
            marker: marker(&token),
            dir,
            request: Mutex::new(request),
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.dir
    }
    pub(crate) fn recovery(&self) -> &Path {
        &self.recovery
    }
    pub(crate) fn marker(&self) -> &str {
        &self.marker
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn request_swap(&self, text: &str) -> io::Result<()> {
        if text.len() as u64 > MAX_BYTES {
            return Err(io::Error::other("Prompt exceeds 2 MiB"));
        }
        self.request_with_text(Operation::Swap, Some(text.to_owned()))
    }
    #[allow(dead_code)]
    pub(crate) fn request_restore_after_attempt(&self, text: &str) -> io::Result<()> {
        self.request_with_text(Operation::Restore, Some(text.to_owned()))
    }
    pub(crate) fn request_restore(&self) -> io::Result<()> {
        self.request(Operation::Restore)
    }
    pub(crate) fn request_verify(&self) -> io::Result<()> {
        self.request(Operation::Verify)
    }

    fn request(&self, operation: Operation) -> io::Result<()> {
        self.request_with_text(operation, None)
    }

    fn request_with_text(
        &self,
        operation: Operation,
        verify_text: Option<String>,
    ) -> io::Result<()> {
        let _lock = lock(&self.dir)?;
        let mut current = self
            .request
            .lock()
            .map_err(|_| io::Error::other("Draft exchange lock poisoned"))?;
        let saved = read_request(&self.dir)?
            .ok_or_else(|| io::Error::other("Draft exchange was cancelled"))?;
        if saved.token != current.token {
            return Err(io::Error::other("Draft exchange ownership changed"));
        }
        check_expiry(&saved)?;
        let next = Request {
            sequence: current.sequence + 1,
            operation,
            verify_text,
            ..current.clone()
        };
        write_json(&self.dir.join(REQUEST), &next)?;
        *current = next;
        Ok(())
    }

    pub(crate) fn acknowledged(&self) -> io::Result<bool> {
        let request = self
            .request
            .lock()
            .map_err(|_| io::Error::other("Draft exchange lock poisoned"))?;
        acknowledgement(&self.dir, &request)
    }

    pub(crate) fn save_recovery(&self, text: &str) -> io::Result<()> {
        let _lock = lock(&self.dir)?;
        let request = self
            .request
            .lock()
            .map_err(|_| io::Error::other("Draft exchange lock poisoned"))?;
        check_expiry(&request)?;
        if read_request(&self.dir)?.is_none_or(|saved| saved.token != request.token) {
            return Err(io::Error::other("Draft exchange ownership changed"));
        }
        if self.recovery.exists() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Draft recovery already exists",
            ));
        }
        crate::platform::atomic_write(&self.recovery, text.as_bytes())
    }

    /// The caller must prove restoration before releasing the recovery copy.
    pub(crate) fn remove_recovery(&self) -> io::Result<()> {
        let _lock = lock(&self.dir)?;
        if self
            .recovery
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("preserved-"))
        {
            let saved: PreservedDraft = serde_json::from_slice(&read_regular(
                &self.dir.join(PRESERVED),
                MAX_RECORD_BYTES,
            )?)?;
            if saved.token
                != self
                    .request
                    .lock()
                    .map_err(|_| io::Error::other("Draft exchange lock poisoned"))?
                    .token
            {
                return Err(io::Error::other("Preserved draft ownership changed"));
            }
            fs::remove_file(self.dir.join(PRESERVED))?;
        }
        fs::remove_file(&self.recovery)?;
        crate::platform::sync_parent_directory(&self.dir)
    }
}

impl Drop for Exchange {
    fn drop(&mut self) {
        let Ok(_lock) = lock(&self.dir) else { return };
        let Ok(current) = self.request.lock() else {
            return;
        };
        if read_request(&self.dir)
            .ok()
            .flatten()
            .is_some_and(|request| request.token == current.token)
        {
            let finished = current.operation == Operation::Idle || has_ack(&self.dir, &current);
            if finished {
                let _ = fs::remove_file(self.dir.join(REQUEST));
            } else {
                let _ = write_json(
                    &self.dir.join(REQUEST),
                    &Request {
                        operation: Operation::Cancelled,
                        ..current.clone()
                    },
                );
            }
        }
    }
}

fn recovery(dir: &Path, token: &str) -> PathBuf {
    dir.join(format!("draft-{token}.txt"))
}
fn preserved_recovery(dir: &Path, token: &str) -> PathBuf {
    dir.join(format!("preserved-{token}.txt"))
}

fn read_regular(path: &Path, maximum: u64) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > maximum {
        return Err(io::Error::other("Invalid draft recovery file"));
    }
    fs::read(path)
}
fn marker(token: &str) -> String {
    format!("__BUS_DRAFT_END_{token}__")
}

fn private_dir(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(io::Error::other(
            "Draft exchange path is not a private directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                private_dir(parent)?;
            }
            crate::platform::fs::create_private_directory(path)
        }
        Err(error) => Err(error),
    }
}

fn lock(dir: &Path) -> io::Result<File> {
    let path = dir.join("exchange.lock");
    let file = match crate::platform::create_private_state_file(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            OpenOptions::new().read(true).write(true).open(path)?
        }
        Err(error) => return Err(error),
    };
    file.lock()?;
    Ok(file)
}

fn read_request(dir: &Path) -> io::Result<Option<Request>> {
    let request: Request = match fs::read(dir.join(REQUEST)) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !valid_token(&request.token) {
        return Err(io::Error::other("Invalid draft exchange token"));
    }
    Ok(Some(request))
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    crate::platform::atomic_write(path, &serde_json::to_vec(value)?)
}

fn check_expiry(request: &Request) -> io::Result<()> {
    if crate::utils::time::now_ms() >= request.expires_ms {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Draft exchange expired",
        ))
    } else {
        Ok(())
    }
}

fn acknowledgement(dir: &Path, request: &Request) -> io::Result<bool> {
    let ack: Acknowledgement = match fs::read(dir.join(ACK)) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    if ack.request != *request {
        return Ok(false);
    }
    match ack.error {
        Some(error) => Err(io::Error::other(error)),
        None => Ok(true),
    }
}

fn has_ack(dir: &Path, request: &Request) -> bool {
    fs::read(dir.join(ACK))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Acknowledgement>(&bytes).ok())
        .is_some_and(|ack| ack.request == *request)
}

/// Runs before ordinary callback logging and never writes a hook JSON response.
pub(crate) fn run(args: &[String]) -> io::Result<()> {
    let file = args
        .get(3)
        .filter(|_| args.len() == 4)
        .ok_or_else(|| io::Error::other("Invalid draft-editor arguments"))?;
    let dir = std::env::var_os(DIRECTORY).map(PathBuf::from);
    let fallback = std::env::var(FALLBACK)
        .unwrap_or_else(|_| if cfg!(windows) { "notepad.exe" } else { "vi" }.into());
    run_at(dir.as_deref(), Path::new(file), &fallback)
}

fn run_at(dir: Option<&Path>, file: &Path, fallback: &str) -> io::Result<()> {
    let Some(dir) = dir else {
        return manual::manual_editor(file, fallback);
    };
    if !dir.exists() {
        return manual::manual_editor(file, fallback);
    }
    let guard = lock(dir)?;
    let Some(request) = read_request(dir)? else {
        drop(guard);
        return manual::manual_editor(file, fallback);
    };
    if request.operation == Operation::Cancelled {
        fs::remove_file(dir.join(REQUEST))?;
        return Err(io::Error::other(
            "Draft exchange was cancelled; editor file was preserved",
        ));
    }
    if let Err(error) = check_expiry(&request) {
        fs::remove_file(dir.join(REQUEST))?;
        return Err(error);
    }
    if acknowledgement(dir, &request)? {
        return Ok(());
    }
    let result = exchange_file(dir, file, &request);
    write_json(
        &dir.join(ACK),
        &Acknowledgement {
            request,
            error: result.as_ref().err().map(ToString::to_string),
        },
    )?;
    result
}

#[cfg(test)]
pub(crate) fn test_run_at(directory: &Path, file: &Path) -> io::Result<()> {
    run_at(Some(directory), file, "false")
}

fn exchange_replacement(
    dir: &Path,
    request: &Request,
    bytes: &[u8],
    saved: &Path,
) -> io::Result<Vec<u8>> {
    match request.operation {
        Operation::Swap => {
            let text = std::str::from_utf8(bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if ["[Image #", "[Pasted text #", "[Pasted Content "]
                .iter()
                .any(|marker| text.contains(marker))
            {
                return Err(io::Error::other("Opaque draft content was preserved"));
            }
            let attempt = request
                .verify_text
                .as_ref()
                .ok_or_else(|| io::Error::other("Missing swap input"))?;
            if attempt.len() as u64 > MAX_BYTES || saved.exists() || dir.join(PRESERVED).exists() {
                return Err(io::Error::other("Draft swap is unavailable"));
            }
            crate::platform::atomic_write(saved, bytes)?;
            write_json(
                &dir.join(PRESERVED),
                &PreservedDraft {
                    token: request.token.clone(),
                    attempt: attempt.clone(),
                },
            )?;
            let mut replacement = attempt.as_bytes().to_vec();
            replacement.extend_from_slice(marker(&request.token).as_bytes());
            Ok(replacement)
        }
        Operation::Restore | Operation::Verify => {
            let mut original = read_regular(saved, MAX_BYTES)?;
            let matches = if request.operation == Operation::Restore {
                bytes.is_empty()
                    || request.verify_text.as_ref().is_some_and(|attempt| {
                        let marker = marker(&request.token);
                        bytes == attempt.as_bytes()
                            || bytes == format!("{attempt}{marker}").as_bytes()
                            || bytes == original
                            || bytes == [original.as_slice(), marker.as_bytes()].concat()
                    })
            } else {
                bytes == original
            };
            if !matches {
                return Err(io::Error::other(
                    "Composer changed; recovery draft was not written",
                ));
            }
            original.extend_from_slice(marker(&request.token).as_bytes());
            Ok(original)
        }
        Operation::Idle | Operation::Cancelled => {
            Err(io::Error::other("No draft operation requested"))
        }
    }
}

fn exchange_file(dir: &Path, file: &Path, request: &Request) -> io::Result<()> {
    let bytes = read_regular(file, MAX_BYTES + marker(&request.token).len() as u64)?;
    if request.operation == Operation::Swap && bytes.len() as u64 > MAX_BYTES {
        return Err(io::Error::other("Draft exceeds 2 MiB"));
    }
    let preserved = preserved_recovery(dir, &request.token);
    let saved = if request.operation == Operation::Swap || preserved.exists() {
        preserved
    } else {
        recovery(dir, &request.token)
    };
    let replacement = exchange_replacement(dir, request, &bytes, &saved)?;
    // Cancellation shares our lock. Recheck expiry and the actual file before
    // each mutation, including after the recovery copy has reached disk.
    check_expiry(request)?;
    if fs::read(file)? != bytes {
        return Err(io::Error::other("Editor file changed; draft was preserved"));
    }
    crate::platform::atomic_write(file, &replacement)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/draft_editor_test.rs"]
mod tests;
