use crate::platform::ipc::restrict_socket_permissions;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[cfg(unix)]
#[test]
fn private_control_refuses_a_shared_directory_without_changing_permissions() {
    let path = crate::utils::test_temp::unique_temp_path("bdp");
    fs::create_dir(&path).unwrap();
    restrict_socket_permissions(&path, 0o755).unwrap();
    let (tx, _rx) = std::sync::mpsc::sync_channel(0);
    let rejected = crate::messaging::control::start(true, &path, tx).is_err();
    let unchanged = fs::metadata(&path).unwrap().permissions().mode() & 0o777 == 0o755;
    fs::remove_dir_all(&path).unwrap();
    assert!(
        rejected,
        "shared parent must not host a private control listener"
    );
    assert!(
        unchanged,
        "startup must not chmod an existing user directory"
    );
}

#[cfg(unix)]
#[test]
fn private_control_client_refuses_an_endpoint_in_a_shared_directory() {
    let path = crate::utils::test_temp::unique_temp_path("bdc");
    fs::create_dir(&path).unwrap();
    restrict_socket_permissions(&path, 0o700).unwrap();
    let (tx, _rx) = std::sync::mpsc::sync_channel(0);
    let server = crate::messaging::control::start(true, &path, tx)
        .unwrap()
        .unwrap();
    restrict_socket_permissions(&path, 0o755).unwrap();
    let result = crate::messaging::control::request(
        &path,
        &crate::messaging::control::Request {
            id: "private".into(),
            method: "state".into(),
            params: serde_json::Value::Null,
        },
    );
    drop(server);
    fs::remove_dir_all(&path).unwrap();
    assert!(result.is_err(), "client must refuse a replaceable endpoint");
}
