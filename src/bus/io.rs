//! Durable private files and OS-released exclusive ownership, outside all UI paths.
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};

pub(crate) fn private_dir(path: &Path) -> io::Result<()> {
    if path.exists() {
        let meta = std::fs::symlink_metadata(path)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(io::Error::other("Bus directory is not a real directory"));
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        private_dir(parent)?;
    }
    crate::platform::create_remote_private_dir(path)
}

pub(crate) fn lock(path: &Path) -> io::Result<File> {
    if !path.exists() {
        match crate::platform::create_private_state_file(path) {
            Ok(file) => file.sync_all()?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("Bus lock is a symlink"));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}

pub(crate) fn append_lock(path: &Path) -> io::Result<File> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match lock(path) {
            Ok(file) => return Ok(file),
            Err(error) if std::time::Instant::now() >= deadline => return Err(error),
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    if path.exists() && std::fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(io::Error::other("Bus refuses to replace a symlink"));
    }
    let temp = parent.join(format!(".bus-{}-{}.tmp", std::process::id(), now_ns()));
    let mut file = crate::platform::create_private_state_file(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    crate::platform::replace_file(&temp, path)?;
    crate::platform::sync_parent_directory(parent)
}

pub(crate) fn now_ns() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

pub(crate) fn now_ms() -> u64 {
    (now_ns() / 1_000_000) as u64
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    #[test]
    fn exclusive_lock_is_released_on_drop_and_atomic_files_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bus-lock-test-{}", super::now_ns()));
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
}
