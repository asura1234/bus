//! No-model executable checks. These never start provider processes.
use std::process::Command;

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
