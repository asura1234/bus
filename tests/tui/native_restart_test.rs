/// Full gates opt in explicitly; default fixture invocations keep ordinary cleanup.
pub(super) fn restart_command(script: &str) -> std::process::Command {
    let mut command = std::process::Command::new("python3");
    command
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tools/acceptance")
                .join(script),
        )
        .args(["--binary", env!("CARGO_BIN_EXE_bus")]);
    if std::env::var_os("TEST_BUS_RETAIN_RESTART_FAILURES").as_deref()
        == Some(std::ffi::OsStr::new("1"))
    {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("restart fixture clock")
            .as_nanos();
        let root = std::path::PathBuf::from(format!(
            "/private/tmp/brf-{:x}-{nonce:x}",
            std::process::id()
        ));
        let socket = root.join("claude/data/herdr-config/sessions/bus/herdr-client.sock");
        assert!(socket.as_os_str().as_encoded_bytes().len() <= 96);
        // Python claims this new root exclusively and removes it only on success.
        command.arg("--retain-failure").arg("--run-dir").arg(root);
    }
    command
}

#[test]
fn immediate_delivery_survives_native_server_restart() {
    let output = restart_command("native_restart_test.py")
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
        let mut command = restart_command("native_restart_test.py");
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
