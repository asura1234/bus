use serde_json::Value;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[cfg(test)]
use tokio::sync::Notify;

const MAX_OUTPUT: u64 = 64 * 1024;

#[derive(Clone, Default)]
pub(crate) struct Cancellation(Arc<Mutex<bool>>);

impl Cancellation {
    pub(crate) fn cancel(&self) {
        *self.0.lock().expect("Feishu cancellation lock poisoned") = true;
    }

    pub(super) fn is_cancelled(&self) -> bool {
        *self.0.lock().expect("Feishu cancellation lock poisoned")
    }
}

#[derive(Clone)]
pub(super) struct Process {
    program: OsString,
    timeout: Duration,
    #[cfg(test)]
    pub(super) path: Option<OsString>,
    /// Tests set this so the deadline starts after a grandchild is known to be running.
    #[cfg(test)]
    arm_timeout: Option<Arc<Notify>>,
}

impl Default for Process {
    fn default() -> Self {
        Self {
            // Unit tests must never fall through to the installed real CLI.
            program: if cfg!(test) {
                "bus-test-no-real-feishu-bridge"
            } else {
                "feishu-bridge"
            }
            .into(),
            timeout: Duration::from_secs(10),
            #[cfg(test)]
            path: None,
            #[cfg(test)]
            arm_timeout: None,
        }
    }
}

impl Process {
    pub(super) fn set_program(&mut self, program: impl Into<OsString>) {
        self.program = program.into();
    }

    pub(super) fn run(
        &self,
        args: &[&str],
        input: &str,
        cancel: &Cancellation,
    ) -> Result<Value, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("Could not start feishu-bridge runtime: {error}"))?;
        runtime.block_on(self.execute(args, input, cancel))
    }

    async fn execute(
        &self,
        args: &[&str],
        input: &str,
        cancel: &Cancellation,
    ) -> Result<Value, String> {
        let mut command = std::process::Command::new(&self.program);
        #[cfg(windows)]
        crate::platform::configure_background_command(&mut command);
        command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(test)]
        if let Some(path) = &self.path {
            command.env("PATH", path);
        }
        let mut command = tokio::process::Command::from(command);
        command.kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = {
            let cancelled = cancel.0.lock().expect("Feishu cancellation lock poisoned");
            if *cancelled {
                return Err("Feishu notifications were disabled".into());
            }
            command.spawn().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    "feishu-bridge is not installed or not on PATH".to_owned()
                } else {
                    format!("Could not start feishu-bridge: {error}")
                }
            })?
        };
        let process_id = child.id();
        let mut stdin = child.stdin.take().expect("feishu-bridge stdin was piped");
        let stdout = child.stdout.take().expect("feishu-bridge stdout was piped");
        let stderr = child.stderr.take().expect("feishu-bridge stderr was piped");
        #[cfg(test)]
        if let Some(arm) = &self.arm_timeout {
            // The child is already spawned. Notify keeps a permit if the test
            // published readiness before this await, so the signal is not lost.
            if tokio::time::timeout(Duration::from_secs(90), arm.notified())
                .await
                .is_err()
            {
                terminate_tree(&mut child, process_id).await;
                return Err("feishu-bridge readiness was not observed".into());
            }
        }
        let finished = {
            let work = async {
                tokio::try_join!(
                    read_limited(stdout),
                    read_limited(stderr),
                    async {
                        stdin.write_all(input.as_bytes()).await?;
                        drop(stdin);
                        Ok::<(), std::io::Error>(())
                    },
                    child.wait(),
                )
            };
            tokio::select! {
                result = tokio::time::timeout(self.timeout, work) => {
                    result.map_err(|_| "feishu-bridge timed out".to_owned())
                        .and_then(|result| result.map_err(|error| format!("feishu-bridge IO failed: {error}")))
                }
                () = wait_cancelled(cancel) => Err("Feishu notifications were disabled".to_owned()),
            }
        };
        let (stdout, _stderr, (), status) = match finished {
            Ok(output) => output,
            Err(error) => {
                terminate_tree(&mut child, process_id).await;
                return Err(error);
            }
        };
        if !status.success() {
            return Err(format!(
                "feishu-bridge failed (exit code {:?})",
                status.code()
            ));
        }
        serde_json::from_slice(&stdout).map_err(|_| "feishu-bridge returned invalid JSON".into())
    }

    #[cfg(all(test, unix))]
    pub(super) fn at(program: impl Into<OsString>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
            path: None,
            arm_timeout: None,
        }
    }
}

async fn read_limited(reader: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "feishu-bridge output is too large",
        ));
    }
    Ok(bytes)
}

async fn wait_cancelled(cancel: &Cancellation) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn terminate_tree(child: &mut tokio::process::Child, process_id: Option<u32>) {
    if let Some(pid) = process_id {
        #[cfg(unix)]
        // SAFETY: this child was spawned in a new private process group.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let mut command = crate::platform::console_command::command("taskkill");
            command
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            let mut command = tokio::process::Command::from(command);
            command.kill_on_drop(true);
            let _ = tokio::time::timeout(Duration::from_secs(2), command.status()).await;
        }
    }
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
}

#[cfg(all(test, unix))]
pub(super) fn fixture_lease() -> std::fs::File {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("temp/feishu-process-tests.lock");
    std::fs::create_dir_all(path.parent().expect("test lock parent"))
        .expect("create test lock parent");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .expect("open fixture lock");
    file.lock()
        .expect("serialize real feishu-bridge process fixtures");
    file
}

#[cfg(all(test, unix))]
#[path = "tests/process_test.rs"]
mod tests;
