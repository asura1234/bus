use super::*;

#[cfg(unix)]
fn capture_shell_output(command: &str, extra_env: &[(&str, &str)]) -> String {
    static CAPTURE_ID: AtomicU64 = AtomicU64::new(0);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let output_path = std::env::temp_dir().join(format!(
        "herdr-pane-term-test-{}-{}-{}.txt",
        std::process::id(),
        CAPTURE_ID.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.arg("-c");
    cmd.arg(format!("{command} > '{}'", output_path.display()));
    cmd.cwd(std::env::current_dir().unwrap());
    cmd.env("TERM", "xterm-ghostty");
    cmd.env("COLORTERM", "falsecolor");
    cmd.env("NO_COLOR", "1");
    apply_pane_terminal_env(&mut cmd);
    for (key, value) in extra_env {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "shell command failed: {status:?}");

    let output = std::fs::read_to_string(&output_path).unwrap();
    let _ = std::fs::remove_file(output_path);
    output
}

fn foreground_process(pid: u32, name: &str) -> crate::platform::ForegroundProcess {
    crate::platform::ForegroundProcess {
        pid,
        name: name.to_string(),
        argv0: None,
        argv: None,
        cmdline: None,
    }
}

fn process_probe_input() -> ProcessProbeInput {
    ProcessProbeInput {
        current_agent: None,
        foreground_pgid: Some(42),
        last_foreground_pgid: Some(42),
        has_process_probe: true,
        acquisition_age: None,
        pending_foreground_shell_clear: false,
        pending_restore_probe: false,
        elapsed_since_process_check: std::time::Duration::from_secs(1),
    }
}

#[path = "compression_test.rs"]
mod compression;
#[path = "detection_test.rs"]
mod detection;
#[path = "io_test.rs"]
mod io;
#[path = "shutdown_test.rs"]
mod shutdown;
#[path = "spawn_test.rs"]
mod spawn;
