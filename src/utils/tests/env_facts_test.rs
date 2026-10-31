use super::*;
use std::ffi::OsString;

struct RestoreRoot(Option<OsString>);

impl Drop for RestoreRoot {
    fn drop(&mut self) {
        if let Some(value) = &self.0 {
            std::env::set_var("BUS_DATA_DIR", value);
        } else {
            std::env::remove_var("BUS_DATA_DIR");
        }
    }
}

#[test]
fn bus_data_dir_keeps_absence_empty_and_inherited_path_bytes() {
    let _guard = crate::utils::config::test_config_env_lock().lock().unwrap();
    let _restore = RestoreRoot(std::env::var_os("BUS_DATA_DIR"));

    std::env::remove_var("BUS_DATA_DIR");
    assert_eq!(bus_data_dir(), None);
    std::env::set_var("BUS_DATA_DIR", "");
    assert_eq!(bus_data_dir(), Some(PathBuf::new()));
    std::env::set_var("BUS_DATA_DIR", "relative/root");
    assert_eq!(bus_data_dir(), Some(PathBuf::from("relative/root")));

    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"/tmp/bus-\xff".to_vec());
        std::env::set_var("BUS_DATA_DIR", &path);
        assert_eq!(bus_data_dir(), Some(PathBuf::from(path)));
    }
}

#[test]
fn operational_environment_keys_keep_the_legacy_spelling() {
    assert_eq!(SOCKET_PATH_ENV_VAR, "HERDR_SOCKET_PATH");
    assert_eq!(STARTUP_CWD_ENV_VAR, "HERDR_STARTUP_CWD");
}

#[test]
fn command_on_path_keeps_the_bare_name_without_an_override() {
    let command = command_on_path("sh");
    assert_eq!(command.get_program(), "sh");
}

/// A test PATH override skips a mode 0644 namesake and runs the later
/// executable. Stopping on the first regular file execs that file and gets EACCES.
#[cfg(unix)]
#[test]
fn command_on_path_skips_a_non_executable_file_for_a_later_executable() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::utils::test_temp::unique_temp_path("bus-path-exec");
    let earlier = root.join("earlier");
    let later = root.join("later");
    std::fs::create_dir_all(&earlier).unwrap();
    std::fs::create_dir_all(&later).unwrap();
    let name = "bus-path-probe-tool";
    let blocked = earlier.join(name);
    std::fs::write(&blocked, b"#!/bin/sh\necho blocked\n").unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o644)).unwrap();
    let runnable = later.join(name);
    std::fs::write(&runnable, b"#!/bin/sh\necho runnable\n").unwrap();
    std::fs::set_permissions(&runnable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let joined = std::env::join_paths([&earlier, &later]).unwrap();
    let _guard = PathOverride::set(&joined);
    let mut command = command_on_path(name);
    let program = command.get_program().to_owned();
    let output = command.output();
    let _ = std::fs::remove_dir_all(&root);
    let output = output.unwrap_or_else(|error| panic!("spawn failed for {program:?}: {error}"));
    assert!(
        output.status.success(),
        "ran {program:?} status {:?} stdout {} stderr {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "runnable");
}
