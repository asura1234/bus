//! Share native trust decisions, retaining old private stores during migration.
use std::path::Path;

#[cfg(unix)]
pub(super) fn share(source: &Path, home: &Path) -> Result<bool, String> {
    use std::os::unix::fs::{symlink, DirBuilderExt};
    if !source.is_absolute() || super::bus_managed_source(source) {
        return Err("Saved Kimi trust source is invalid".into());
    }
    let target = source.join("workspace-trust");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(&target)
        .map_err(|e| e.to_string())?;
    // Source directory links are supported, just as for credentials and oauth.
    if !target.is_dir() {
        return Err("Kimi shared trust directory is unavailable".into());
    }
    let link = home.join("workspace-trust");
    let backup = home.join("workspace-trust.before-shared");
    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            if std::fs::read_link(&link).map_err(|e| e.to_string())? != target {
                return Err("Saved Kimi trust link changed; relaunch required".into());
            }
            return Ok(false);
        }
        Ok(_) => {
            let expected = super::home_record(&link, source)?;
            if std::fs::symlink_metadata(&backup).is_ok() {
                return Err("Kimi private trust backup already exists".into());
            }
            std::fs::rename(&link, &backup).map_err(|e| e.to_string())?;
            let mut moved = super::home_record(&backup, source)?;
            moved.path = link.clone();
            if moved != expected {
                return Err("Kimi private trust directory changed".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    // An interrupted migration resumes from the retained, identity-checked store.
    match std::fs::symlink_metadata(&backup) {
        Ok(_) => migrate(&backup, &target, source)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    symlink(&target, &link).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(unix)]
fn migrate(backup: &Path, target: &Path, source: &Path) -> Result<(), String> {
    use std::{
        ffi::CString,
        fs::{File, OpenOptions},
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
    };
    let expected = super::home_record(backup, source)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(backup)
        .map_err(|e| e.to_string())?;
    let meta = directory.metadata().map_err(|e| e.to_string())?;
    if (meta.dev(), meta.ino()) != (expected.device, expected.inode) {
        return Err("Kimi private trust directory changed".into());
    }
    for entry in std::fs::read_dir(backup).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            continue;
        }
        let name = CString::new(entry.file_name().as_bytes()).map_err(|e| e.to_string())?;
        // SAFETY: pinned directory descriptor and one NUL-terminated entry name.
        // Refuse leaf links and never block on a replaced FIFO.
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        // SAFETY: successful openat returns a new owned descriptor.
        let file = unsafe { File::from_raw_fd(fd) };
        let meta = file.metadata().map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > 4096 {
            continue;
        }
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if bytes.len() > 4096
            || !record
                .get("root")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|root| Path::new(root).is_absolute())
            || record
                .get("trustedAt")
                .and_then(serde_json::Value::as_u64)
                .is_none()
        {
            continue;
        }
        // Publish complete records without replacing any current shared decision.
        // The old store remains intact, including unrecognized entries.
        let temp = tempfile_path(target);
        super::hook_json::atomic_write(&temp, &bytes).map_err(|e| e.to_string())?;
        let result = std::fs::hard_link(&temp, target.join(entry.file_name()));
        let cleanup = std::fs::remove_file(&temp);
        match result {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
        cleanup.map_err(|e| e.to_string())?;
    }
    if super::home_record(backup, source)? != expected {
        return Err("Kimi private trust directory changed".into());
    }
    Ok(())
}

#[cfg(unix)]
fn tempfile_path(target: &Path) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    target.join(format!(
        ".bus-trust-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(not(unix))]
pub(super) fn share(_source: &Path, _home: &Path) -> Result<bool, String> {
    Err("Kimi shared trust requires shared authentication support".into())
}
