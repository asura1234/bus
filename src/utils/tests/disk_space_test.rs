use crate::utils::disk_space::available_bytes;
use std::os::unix::fs::DirBuilderExt;

#[test]
pub(crate) fn scratch_disk_samples_never_fall_back_to_the_host_volume() {
    const CASE: &str = "BUS_DISK_SOURCE_TEST_CHILD";
    if let Some(case) = std::env::var_os(CASE) {
        let data = crate::utils::test_temp::unique_temp_path("bus-disk-source");
        assert!(!data.exists());
        // Only this exact test runs in the child, so its environment is private.
        std::env::remove_var("TEST_BUS_PROTECTED_HOME");
        if case == "guarded" {
            std::env::set_var("TEST_BUS_PROTECTED_HOME", data.with_extension("protected"));
        }
        if case == "normal" {
            assert!(available_bytes(&data).is_err());
            return;
        }
        // A nonexistent data path makes an accidental statvfs deterministic,
        // regardless of how much capacity the build machine has left.
        assert_eq!(available_bytes(&data).unwrap(), u64::MAX);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&data)
            .unwrap();
        let fixture = data.join("disk-free-bytes");
        std::fs::write(&fixture, "19000000000\n").unwrap();
        assert_eq!(available_bytes(&data).unwrap(), 19_000_000_000);
        std::fs::write(&fixture, "invalid").unwrap();
        assert_eq!(
            available_bytes(&data).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        std::fs::remove_dir_all(data).unwrap();
        return;
    }
    let mut cases = vec!["guarded", "normal"];
    if cfg!(feature = "dev-tools") {
        cases.extend(["fake", "driver"]);
    }
    for case in cases {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "utils::disk_space::tests::scratch_disk_samples_never_fall_back_to_the_host_volume",
                "--nocapture",
            ])
            .env(CASE, case)
            .env_remove("BUS_TUI_OWNER")
            .env_remove("BUS_TUI_FAKE_AGENT");
        if case == "fake" {
            command.env("BUS_TUI_FAKE_AGENT", "1");
        } else if case == "driver" {
            command.env("BUS_TUI_OWNER", "isolated-disk-test");
        }
        let output = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed; 0 failed;"),
            "{case}: {stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
