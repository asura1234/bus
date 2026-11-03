#[test]
fn reply_delivery_survives_actual_server_restart_mid_request() {
    let output = super::native_restart::restart_command("reply_restart_test.py")
        .output()
        .expect("run isolated reply restart scenario");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
