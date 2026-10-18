use serde_json::Value;
use std::ffi::OsString;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

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
}

impl Default for Process {
    fn default() -> Self {
        Self {
            program: "lark-cli".into(),
            timeout: Duration::from_secs(10),
        }
    }
}

impl Process {
    pub(super) fn run(
        &self,
        args: &[&str],
        input: &str,
        cancel: &Cancellation,
    ) -> Result<Value, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("Could not start lark-cli runtime: {error}"))?;
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
            .env("LARKSUITE_CLI_NO_UPDATE_NOTIFIER", "1")
            .env("LARKSUITE_CLI_NO_SKILLS_NOTIFIER", "1")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
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
                    "lark-cli is not installed or not on PATH".to_owned()
                } else {
                    format!("Could not start lark-cli: {error}")
                }
            })?
        };
        let process_id = child.id();
        let mut stdin = child.stdin.take().expect("lark-cli stdin was piped");
        let stdout = child.stdout.take().expect("lark-cli stdout was piped");
        let stderr = child.stderr.take().expect("lark-cli stderr was piped");
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
                    result.map_err(|_| "lark-cli timed out".to_owned())
                        .and_then(|result| result.map_err(|error| format!("lark-cli IO failed: {error}")))
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
            return Err(format!("lark-cli failed (exit code {:?})", status.code()));
        }
        serde_json::from_slice(&stdout).map_err(|_| "lark-cli returned invalid JSON".into())
    }

    #[cfg(all(test, unix))]
    pub(super) fn at(program: impl Into<OsString>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            timeout,
        }
    }
}

async fn read_limited(reader: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(MAX_OUTPUT + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > MAX_OUTPUT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "lark-cli output is too large",
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
        .expect("serialize real lark-cli process fixtures");
    file
}

#[cfg(all(test, unix))]
#[path = "tests/process_test.rs"]
mod tests;
