//! Synthetic CLI-hook smoke: no provider process, API server or model is started.
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn bus_callback_dispatches_inside_inherited_herdr_session_and_spools_atomically() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bus-cli-callback-{nonce}"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"agent_id":2,"provider":"codex","launch_id":"launch-fixture"}"#,
    )
    .unwrap();
    let payload = r#"{"session_id":"fixture-session","transcript_path":"/tmp/bus-fixture.jsonl","hook_event_name":"UserPromptSubmit","turn_id":"fixture-turn","prompt":"fixture @literal $HOME"}"#;
    for _ in 0..2 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_herdr"))
            .args(["--bus-callback", "codex-hook"])
            .env("HERDR_ENV", "1")
            .env("BUS_CALLBACK_DIR", &dir)
            .env("BUS_LAUNCH_ID", "launch-fixture")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, b"{}\n");
        assert!(output.stderr.is_empty());
    }
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(Result::unwrap)
        .filter(|e| e.file_name().to_string_lossy().starts_with("event-"))
        .collect();
    assert_eq!(files.len(), 1);
    let event: serde_json::Value =
        serde_json::from_slice(&std::fs::read(files[0].path()).unwrap()).unwrap();
    assert_eq!(event["sequence"], 1);
    assert_eq!(event["manifest"]["agent_id"], 2);
    assert_eq!(event["value"]["turn_id"], "fixture-turn");
    assert_eq!(event["value"]["prompt"], "fixture @literal $HOME");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn project_hook_is_inert_without_bus_launch_environment() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args(["--bus-callback", "cursor-hook"])
        .env("HERDR_ENV", "1")
        .env_remove("BUS_CALLBACK_DIR")
        .env_remove("BUS_LAUNCH_ID")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"{}\n");
    assert!(output.stderr.is_empty());
}

#[test]
fn callback_cli_rejects_wrong_launch_and_oversized_input_without_spooling() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bus-cli-rejected-{nonce}"));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"agent_id":2,"provider":"codex","launch_id":"expected"}"#,
    )
    .unwrap();
    for (launch, input, error) in [
        ("wrong", b"{}".to_vec(), "identity mismatch"),
        ("expected", vec![b' '; 2 * 1024 * 1024 + 1], "exceeds 2 MiB"),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_herdr"))
            .args(["--bus-callback", "codex-hook"])
            .env("HERDR_ENV", "1")
            .env("BUS_CALLBACK_DIR", &dir)
            .env("BUS_LAUNCH_ID", launch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
    }
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}
