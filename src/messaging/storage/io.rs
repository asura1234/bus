//! Durable private files and OS-released exclusive ownership, outside all UI paths.
pub(crate) use crate::platform::fs::guarded_atomic_write as atomic_write;
// Preserve existing messaging call sites until the S12 import rewrite.
pub(crate) use crate::platform::fs::private_dir;
use crate::platform::fs::FileLock;
pub(crate) use crate::utils::time::{now_ms, now_ns};
use std::{fs::File, io, path::Path};

/// Creates a private lock file when needed, rejects a symlink, and opens it
/// read/write. Every lock entry point shares this so they enforce one contract.
fn open_lock(path: &Path) -> io::Result<File> {
    if !path.exists() {
        match crate::platform::create_private_state_file(path) {
            Ok(file) => file.sync_all()?,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("Bus lock is a symlink"));
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
}

pub(crate) fn lock(path: &Path) -> io::Result<FileLock> {
    let file = open_lock(path)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => io::Error::from(io::ErrorKind::WouldBlock),
        std::fs::TryLockError::Error(error) => error,
    })?;
    Ok(FileLock::from_locked(file))
}

pub(crate) fn append_lock(path: &Path) -> io::Result<FileLock> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match lock(path) {
            Ok(file) => return Ok(file),
            Err(error) if std::time::Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn private_dir_creates_missing_roots_owner_only_even_when_racing() {
        use std::os::unix::fs::PermissionsExt;
        let base = crate::utils::test_temp::unique_temp_path("bus-private-dir");
        let root = base.join("data");
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| super::private_dir(&root).unwrap());
            }
        });
        for dir in [&base, &root] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", dir.display());
        }
        std::fs::write(base.join("file"), b"").unwrap();
        assert!(super::private_dir(&base.join("file")).is_err());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn exclusive_lock_is_released_on_drop_and_atomic_files_roundtrip() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-lock-test");
        super::private_dir(&dir).unwrap();
        let lease = super::lock(&dir.join("lock")).unwrap();
        assert!(super::lock(&dir.join("lock")).is_err());
        super::atomic_write(&dir.join("state"), b"first").unwrap();
        super::atomic_write(&dir.join("state"), b"second").unwrap();
        assert_eq!(std::fs::read(dir.join("state")).unwrap(), b"second");
        drop(lease);
        assert!(super::lock(&dir.join("lock")).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn storage_write_and_lock_reject_real_and_dangling_symlinks() {
        let dir = crate::utils::test_temp::unique_temp_path("bus-symlink-policy");
        super::private_dir(&dir).unwrap();
        let target = dir.join("real");
        std::fs::write(&target, b"original").unwrap();
        for (name, target) in [
            ("real-link", target),
            ("dangling-link", dir.join("missing")),
        ] {
            let link = dir.join(name);
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(super::atomic_write(&link, b"replacement").is_err());
            assert!(super::lock(&link).is_err());
            assert!(std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink());
        }
        assert_eq!(std::fs::read(dir.join("real")).unwrap(), b"original");
        assert!(!dir.join("missing").exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
