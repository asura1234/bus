#[test]
fn immediate_delivery_survives_native_server_restart() {
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tools/acceptance/native_restart_test.py"
        ))
        .args(["--binary", env!("CARGO_BIN_EXE_bus")])
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
        let mut command = std::process::Command::new("python3");
        command
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tools/acceptance/native_restart_test.py"
            ))
            .args(["--binary", env!("CARGO_BIN_EXE_bus"), "--whole-session"]);
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
