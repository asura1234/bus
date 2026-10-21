use super::*;

#[test]
fn fixture_environment_is_not_mistaken_for_an_inherited_live_bus() {
    const CHILD: &str = "BUS_TEST_FIXTURE_ENV_ASSERTION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "live_bus_env::fixture_environment_tests::fixture_environment_is_not_mistaken_for_an_inherited_live_bus",
                "--test-threads",
                "1",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    let before = std::env::var_os("BUS_DATA_DIR");
    let fake = std::env::temp_dir().join(format!("bus-controlled-env-{}", std::process::id()));
    std::env::set_var("BUS_DATA_DIR", &fake);
    // Other libtest workers can inspect the environment while an existing
    // fixture has configured its own isolated root.
    let inspected = std::thread::spawn(|| {
        std::panic::catch_unwind(no_live_bus_variable_reaches_this_test_binary)
    })
    .join()
    .unwrap();
    match before {
        Some(value) => std::env::set_var("BUS_DATA_DIR", value),
        None => std::env::remove_var("BUS_DATA_DIR"),
    }
    assert!(
        inspected.is_ok(),
        "a controlled fixture root was reported as an inherited live Bus"
    );
}
