//! Durable private files and OS-released exclusive ownership, outside all UI paths.
// Preserve existing messaging call sites until the S12 import rewrite.
pub(crate) use crate::platform::fs::private_dir;
pub(crate) use crate::utils::time::{now_ms, now_ns};
use std::{fs::File, io, path::Path};

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
    crate::platform::fs::lock_file(path)
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

/// Blocks until this process holds the exclusive lock. `append_lock` gives up
/// after 3s, which is too short for a critical section that copies the skill tree.
pub(crate) fn lock_wait(path: &Path) -> io::Result<File> {
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
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    loop {
        match file.lock() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(io::Error::other("Bus refuses to replace a symlink"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    crate::platform::fs::atomic_write(path, bytes)
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
            assert!(super::lock_wait(&link).is_err());
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

    #[cfg(unix)]
    #[test]
    fn subprocess_lock_holder() {
        let Some(path) = std::env::var_os("BUS_TEST_LOCK_HOLD") else {
            return;
        };
        let path = std::path::PathBuf::from(path);
        let lease = super::lock(&path).unwrap();
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".held");
        std::fs::write(path.with_file_name(name), b"1").unwrap();
        let mut byte = [0];
        let mut input = std::io::stdin();
        let _ = std::io::Read::read(&mut input, &mut byte);
        drop(lease);
    }

    #[cfg(unix)]
    #[test]
    fn lock_wait_blocks_past_the_append_timeout() {
        use std::process::{Command, Stdio};
        let dir = crate::utils::test_temp::unique_temp_path("bus-lock-wait");
        super::private_dir(&dir).unwrap();
        let path = dir.join("lock");
        let marker = dir.join("lock.held");
        let stderr_path = dir.join("holder-stderr");
        let stderr = std::fs::File::create(&stderr_path).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "messaging::storage::io::tests::subprocess_lock_holder",
                "--test-threads",
                "1",
            ])
            .env("BUS_TEST_LOCK_HOLD", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut locked = false;
        while std::time::Instant::now() < deadline {
            if marker.is_file() {
                locked = true;
                break;
            }
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if !locked {
            let _ = child.kill();
            let _ = child.wait();
            let detail = std::fs::read_to_string(&stderr_path).unwrap_or_default();
            let _ = std::fs::remove_dir_all(&dir);
            panic!("child did not acquire lock: {detail}");
        }
        let stdin = child.stdin.take().unwrap();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(4));
            drop(stdin);
        });
        let started = std::time::Instant::now();
        let lease = super::lock_wait(&path).unwrap();
        let elapsed = started.elapsed();
        drop(lease);
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            elapsed >= std::time::Duration::from_secs(3),
            "lock_wait returned in {elapsed:?}, inside the 3s append timeout"
        );
    }
}
