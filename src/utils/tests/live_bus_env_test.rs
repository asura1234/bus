//! A test binary started inside a Bus agent pane inherits that live Bus's
//! variables. Every test binary includes this module, so none of them reads the
//! live Bus's data, socket or settings, and no process a test spawns reaches it.

use std::ffi::OsStr;

/// What a running Bus hands to every pane it launches besides `HERDR_*`: the
/// data root, session, dev and log mode, and the provider hook spool.
const LIVE_BUS_VARS: &[&str] = &[
    "BUS_DATA_DIR",
    "BUS_SESSION_ID",
    "BUS_DEV",
    "BUS_DEV_EXISTING_SERVER",
    "BUS_LOG",
    "BUS_LAUNCH_ID",
    "BUS_CALLBACK_DIR",
];

/// Test-only knobs (`BUS_UPDATE_API_SCHEMA`) and child-process markers
/// (`BUS_*_CHILD`) are not among them and pass through.
fn names_a_live_bus(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| name.starts_with("HERDR_") || LIVE_BUS_VARS.contains(&name))
}

/// Set once the variables are dropped. A test binary that a test starts again
/// inherits it and keeps the variables that test set on purpose.
const DROPPED: &str = "BUS_TEST_INHERITED_ENV_DROPPED";

// Runs before main, while the process is still single-threaded.
#[ctor::ctor(unsafe)]
fn drop_inherited_live_bus_variables() {
    if std::env::var_os(DROPPED).is_some() {
        return;
    }
    for (name, _) in std::env::vars_os() {
        if names_a_live_bus(&name) {
            std::env::remove_var(name);
        }
    }
    std::env::set_var(DROPPED, "1");
}

/// Set only in the child that the re-run test below starts. The check runs there
/// alone: in a shared process another test's own fixture root would look inherited.
const PROBE: &str = "BUS_TEST_LIVE_BUS_ENV_PROBE";

#[test]
fn no_live_bus_variable_reaches_this_test_binary() {
    if std::env::var_os(PROBE).is_none() {
        return;
    }
    let leaked: Vec<_> = std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| names_a_live_bus(name))
        .collect();
    assert!(
        leaked.is_empty(),
        "inherited live Bus variables: {leaked:?}"
    );
}

#[test]
fn a_test_binary_started_in_a_live_bus_pane_drops_its_variables() {
    let fake = std::env::temp_dir().join(format!("bus-live-env-{}", std::process::id()));
    let data = fake.join("data");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    // Started from a pane, not by another test.
    child.env_remove(DROPPED).args([
        "--exact",
        "live_bus_env::no_live_bus_variable_reaches_this_test_binary",
        "--test-threads",
        "1",
    ]);
    for name in LIVE_BUS_VARS {
        child.env(name, data.join(name));
    }
    for name in [
        "HERDR_ENV",
        "HERDR_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_PANE_ID",
    ] {
        child.env(name, fake.join(name));
    }
    let output = child
        .env(PROBE, "1")
        .env("BUS_UPDATE_API_SCHEMA", "kept")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    assert!(!fake.exists(), "a test wrote into the inherited Bus dir");
    assert!(!names_a_live_bus(OsStr::new("BUS_UPDATE_API_SCHEMA")));
    assert!(!names_a_live_bus(OsStr::new("BUS_SOUND_RELOAD_TEST_CHILD")));
}

#[test]
fn a_test_started_again_by_a_test_keeps_the_variables_it_was_given() {
    if let Some(expected) = std::env::var_os("BUS_TEST_EXPECT_DATA_DIR") {
        assert_eq!(std::env::var_os("BUS_DATA_DIR"), Some(expected));
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "live_bus_env::a_test_started_again_by_a_test_keeps_the_variables_it_was_given",
            "--test-threads",
            "1",
        ])
        .env("BUS_DATA_DIR", "/given")
        .env("BUS_TEST_EXPECT_DATA_DIR", "/given")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn fixture_environment_is_not_mistaken_for_an_inherited_live_bus() {
    const CHILD: &str = "BUS_TEST_FIXTURE_ENV_ASSERTION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "live_bus_env::fixture_environment_is_not_mistaken_for_an_inherited_live_bus",
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
