use super::*;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::Duration;

#[test]
fn prepare_socket_path_removes_stale_socket() {
    let dir = PathBuf::from(format!(
        "/tmp/hs-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::create_dir_all(&dir);
    let socket_path = dir.join("stale.sock");

    {
        let _listener = UnixListener::bind(&socket_path).expect("bind stale socket");
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(&socket_path).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let result = prepare_socket_path(&socket_path);
    assert!(result.is_ok(), "should remove stale socket: {result:?}");
    assert!(!socket_path.exists());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn prepare_socket_path_rejects_live_socket() {
    let dir = PathBuf::from(format!(
        "/tmp/hl-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::create_dir_all(&dir);
    let socket_path = dir.join("live.sock");

    let _listener = UnixListener::bind(&socket_path).expect("bind");

    let result = prepare_socket_path(&socket_path);
    assert!(result.is_err());
    let error = result.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    assert_eq!(
        error.to_string(),
        format!(
            "herdr server is already running (socket busy at {})",
            socket_path.display()
        )
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restrict_socket_permissions_keeps_owner_read_write_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!(
        "herdr-private-socket-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&dir).unwrap();
    let socket_path = dir.join("private.sock");
    let _listener = UnixListener::bind(&socket_path).unwrap();
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666)).unwrap();

    restrict_socket_permissions(&socket_path).unwrap();

    assert_eq!(
        fs::metadata(&socket_path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    drop(_listener);
    fs::remove_dir_all(dir).unwrap();
}
