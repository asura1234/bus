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
    let output = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["--bus", "--paths"])
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

    let exact = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["--bus", "resume", "0123456789abcdef"])
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

    let last = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["--bus", "resume", "--last"])
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
    let output = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["--bus", "--help"])
        .output()
        .expect("run help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("A plain `bus` launch always creates a new local session."));
    assert!(help.contains("bus [--dev] resume <session-id>"));
    assert!(help.contains("bus [--dev] resume --last"));
}
