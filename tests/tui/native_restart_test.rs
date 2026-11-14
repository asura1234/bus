pub(super) struct RestartBinary {
    path: std::path::PathBuf,
    retain_failure: bool,
}

impl Drop for RestartBinary {
    fn drop(&mut self) {
        if self.retain_failure && std::thread::panicking() {
            eprintln!("Retained restart fixture binary: {}", self.path.display());
            return;
        }
        let _ = std::fs::remove_file(&self.path);
        if let Some(directory) = self.path.parent() {
            let _ = std::fs::remove_dir(directory);
        }
    }
}

/// Full gates opt in explicitly; default fixture invocations keep ordinary cleanup.
/// Keep the binary guard through assertions so failing full gates retain the copy.
pub(super) fn restart_command(script: &str) -> (std::process::Command, RestartBinary) {
    use std::os::unix::fs::DirBuilderExt;
    // A main-worktree build is a protected path to the acceptance fixtures.
    // Give them an independent copy, keeping their live-binary guard intact.
    let directory = super::utils::test_temp::unique_temp_path("restart-binary");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .expect("create private restart binary directory");
    let binary = RestartBinary {
        path: directory.join("bus"),
        retain_failure: std::env::var_os("TEST_BUS_RETAIN_RESTART_FAILURES").as_deref()
            == Some(std::ffi::OsStr::new("1")),
    };
    std::fs::copy(env!("CARGO_BIN_EXE_bus"), &binary.path)
        .expect("copy built Bus for isolated restart fixture");
    let mut command = std::process::Command::new("python3");
    command.arg(script).arg("--binary").arg(&binary.path);
    if binary.retain_failure {
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
    (command, binary)
}

#[test]
fn restart_command_stages_an_independent_binary_and_cleans_only_the_copy() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::MetadataExt;
    let source = std::path::Path::new(env!("CARGO_BIN_EXE_bus"));
    let (command, binary) = restart_command("unused.py");
    let copy = binary.path.clone();
    assert_eq!(command.get_args().nth(2), Some(copy.as_os_str()));
    assert_eq!(
        Sha256::digest(std::fs::read(&copy).unwrap()),
        Sha256::digest(std::fs::read(source).unwrap())
    );
    let original = source.metadata().unwrap();
    let staged = copy.metadata().unwrap();
    assert_ne!(
        (original.dev(), original.ino()),
        (staged.dev(), staged.ino())
    );
    drop(command);
    drop(binary);
    assert!(!copy.exists());
    assert!(!copy.parent().unwrap().exists());
    assert!(source.is_file());
}

#[test]
fn immediate_delivery_survives_native_server_restart() {
    let (mut command, _binary) = restart_command(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tools/acceptance/native_restart_test.py"
    ));
    let output = command
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
        let (mut command, _binary) = restart_command(concat!(
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
