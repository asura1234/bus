//! Synthetic CLI-hook smoke: no provider process, API server or model is started.
use std::{
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn reply_capture_diagnostics_distinguish_empty_missing_and_ignored_callbacks() {
    let root = std::env::temp_dir().join(format!(
        "bus-hook-shape-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    for (index, (text, transcript, expected, parsed)) in [
        (serde_json::Value::Null, true, "null", "invalid"),
        (serde_json::json!(""), true, "empty", "invalid"),
        (serde_json::json!("PRIVATE_REPLY"), true, "text", "parsed"),
        (serde_json::json!("PRIVATE_REPLY"), false, "text", "ignored"),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = root.join(index.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            r#"{"agent_id":2,"provider":"codex","launch_id":"launch"}"#,
        )
        .unwrap();
        let mut payload = serde_json::json!({
            "hook_event_name":"Stop", "session_id":"shape-session", "turn_id":"shape-turn",
            "last_assistant_message":text, "prompt":"PRIVATE_PROMPT"
        });
        if transcript {
            payload["transcript_path"] = serde_json::json!("/PRIVATE_PATH/transcript.jsonl");
        }
        let mut child = Command::new(env!("CARGO_BIN_EXE_bus"))
            .args(["--bus-callback", "codex-hook"])
            .env("BUS_CALLBACK_DIR", &dir)
            .env("BUS_LAUNCH_ID", "launch")
            .env("BUS_LOG", "bus=info")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        let log = std::fs::read_to_string(dir.join("hook.log")).unwrap();
        let shape = log.lines().find(|line| line.contains("bus.callback.payload_shape"))
            .expect("every decoded hook needs a content-free shape observation, including ignored hooks");
        for field in [
            format!("final_state=\"{expected}\""),
            format!("parse_outcome=\"{parsed}\""),
            "hook_event=\"Stop\"".into(),
            "session_id=Some(\"shape-session\")".into(),
            "turn_id=Some(\"shape-turn\")".into(),
            format!("transcript_present={transcript}"),
        ] {
            assert!(shape.contains(&field), "missing {field}: {shape}");
        }
        assert!(!log.contains("PRIVATE"), "{log}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bus_callback_dispatches_inside_inherited_herdr_session_and_spools_atomically() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("bus-cli-callback-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"agent_id":2,"provider":"codex","launch_id":"launch-fixture"}"#,
    )
    .unwrap();
    let payload = r#"{"session_id":"fixture-session","transcript_path":"/tmp/bus-fixture.jsonl","hook_event_name":"UserPromptSubmit","turn_id":"fixture-turn","prompt":"fixture @literal $HOME"}"#;
    for _ in 0..2 {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bus"))
            .args(["--bus-callback", "codex-hook"])
            .env("HERDR_ENV", "1")
            .env("BUS_DEV", "1")
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
    let log = std::fs::read_to_string(dir.join("hook.log")).unwrap();
    assert!(log.contains("bus.callback.spooled"), "{log}");
    assert!(log.contains("bus.callback.duplicate"), "{log}");
    assert!(log.contains(event["id"].as_str().unwrap()), "{log}");
    assert!(!log.contains("fixture @literal $HOME"), "{log}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn callback_cli_continues_existing_launch_spool_without_reset_or_replay() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "bus-cli-saved-spool-{}-{nonce}",
        std::process::id()
    ));
    let dir = root.join("callbacks/saved-launch");
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = br#"{"agent_id":7,"provider":"codex","launch_id":"saved-launch"}"#;
    let saved = br#"{"id":"saved","sequence":41,"at_ms":1,"manifest":{"agent_id":7,"provider":"codex","launch_id":"saved-launch"},"value":{"session_id":"saved-session","turn_id":"saved-turn"}}"#;
    std::fs::write(dir.join("manifest.json"), manifest).unwrap();
    std::fs::write(dir.join("sequence"), "41").unwrap();
    std::fs::write(dir.join("event-saved.json"), saved).unwrap();

    // Each callback is a fresh process using the previously saved launch state.
    for turn in ["next-turn", "next-turn", "following-turn"] {
        let payload = serde_json::json!({
            "session_id": "saved-session",
            "transcript_path": root.join("saved-transcript.jsonl"),
            "hook_event_name": "UserPromptSubmit",
            "turn_id": turn,
            "prompt": "synthetic saved-launch prompt"
        });
        let mut child = Command::new(env!("CARGO_BIN_EXE_bus"))
            .args(["--bus-callback", "codex-hook"])
            .env("HERDR_ENV", "1")
            .env("HERDR_SESSION", "bus")
            .env("HERDR_SOCKET_PATH", root.join("herdr.sock"))
            .env("HERDR_CLIENT_SOCKET_PATH", root.join("herdr-client.sock"))
            .env("HERDR_CONFIG_PATH", root.join("herdr-config/config.toml"))
            .env("BUS_DATA_DIR", &root)
            .env("BUS_SESSION_ID", "0123456789abcdef")
            .env("BUS_CALLBACK_DIR", &dir)
            .env("BUS_LAUNCH_ID", "saved-launch")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"{}\n");
        assert!(output.stderr.is_empty());
    }

    let mut events: Vec<serde_json::Value> = std::fs::read_dir(&dir)
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("event-"))
        .map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap())
        .collect();
    events.sort_by_key(|event| event["sequence"].as_u64().unwrap());
    assert_eq!(events.len(), 3);
    for (event, sequence) in events.iter().zip([41, 42, 43]) {
        assert_eq!(event["sequence"], sequence);
        assert_eq!(event["manifest"]["agent_id"], 7);
        assert_eq!(event["manifest"]["provider"], "codex");
        assert_eq!(event["manifest"]["launch_id"], "saved-launch");
    }
    assert_eq!(events[1]["value"]["turn_id"], "next-turn");
    assert_eq!(events[2]["value"]["turn_id"], "following-turn");
    assert_eq!(std::fs::read_to_string(dir.join("sequence")).unwrap(), "43");
    assert_eq!(std::fs::read(dir.join("manifest.json")).unwrap(), manifest);
    assert_eq!(std::fs::read(dir.join("event-saved.json")).unwrap(), saved);
    assert!(!root.join("herdr-config").exists());
    assert!(!root.join("herdr.sock").exists());
    assert!(!root.join("herdr-client.sock").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn project_hook_is_inert_without_bus_launch_environment() {
    let output = Command::new(env!("CARGO_BIN_EXE_bus"))
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
    let dir = std::env::temp_dir().join(format!("bus-cli-rejected-{}-{nonce}", std::process::id()));
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_bus"))
            .args(["--bus-callback", "codex-hook"])
            .env("HERDR_ENV", "1")
            .env("BUS_DEV", "1")
            .env("BUS_CALLBACK_DIR", &dir)
            .env("BUS_LAUNCH_ID", launch)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&input).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            !output.status.success(),
            "a failed capture must not report success"
        );
        assert_eq!(output.stdout, b"{}\n");
        assert!(String::from_utf8_lossy(&output.stderr).contains(error));
    }
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        [
            ".hook.log.archive-mode",
            "diagnostics.lock",
            "hook.log",
            "manifest.json"
        ]
    );
    let log = std::fs::read_to_string(dir.join("hook.log")).unwrap();
    assert_eq!(
        log.matches("bus.callback.capture_failed").count(),
        2,
        "{log}"
    );
    assert!(!log.contains("bus.callback.spooled"), "{log}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn held_spool_and_diagnostics_locks_leave_a_durable_hook_before_success() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("bus-cli-held-spool-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        r#"{"agent_id":2,"provider":"codex","launch_id":"launch"}"#,
    )
    .unwrap();
    let locks: Vec<_> = ["append.lock", "diagnostics.lock"]
        .into_iter()
        .map(|name| {
            let file = std::fs::OpenOptions::new()
                .create_new(true)
                .read(true)
                .write(true)
                .open(dir.join(name))
                .unwrap();
            file.lock().unwrap();
            file
        })
        .collect();
    let payload = serde_json::json!({"hook_event_name":"Stop","session_id":"session",
        "turn_id":"turn","transcript_path":"/tmp/held-spool.jsonl","last_assistant_message":"durable reply"});
    let invoke = || {
        let mut child = Command::new(env!("CARGO_BIN_EXE_bus"))
            .args(["--bus-callback", "codex-hook"])
            .env("BUS_DATA_DIR", dir.join("scratch-data"))
            .env("BUS_CALLBACK_DIR", &dir)
            .env("BUS_LAUNCH_ID", "launch")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    let output = invoke();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"{}\n");
    let pending: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(Result::unwrap)
        .filter(|e| e.file_name().to_string_lossy().starts_with("pending-"))
        .collect();
    assert_eq!(
        pending.len(),
        1,
        "success while the lock is held requires a durable envelope"
    );
    let staged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(pending[0].path()).unwrap()).unwrap();
    assert_eq!(staged["value"], payload);
    assert!(
        !dir.join("sequence").exists(),
        "the held lock must still fence publication"
    );
    drop(locks);
    std::fs::remove_dir_all(dir).unwrap();
}
