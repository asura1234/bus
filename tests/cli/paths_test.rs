//! No-model executable checks. These never start provider processes.
use std::process::Command;

fn isolated_home(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "bus-shell-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn bus_paths_are_isolated_even_with_inherited_herdr_and_xdg_overrides() {
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["--paths"])
        .env("BUS_DATA_DIR", "/tmp/bus-path-contract")
        .env("HERDR_CONFIG_PATH", "/tmp/not-bus/config.toml")
        .env("XDG_CONFIG_HOME", "/tmp/not-bus/config")
        .env("XDG_STATE_HOME", "/tmp/not-bus/state")
        .output()
        .expect("run executable");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let paths: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(paths["config"], "/tmp/bus-path-contract/herdr-config");
    assert_eq!(paths["state"], "/tmp/bus-path-contract/herdr-state");
    assert_eq!(paths["xdg_config"], "/tmp/not-bus/config");
    assert_eq!(paths["xdg_state"], "/tmp/not-bus/state");
}

#[test]
fn bus_resume_reports_missing_exact_and_last_sessions_without_starting() {
    let home = isolated_home("resume-missing");

    let exact = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["resume", "0123456789abcdef"])
        .env_remove("BUS_DATA_DIR")
        .env("HOME", &home)
        .output()
        .expect("run exact resume");
    assert!(!exact.status.success());
    assert!(
        String::from_utf8_lossy(&exact.stderr)
            .contains("Bus session '0123456789abcdef' was not found"),
        "{}",
        String::from_utf8_lossy(&exact.stderr)
    );

    let last = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["resume", "--last"])
        .env_remove("BUS_DATA_DIR")
        .env("HOME", &home)
        .output()
        .expect("run last resume");
    assert!(!last.status.success());
    assert!(
        String::from_utf8_lossy(&last.stderr)
            .contains("No local Bus session has been recorded yet"),
        "{}",
        String::from_utf8_lossy(&last.stderr)
    );

    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn bus_help_documents_fresh_and_resumed_session_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["--help"])
        .output()
        .expect("run help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("A plain `bus` launch always creates a new local session."));
    assert!(help.contains("bus [--dev] resume <session-id>"));
    assert!(help.contains("bus [--dev] resume --last"));
}

#[test]
fn bus_sessions_is_read_only_and_succeeds_when_no_sessions_exist() {
    let home = isolated_home("sessions-empty");

    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .args(["sessions"])
        .env_remove("BUS_DATA_DIR")
        .env("HOME", &home)
        .output()
        .expect("list sessions");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(!home.join(".local/share/bus").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn bus_stop_without_recorded_session_emits_false_without_creating_state() {
    let home = isolated_home("stop-empty");
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .arg("stop")
        .env_remove("BUS_DATA_DIR")
        .env_remove("BUS_SESSION_ID")
        .env("HOME", &home)
        .output()
        .expect("stop without a session");
    let created_state = home.join(".local/share/bus").exists();
    std::fs::remove_dir_all(home).unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response, serde_json::json!({"stopped": false}));
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
    assert!(
        !created_state,
        "a no-op stop must not create a session registry"
    );
}

#[test]
fn bus_stop_preserves_invalid_record_and_missing_session_errors() {
    for (record, error) in [
        ("invalid-id", "The last local Bus session record is invalid"),
        (
            "0123456789abcdef",
            "Bus session '0123456789abcdef' was not found",
        ),
    ] {
        let home = isolated_home("stop-invalid");
        let base = home.join(".local/share/bus");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("last-session"), record).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_bus"))
            .arg("stop")
            .env_remove("BUS_DATA_DIR")
            .env_remove("BUS_SESSION_ID")
            .env("HOME", &home)
            .output()
            .expect("stop with an invalid session record");
        let retained_record = std::fs::read_to_string(base.join("last-session")).unwrap();
        let created_sessions = base.join("sessions").exists();
        std::fs::remove_dir_all(home).unwrap();

        assert!(
            !output.status.success(),
            "an invalid record must remain an error"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
        assert!(output.stdout.is_empty());
        assert_eq!(retained_record, record);
        assert!(!created_sessions);
    }
}

#[test]
fn bus_stop_validates_explicit_data_root_without_a_recorded_session() {
    let home = isolated_home("stop-explicit-root");
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .arg("stop")
        .env("BUS_DATA_DIR", "relative-bus-root")
        .env_remove("BUS_SESSION_ID")
        .env("HOME", &home)
        .output()
        .expect("stop with an invalid explicit root");
    let created_state = home.join(".local/share/bus").exists();
    std::fs::remove_dir_all(home).unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("BUS_DATA_DIR must be an absolute directory"));
    assert!(output.stdout.is_empty());
    assert!(!created_state);
}

/// Every session runs its control socket in the data root, so a shared
/// BUS_DATA_DIR is refused before any server starts, with the fix named.
#[cfg(unix)]
#[test]
fn bus_refuses_a_shared_data_dir_before_starting_anything() {
    use std::os::unix::fs::PermissionsExt;
    let home = isolated_home("shared-root");
    let root = home.join("shared");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
        .env("BUS_DATA_DIR", &root)
        .env("HOME", &home)
        .env_remove("BUS_SESSION_ID")
        .env_remove("BUS_DEV")
        .output()
        .expect("run bus");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("chmod 700"), "{stderr}");
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        0,
        "nothing started"
    );
    std::fs::remove_dir_all(home).unwrap();
}
