//! Best-effort hints over the existing private coordinator endpoint.
//! The durable envelope is the source of truth; this frame carries no event data.
use crate::platform::ipc;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Hint {
    callback_wake: String,
}

pub(crate) fn ready_launch(bytes: &[u8]) -> Option<String> {
    let hint: Hint = serde_json::from_slice(bytes).ok()?;
    let launch = hint.callback_wake;
    (!launch.is_empty()
        && launch.len() <= 128
        && launch
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)))
    .then_some(launch)
}

pub(super) fn notify(dir: &Path, launch: &str) {
    // Spools use <private data root>/callbacks/<launch>. Never infer the live
    // session, make endpoints, or wait for the coordinator to apply the event.
    let Some(data) = dir
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == "callbacks"))
        .and_then(Path::parent)
    else {
        return;
    };
    let _ = send(data, launch);
}

fn send(data: &Path, launch: &str) -> std::io::Result<()> {
    ipc::validate_private_socket_directory(data)?;
    let end = Instant::now() + Duration::from_millis(20);
    let mut stream =
        ipc::connect_local_stream_timeout(&data.join("control.sock"), Duration::from_millis(20))?;
    let result = (|| {
        let mut bytes = serde_json::to_vec(&Hint {
            callback_wake: launch.into(),
        })?;
        bytes.push(b'\n');
        let mut offset = 0;
        while Instant::now() < end {
            if offset < bytes.len() {
                match stream.write(&bytes[offset..]) {
                    Ok(0) => break,
                    Ok(n) => offset += n,
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(e) => return Err(e),
                }
            } else if !matches!(
                ipc::poll_local_stream_read_count(&mut stream, &mut [0])?,
                ipc::LocalStreamReadCount::Pending
            ) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "Callback wake hint timed out",
        ))
    })();
    // A missed hint is safe: the staged callback survives for polling/recovery.
    // Never let Windows linger past the hook's bounded notification budget.
    ipc::discard_local_stream_output_on_close(&stream);
    result
}

#[cfg(test)]
#[path = "../tests/spool_wake_test.rs"]
mod tests;
