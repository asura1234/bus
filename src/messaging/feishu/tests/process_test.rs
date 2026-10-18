use super::*;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Instant;

struct Fixture(PathBuf, std::fs::File);

impl Fixture {
    fn new() -> Self {
        let lease = super::fixture_lease();
        let root = crate::utils::test_temp::unique_temp_path("feishu-process");
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join("lark-cli");
        std::fs::write(&program, include_str!("process_fixture.txt")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(root, lease)
    }

    fn process(&self) -> Process {
        Process::at(self.0.join("lark-cli"), Duration::from_secs(10))
    }

    fn hanging_wrapper(&self) -> Process {
        let program = self.0.join("lark-wrapper");
        std::fs::write(
            &program,
            "#!/bin/sh\nsleep 60 &\nprintf '%s' \"$!\" > \"${0%/*}/grandchild.pid\"\nwait\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        Process::at(program, Duration::from_secs(1))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(self.0.join("grandchild.pid")) {
            if let Ok(pid) = pid.parse::<i32>() {
                // SAFETY: the fixture wrote the pid of the child it just started.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.0);
        let _ = self.1.unlock();
    }
}

fn has_exited(state: &str) -> bool {
    let state = state.trim();
    // Darwin can show a killed, unreaped child as `?E` (exiting) before Z
    // or disappearance. It no longer executes the fixture's sleep command.
    state.is_empty() || state.starts_with('Z') || (cfg!(target_os = "macos") && state.contains('E'))
}

#[test]
fn feishu_process_preserves_argv_and_literal_stdin_without_a_shell() {
    let fixture = Fixture::new();
    let input = "[Bus]\n$HOME `touch /tmp/never-run` $(printf danger)\nquotes ' \\\"";
    let result = fixture.process().run(
        &["echo", "; $(printf never)"],
        input,
        &Cancellation::default(),
    );
    assert!(
        result.is_ok(),
        "result={result:?}; fixture stage={:?}",
        std::fs::read_to_string(fixture.0.join("stage"))
    );
    let value = result.unwrap();
    assert_eq!(
        value["args"],
        serde_json::json!(["echo", "; $(printf never)"])
    );
    assert_eq!(value["stdin"], input);
}

#[test]
fn feishu_process_bounds_output_and_never_leaks_failed_stdout_or_stderr() {
    let fixture = Fixture::new();
    let process = fixture.process();
    let large = process
        .run(&["oversize"], "", &Cancellation::default())
        .unwrap_err();
    assert!(
        large.contains("too large"),
        "{large}; fixture stage={:?}",
        std::fs::read_to_string(fixture.0.join("stage"))
    );
    for mode in ["error", "malformed"] {
        let error = process
            .run(&[mode], "", &Cancellation::default())
            .unwrap_err();
        assert!(!error.contains("DO_NOT_LEAK"), "{error}");
    }
}

#[test]
fn feishu_process_timeout_terminates_wrapper_and_grandchild_holding_pipes() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let error = fixture
        .hanging_wrapper()
        .run(&[], "", &Cancellation::default())
        .unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(4));
    let pid: u32 = std::fs::read_to_string(fixture.0.join("grandchild.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(has_exited(&state), "grandchild still running: {state}");
}

#[test]
fn feishu_process_cancel_before_start_never_invokes_the_cli() {
    let fixture = Fixture::new();
    let cancel = Cancellation::default();
    cancel.cancel();
    let error = fixture.process().run(&["echo"], "", &cancel).unwrap_err();
    assert!(error.contains("disabled"), "{error}");
    assert!(!fixture.0.join("called").exists());
}

#[test]
fn feishu_process_cancellation_terminates_running_wrapper_and_grandchild() {
    let fixture = Fixture::new();
    let mut process = fixture.hanging_wrapper();
    process.timeout = Duration::from_secs(10);
    let cancel = Cancellation::default();
    let job_cancel = cancel.clone();
    let job = std::thread::spawn(move || process.run(&[], "", &job_cancel));
    let deadline = Instant::now() + Duration::from_secs(3);
    while !fixture.0.join("grandchild.pid").exists() {
        assert!(Instant::now() < deadline, "wrapper did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    cancel.cancel();
    let error = job.join().unwrap().unwrap_err();
    assert!(error.contains("disabled"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(3));
    let pid = std::fs::read_to_string(fixture.0.join("grandchild.pid")).unwrap();
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid.trim()])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    assert!(
        has_exited(&state),
        "grandchild still running after cancellation: {state}"
    );
}
