use super::*;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub struct SpawnedBus {
    _master: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send + Sync>,
}

impl Drop for SpawnedBus {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        let _ = self.child.kill();

        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut status = 0;
                let result =
                    unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                if result == pid as libc::pid_t || result == -1 {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }

            unregister_spawned_bus_pid(Some(pid));
        }
    }
}

pub fn cleanup_spawned_bus(spawned: SpawnedBus, base: PathBuf) {
    drop(spawned);
    cleanup_test_base(&base);
}

pub fn spawn_bus(config_home: &Path, runtime_dir: &Path, socket_path: &Path) -> SpawnedBus {
    spawn_bus_with_options(config_home, runtime_dir, socket_path, None, "/bin/sh")
}

pub fn spawn_bus_with_path(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
    path_override: &Path,
) -> SpawnedBus {
    spawn_bus_with_options(
        config_home,
        runtime_dir,
        socket_path,
        Some(path_override),
        "/bin/sh",
    )
}

#[cfg(target_os = "linux")]
pub fn spawn_bus_with_shell(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
    shell: &str,
) -> SpawnedBus {
    spawn_bus_with_options(config_home, runtime_dir, socket_path, None, shell)
}

pub fn spawn_bus_with_options(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
    path_override: Option<&Path>,
    shell: &str,
) -> SpawnedBus {
    fs::create_dir_all(config_home.join("bus")).unwrap();
    fs::create_dir_all(runtime_dir).unwrap();
    register_runtime_dir(runtime_dir);
    fs::write(config_home.join("bus/config.toml"), "onboarding = false\n").unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_bus"));
    cmd.arg("server");
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("HERDR_SOCKET_PATH", socket_path);
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env("SHELL", shell);
    cmd.env_remove("HERDR_ENV");
    cmd.env_remove("BUS_DATA_DIR");
    cmd.env_remove("BUS_SESSION_ID");
    cmd.env_remove("HERDR_SESSION");
    if let Some(path) = path_override {
        cmd.env("PATH", path);
    }

    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_bus_pid(child.process_id());

    SpawnedBus {
        _master: pair.master,
        child,
    }
}

pub fn write_fake_agent(bin_dir: &Path, name: &str, script: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(bin_dir).unwrap();
    let path = bin_dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{script}")).unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
}
