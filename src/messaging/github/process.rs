//! Bounded subprocesses used only by the watcher thread, never a coordinator tick.
use std::{
    process::{Command, Stdio},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};

pub(super) fn run(program: &str, args: &[String]) -> Result<String, &'static str> {
    let (success, stdout, _) = output(program, args)?;
    if success {
        Ok(stdout)
    } else {
        Err("Git remote discovery failed")
    }
}
pub(super) fn output(
    program: &str,
    args: &[String],
) -> Result<(bool, String, String), &'static str> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Could not start GitHub subprocess runtime")?;
    runtime.block_on(execute(program, args))
}
async fn execute(program: &str, args: &[String]) -> Result<(bool, String, String), &'static str> {
    let mut command = Command::new(program);
    #[cfg(windows)]
    crate::platform::configure_background_command(&mut command);
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command = tokio::process::Command::from(command);
    command.kill_on_drop(true);
    let mut child = command.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "GitHub watch requires gh and git on PATH"
        } else {
            "GitHub watch could not start subprocess"
        }
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let work = async { tokio::try_join!(limited(stdout), limited(stderr), child.wait()) };
    match tokio::time::timeout(Duration::from_secs(20), work).await {
        Ok(Ok((stdout, stderr, status))) => Ok((status.success(), stdout, stderr)),
        failed => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            Err(if failed.is_err() {
                "GitHub subprocess timed out"
            } else {
                "GitHub subprocess output failed or exceeded limit"
            })
        }
    }
}
async fn limited(reader: impl AsyncRead + Unpin) -> Result<String, std::io::Error> {
    const LIMIT: u64 = 8 * 1024 * 1024;
    let mut bytes = Vec::new();
    reader.take(LIMIT + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > LIMIT {
        return Err(std::io::Error::other("output limit"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn github_watch_subprocess_captures_success_failure_and_missing_executable_without_network() {
        assert_eq!(
            run("/bin/sh", &["-c".into(), "printf fixture".into()]).unwrap(),
            "fixture"
        );
        let (false, stdout, stderr) = output(
            "/bin/sh",
            &["-c".into(), "printf out; printf err >&2; exit 7".into()],
        )
        .unwrap() else {
            panic!("nonzero status")
        };
        assert_eq!(stdout, "out");
        assert_eq!(stderr, "err");
        assert!(run("/bin/sh", &["-c".into(), "exit 1".into()]).is_err());
        assert!(output("/bus-test-missing-gh", &[]).is_err());
    }
}
