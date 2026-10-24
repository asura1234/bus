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
