//! A timed-out waiter must revoke its queued input before returning to the caller.
use std::{io, sync::mpsc, time::Duration};

#[derive(Debug)]
pub(crate) struct InputCompletion {
    receiver: mpsc::Receiver<io::Result<()>>,
    #[cfg(unix)]
    cancellation: Option<Cancellation>,
}

impl From<mpsc::Receiver<io::Result<()>>> for InputCompletion {
    fn from(receiver: mpsc::Receiver<io::Result<()>>) -> Self {
        Self {
            receiver,
            #[cfg(unix)]
            cancellation: None,
        }
    }
}

impl InputCompletion {
    pub(crate) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<io::Result<()>, mpsc::RecvTimeoutError> {
        let result = self.receiver.recv_timeout(timeout);
        #[cfg(unix)]
        if result.is_err() {
            if let Some(cancellation) = &self.cancellation {
                cancellation.permit.cancel();
            }
        }
        result
    }

    #[cfg(all(test, unix))]
    pub(crate) fn recv(&self) -> Result<io::Result<()>, mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Legacy fire-and-forget input has no waiter to cancel it. Its actor-side
    /// deadline still applies; observed writes retain this completion instead.
    pub(crate) fn into_receiver(self) -> mpsc::Receiver<io::Result<()>> {
        #[cfg(unix)]
        if let Some(mut cancellation) = self.cancellation {
            cancellation.armed = false;
        }
        self.receiver
    }

    #[cfg(unix)]
    pub(super) fn tracked(receiver: mpsc::Receiver<io::Result<()>>, permit: WritePermit) -> Self {
        Self {
            receiver,
            cancellation: Some(Cancellation {
                permit,
                armed: true,
            }),
        }
    }
}

#[cfg(unix)]
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

#[cfg(unix)]
#[derive(Clone)]
pub(super) struct WritePermit {
    cancelled: Arc<Mutex<bool>>,
    pub(super) deadline: Option<Instant>,
    wake: crate::terminal::pty::fd::WakeWriter,
}

#[cfg(unix)]
impl WritePermit {
    pub(super) fn new(
        deadline: Option<Instant>,
        wake: crate::terminal::pty::fd::WakeWriter,
    ) -> Self {
        Self {
            cancelled: Arc::new(Mutex::new(false)),
            deadline,
            wake,
        }
    }

    // Hold this guard across each nonblocking write. Cancellation takes the
    // same lock, so once it returns no write can still be in flight or start.
    pub(super) fn lock(&self) -> io::Result<MutexGuard<'_, bool>> {
        let cancelled = self
            .cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *cancelled
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PTY input submission expired or was cancelled",
            ))
        } else {
            Ok(cancelled)
        }
    }

    fn cancel(&self) {
        *self
            .cancelled
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        let _ = self.wake.wake();
    }
}

#[cfg(unix)]
struct Cancellation {
    permit: WritePermit,
    armed: bool,
}

#[cfg(unix)]
impl std::fmt::Debug for Cancellation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cancellation")
            .field("armed", &self.armed)
            .finish_non_exhaustive()
    }
}

#[cfg(unix)]
impl Drop for Cancellation {
    fn drop(&mut self) {
        if self.armed {
            self.permit.cancel();
        }
    }
}
