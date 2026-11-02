use super::*;
use std::fs;
use std::os::unix::net::UnixListener;
use std::time::Duration;

#[test]
fn prepare_socket_path_removes_stale_socket() {
    let dir = crate::utils::test_temp::unique_temp_path("hs");
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
    let dir = crate::utils::test_temp::unique_temp_path("hl");
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
            "bus server is already running (socket busy at {})",
            socket_path.display()
        )
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn restrict_socket_permissions_keeps_owner_read_write_only() {
    use std::os::unix::fs::PermissionsExt;
    // macOS TMPDIR can exceed sockaddr_un's path limit before the socket name.
    let dir = crate::utils::test_temp::unique_temp_path("hperm");
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

#[test]
fn prepare_socket_path_preserves_an_existing_regular_file() {
    let dir = crate::utils::test_temp::unique_temp_path("socket-file");
    fs::create_dir(&dir).unwrap();
    let socket_path = dir.join("endpoint.sock");
    let contents = b"existing user data";
    fs::write(&socket_path, contents).unwrap();

    let result = prepare_socket_path(&socket_path);
    let remaining_contents = fs::read(&socket_path).ok();
    fs::remove_dir_all(&dir).unwrap();

    assert_eq!(
        remaining_contents.as_deref(),
        Some(contents.as_slice()),
        "preparing a socket path must preserve an existing regular file"
    );
    assert!(result.is_err(), "a regular file is not a stale socket");
}
