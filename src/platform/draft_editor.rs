//! Exact, durable draft exchange through the provider's external-editor file.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

const DIRECTORY: &str = "BUS_DRAFT_EDITOR_DIR";
const FALLBACK: &str = "BUS_DRAFT_EDITOR_FALLBACK";
const REQUEST: &str = "request.json";
const ACK: &str = "ack.json";
pub(crate) const MAX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Operation {
    Idle,
    Capture,
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
}

#[derive(Deserialize, Serialize)]
struct Acknowledgement {
    request: Request,
    error: Option<String>,
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
    let exe = std::env::current_exe()?;
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
        private_dir(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let _lock = lock(&dir)?;
        let orphaned = fs::read_dir(&dir)?.any(|entry| {
            entry.is_ok_and(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("draft-") && name.ends_with(".txt")
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
            if name.starts_with("draft-") && name.ends_with(".txt") {
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
    pub(crate) fn request_capture(&self) -> io::Result<()> {
        self.request(Operation::Capture)
    }
    pub(crate) fn request_restore(&self) -> io::Result<()> {
        self.request(Operation::Restore)
    }
    pub(crate) fn request_verify(&self) -> io::Result<()> {
        self.request(Operation::Verify)
    }

    fn request(&self, operation: Operation) -> io::Result<()> {
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

    pub(crate) fn captured_text(&self) -> io::Result<String> {
        fs::read_to_string(&self.recovery)
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
    if request.token.is_empty()
        || request.token.len() > 128
        || !request
            .token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    {
        return Err(io::Error::other("Invalid draft exchange token"));
    }
    Ok(Some(request))
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
        return manual_editor(file, fallback);
    };
    if !dir.exists() {
        return manual_editor(file, fallback);
    }
    let guard = lock(dir)?;
    let Some(request) = read_request(dir)? else {
        drop(guard);
        return manual_editor(file, fallback);
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

fn exchange_file(dir: &Path, file: &Path, request: &Request) -> io::Result<()> {
    if fs::metadata(file)?.len() > MAX_BYTES {
        return Err(io::Error::other("Draft exceeds 2 MiB"));
    }
    let bytes = fs::read(file)?;
    let saved = recovery(dir, &request.token);
    let replacement = match request.operation {
        Operation::Capture => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if ["[Image #", "[Pasted text #", "[Pasted Content "]
                .iter()
                .any(|placeholder| text.contains(placeholder))
            {
                return Err(io::Error::other("Opaque draft content was preserved"));
            }
            if saved.exists() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Draft recovery already exists",
                ));
            }
            crate::platform::atomic_write(&saved, &bytes)?;
            Vec::new()
        }
        Operation::Restore | Operation::Verify => {
            let mut original = fs::read(saved)?;
            let expected = if request.operation == Operation::Restore {
                &[][..]
            } else {
                original.as_slice()
            };
            if bytes != expected {
                return Err(io::Error::other(
                    "Composer changed; recovery draft was not written",
                ));
            }
            original.extend_from_slice(marker(&request.token).as_bytes());
            original
        }
        Operation::Idle | Operation::Cancelled => {
            return Err(io::Error::other("No draft operation requested"))
        }
    };
    // Cancellation shares our lock. Recheck expiry and the actual file before
    // each mutation, including after the recovery copy has reached disk.
    check_expiry(request)?;
    if fs::read(file)? != bytes {
        return Err(io::Error::other("Editor file changed; draft was preserved"));
    }
    crate::platform::atomic_write(file, &replacement)
}

#[cfg(not(windows))]
fn manual_editor(file: &Path, editor: &str) -> io::Result<()> {
    if wrapper(editor) {
        return Err(io::Error::other("Recursive Bus editor configuration"));
    }
    let status = Command::new("/bin/sh")
        .args(["-c", &format!("exec {editor} \"$@\""), "bus-draft-editor"])
        .arg(file)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("The developer's editor failed"))
    }
}

#[cfg(windows)]
fn manual_editor(file: &Path, editor: &str) -> io::Result<()> {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    if wrapper(editor) {
        return Err(io::Error::other("Recursive Bus editor configuration"));
    }
    let command: Vec<_> = editor.encode_utf16().chain(Some(0)).collect();
    let mut count = 0;
    let argv = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    if argv.is_null() || count <= 0 {
        return Err(io::Error::other("Invalid editor command"));
    }
    let mut words = Vec::new();
    for &word in unsafe { std::slice::from_raw_parts(argv, count as usize) } {
        let mut length = 0;
        unsafe {
            while *word.add(length) != 0 {
                length += 1;
            }
        }
        words.push(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(word, length)
        }));
    }
    unsafe {
        LocalFree(argv.cast());
    }
    let (program, args) = words
        .split_first()
        .ok_or_else(|| io::Error::other("Empty editor command"))?;
    if Command::new(program)
        .args(args)
        .arg(file)
        .status()?
        .success()
    {
        Ok(())
    } else {
        Err(io::Error::other("The developer's editor failed"))
    }
}

#[cfg(test)]
#[path = "tests/draft_editor_test.rs"]
mod tests;
