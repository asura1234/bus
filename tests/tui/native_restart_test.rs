/// Full gates opt in explicitly; default fixture invocations keep ordinary cleanup.
pub(super) fn restart_command(script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("python3");
    command
        .arg(script)
        .args(["--binary", env!("CARGO_BIN_EXE_bus")]);
    if std::env::var_os("TEST_BUS_RETAIN_RESTART_FAILURES").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        let fixture = super::utils::test_temp::unique_temp_path("rf");
        // Keep the shared PID/clock/sequence identity; compact only its label.
        let identity = fixture
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix('b'))
            .and_then(|name| name.strip_suffix("-rf"))
            .expect("shared fixture identity");
        let root = fixture.with_file_name(format!("rf{identity}"));
        let socket = root.join("claude/data/herdr-config/sessions/bus/herdr-client.sock");
        assert!(socket.as_os_str().as_encoded_bytes().len() <= 96);
        // Python claims this new root exclusively and removes it only on success.
        command.arg("--retain-failure").arg("--run-dir").arg(root);
    }
    command
}

#[test]
fn immediate_delivery_survives_native_server_restart() {
    let output = restart_command(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tools/acceptance/native_restart_test.py"
    ))
    .output()
    .expect("run isolated immediate native restart scenario");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn whole_bus_restart_and_rollback_preserve_native_provider_session_and_draft() {
    for rollback in [false, true] {
        let mut command = restart_command(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tools/acceptance/native_restart_test.py"
        ));
        command.arg("--whole-session");
        if rollback {
            command.arg("--unhealthy");
        }
        let output = command
            .output()
            .expect("run isolated whole-session restart");
        assert!(
            output.status.success(),
            "rollback={rollback}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
