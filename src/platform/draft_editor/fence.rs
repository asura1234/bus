//! Publish an expired swap fence without waiting on filesystem sync in the pane.
use super::{write_json, Operation, Request, REQUEST};
use std::{
    fmt,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

#[cfg(test)]
#[path = "../tests/draft_fence_hook_test.rs"]
pub(crate) mod test_hooks;

const MAX_WRITERS: usize = 8;
static WRITERS: AtomicUsize = AtomicUsize::new(0);

struct WriterPermit;

impl WriterPermit {
    fn acquire() -> io::Result<Self> {
        WRITERS
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_WRITERS).then_some(active + 1)
            })
            .map(|_| Self)
            .map_err(|_| io::Error::new(io::ErrorKind::WouldBlock, "Draft fence writers are busy"))
    }
}

impl Drop for WriterPermit {
    fn drop(&mut self) {
        WRITERS.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Debug)]
struct Committed;

impl fmt::Display for Committed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Expired draft swap was fenced")
    }
}

impl std::error::Error for Committed {}

pub(crate) fn expired_swap_fence_committed(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.is::<Committed>())
}

pub(super) fn publish(
    dir: PathBuf,
    request: Request,
    lock: File,
    remaining: Duration,
) -> io::Error {
    match publish_and_wait(dir, request, lock, remaining) {
        Ok(()) => io::Error::new(io::ErrorKind::WouldBlock, Committed),
        Err(error) => error,
    }
}

fn publish_and_wait(
    dir: PathBuf,
    request: Request,
    lock: File,
    remaining: Duration,
) -> io::Result<()> {
    let started = std::time::Instant::now();
    let timed_out = || {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "Draft fence did not commit before the deadline",
        )
    };
    if remaining.is_zero() {
        return Err(timed_out());
    }
    let permit = WriterPermit::acquire()?;
    let (send, receive) = mpsc::channel();
    std::thread::Builder::new()
        .name("bus-draft-fence".into())
        .spawn(move || {
            let result = write(
                &dir,
                &Request {
                    operation: Operation::ExpiredSwap,
                    verify_text: None,
                    ..request
                },
            );
            // Keep the callback excluded until both the file and directory
            // have been synced. A timed-out caller cannot see a partial receipt.
            drop(lock);
            drop(permit);
            let _ = send.send(result);
        })?;
    let left = remaining.saturating_sub(started.elapsed());
    if left.is_zero() {
        return Err(timed_out());
    }
    receive.recv_timeout(left).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => timed_out(),
        mpsc::RecvTimeoutError::Disconnected => io::Error::other("Draft fence writer exited"),
    })?
}

fn write(dir: &Path, request: &Request) -> io::Result<()> {
    let path = dir.join(REQUEST);
    #[cfg(test)]
    if let Some(pause) = test_hooks::take(dir) {
        return crate::platform::fs::atomic_write_detailed_with_checkpoint(
            &path,
            &serde_json::to_vec(request)?,
            pause,
        )
        .map_err(|error| error.source);
    }
    write_json(&path, request)
}
