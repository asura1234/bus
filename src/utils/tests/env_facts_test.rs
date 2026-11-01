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

/// kimirev254's probe: `PATH=.` finds `search/tool`, then the command's
/// `current_dir` is `child/`. With no override the program stays the bare
/// name, so this process does not freeze `search/tool` before that chdir.
/// `Command::new("tool")` then lets the OS search `PATH` after `current_dir`,
/// which does not run `search/tool`.
#[cfg(unix)]
#[test]
fn subprocess_command_on_path_relative_entry() {
    let Some(root) = std::env::var_os("BUS_TEST_COMMAND_PATH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    std::env::set_current_dir(root.join("search")).unwrap();
    let command = command_on_path("tool");
    assert_eq!(command.get_program(), "tool");
}

#[cfg(unix)]
#[test]
fn command_on_path_relative_entry_stays_a_bare_name() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::utils::test_temp::unique_temp_path("bus-command-path-relative");
    std::fs::create_dir_all(root.join("search")).unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    let tool = root.join("search/tool");
    std::fs::write(&tool, b"#!/bin/sh\nprintf relative-path-ok").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "utils::env::tests::subprocess_command_on_path_relative_entry",
            "--test-threads",
            "1",
        ])
        .env("PATH", ".")
        .env("BUS_TEST_COMMAND_PATH_ROOT", &root)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        output.status.success(),
        "relative PATH lookup changed the bare program: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// An active override resolves `PATH=.` against the lookup directory and keeps
/// that absolute path across the child `current_dir`.
#[cfg(unix)]
#[test]
fn subprocess_command_on_path_relative_override() {
    let Some(root) = std::env::var_os("BUS_TEST_COMMAND_PATH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    std::env::set_current_dir(root.join("search")).unwrap();
    let _guard = PathOverride::set(".");
    let output = command_on_path("tool")
        .current_dir(root.join("child"))
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"relative-path-ok");
}

#[cfg(unix)]
#[test]
fn command_on_path_relative_override_survives_child_chdir() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::utils::test_temp::unique_temp_path("bus-command-path-override");
    std::fs::create_dir_all(root.join("search")).unwrap();
    std::fs::create_dir(root.join("child")).unwrap();
    let tool = root.join("search/tool");
    std::fs::write(&tool, b"#!/bin/sh\nprintf relative-path-ok").unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "utils::env::tests::subprocess_command_on_path_relative_override",
            "--test-threads",
            "1",
        ])
        .env("BUS_TEST_COMMAND_PATH_ROOT", &root)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    assert!(
        output.status.success(),
        "relative override did not survive child chdir: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
