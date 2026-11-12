use super::*;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

const GRANDCHILD_READY_TIMEOUT: Duration = Duration::from_secs(60);
const GRANDCHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

struct Fixture(PathBuf, std::fs::File);

impl Fixture {
    fn new() -> Self {
        let lease = super::fixture_lease();
        let root = crate::utils::test_temp::unique_temp_path("feishu-process");
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join("feishu-bridge");
        std::fs::write(&program, include_str!("process_fixture.txt")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(root, lease)
    }

    fn process(&self) -> Process {
        Process::at(self.0.join("feishu-bridge"), Duration::from_secs(10))
    }

    fn hanging_wrapper(&self) -> Process {
        let program = self.0.join("bridge-wrapper");
        std::fs::write(
            &program,
            "#!/bin/sh\nsleep 60 &\nprintf '%s' \"$!\" > \"${0%/*}/grandchild.pid\"\nprintf '%s\\n' \"$!\" > \"${0%/*}/grandchild.ready\"\nwait\n",
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        publish_ready_fifo(&self.0.join("grandchild.ready"));
        // Three seconds is the timeout under test. It starts only after the
        // ready pipe is read, so a slow shell still has a running grandchild
        // when the deadline fires. That child sleeps far past the deadline.
        Process::at(program, Duration::from_secs(3))
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

fn publish_ready_fifo(path: &Path) {
    let bytes = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: `bytes` is a NUL-terminated path owned by this fixture.
    let created = unsafe { libc::mkfifo(bytes.as_ptr(), 0o600) };
    assert_eq!(
        created,
        0,
        "mkfifo failed: {}",
        std::io::Error::last_os_error()
    );
}

struct ReadyPipe {
    rx: mpsc::Receiver<std::io::Result<String>>,
}

impl ReadyPipe {
    fn open(dir: &Path) -> Self {
        let path = dir.join("grandchild.ready");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let outcome = (|| {
                let mut file = std::fs::File::open(&path)?;
                let mut text = String::new();
                file.read_to_string(&mut text)?;
                Ok(text)
            })();
            let _ = tx.send(outcome);
        });
        Self { rx }
    }

    fn wait(self, timeout: Duration) -> Result<u32, String> {
        let text = match self.rx.recv_timeout(timeout) {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => return Err(format!("ready pipe failed: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "grandchild ready pipe was not written within {timeout:?}"
                ));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("ready pipe reader exited before publishing a pid".into());
            }
        };
        let pid = text
            .trim()
            .parse::<u32>()
            .map_err(|error| format!("ready pipe pid: {error} ({text})"))?;
        if pid == 0 {
            return Err(format!("ready pipe published pid 0 ({text})"));
        }
        Ok(pid)
    }
}

fn wait_until_exited(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let state = String::from_utf8_lossy(&output.stdout);
        if has_exited(&state) {
            return;
        }
        if Instant::now() >= deadline {
            panic!("grandchild {pid} still running after {timeout:?}: {state}");
        }
        std::thread::sleep(Duration::from_millis(10));
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
    let mut process = fixture.hanging_wrapper();
    let arm = std::sync::Arc::new(tokio::sync::Notify::new());
    process.arm_timeout = Some(std::sync::Arc::clone(&arm));
    let ready = ReadyPipe::open(&fixture.0);
    let job = std::thread::spawn(move || process.run(&[], "", &Cancellation::default()));
    let pid = match ready.wait(GRANDCHILD_READY_TIMEOUT) {
        Ok(pid) => pid,
        Err(error) => {
            arm.notify_one();
            let _ = job.join();
            panic!("{error}");
        }
    };
    arm.notify_one();
    let error = job.join().unwrap().unwrap_err();
    assert!(error.contains("timed out"), "{error}");
    wait_until_exited(pid, GRANDCHILD_EXIT_TIMEOUT);
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
    // Ceiling only. Cancel runs when the ready pipe delivers the grandchild
    // pid, so a slow start cannot turn this into the timeout case.
    process.timeout = Some(Duration::from_secs(120));
    let cancel = Cancellation::default();
    let job_cancel = cancel.clone();
    let ready = ReadyPipe::open(&fixture.0);
    let job = std::thread::spawn(move || process.run(&[], "", &job_cancel));
    let pid = match ready.wait(GRANDCHILD_READY_TIMEOUT) {
        Ok(pid) => pid,
        Err(error) => {
            cancel.cancel();
            let _ = job.join();
            panic!("{error}");
        }
    };
    cancel.cancel();
    let error = job.join().unwrap().unwrap_err();
    assert!(error.contains("disabled"), "{error}");
    wait_until_exited(pid, GRANDCHILD_EXIT_TIMEOUT);
}
