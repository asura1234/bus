use super::*;

fn temp_dir(label: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bus-fs-{label}-{}-{unique}", std::process::id()));
    create_private_directory(&dir).unwrap();
    dir
}

#[test]
fn failed_write_removes_partial_temp_and_preserves_destination() {
    let dir = temp_dir("failed-write");
    let path = dir.join("state.json");
    atomic_write(&path, b"original").unwrap();
    let error = atomic_write_with(&path, |file| {
        file.write_all(b"partial")?;
        Err(io::Error::other("injected write failure"))
    })
    .unwrap_err();
    assert_eq!(error.source.to_string(), "injected write failure");
    assert!(!error.path.exists());
    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_replace_removes_temp_and_preserves_destination() {
    let dir = temp_dir("failed-replace");
    let path = dir.join("directory");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep"), b"original").unwrap();
    let error = atomic_write_detailed(&path, b"replacement").unwrap_err();
    assert_eq!(error.path, path);
    assert_eq!(std::fs::read(path.join("keep")).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_writes_use_distinct_temps_and_leave_complete_bytes() {
    let dir = temp_dir("concurrent");
    let path = dir.join("state.json");
    std::thread::scope(|scope| {
        for index in 0..8_u8 {
            let path = &path;
            scope.spawn(move || atomic_write(path, &[index; 1024]).unwrap());
        }
    });
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 1024);
    assert!(bytes.iter().all(|byte| *byte == bytes[0]));
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn exclusive_file_lock_is_released_on_drop() {
    let dir = temp_dir("lock");
    let path = dir.join("lock");
    create_private_state_file(&path).unwrap();
    let lease = lock_file(&path).unwrap();
    assert!(lock_file(&path).is_err());
    drop(lease);
    assert!(lock_file(&path).is_ok());
    assert!(lock_file(&dir.join("missing")).is_err());
    assert!(!dir.join("missing").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn private_files_and_directories_are_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("permissions");
    let path = dir.join("state.json");
    atomic_write(&path, b"first").unwrap();
    atomic_write(&path, b"second").unwrap();
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(dir).unwrap();
}
