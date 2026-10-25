//! Cancellation stays owned until the submission's outcome is published.
use std::{io, sync::mpsc, time::Duration};

#[derive(Debug)]
pub(crate) struct InputCompletion {
    receiver: mpsc::Receiver<io::Result<()>>,
    enter_delay_allowance: Duration,
    #[cfg(unix)]
    cancellation: Option<Cancellation>,
}

impl From<mpsc::Receiver<io::Result<()>>> for InputCompletion {
    fn from(receiver: mpsc::Receiver<io::Result<()>>) -> Self {
        Self {
            receiver,
            enter_delay_allowance: Duration::ZERO,
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

    /// Settle cancellation before formatting a timeout response. A timed-out
    /// write may have left bytes already accepted by the PTY in the composer.
    pub(crate) fn wait_for_api(&self, timeout: Duration) -> io::Result<()> {
        let error = match self.recv_timeout(timeout) {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => error,
            Err(mpsc::RecvTimeoutError::Timeout) => io::Error::new(
                io::ErrorKind::TimedOut,
                "Native prompt submission exceeded its hard deadline",
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "pty actor closed")
            }
        };
        #[cfg(unix)]
        if let Some(cancellation) = &self.cancellation {
            return Err(cancellation.permit.with_draft_guidance(error));
        }
        Err(error)
    }

    pub(crate) fn with_enter_delay_allowance(mut self, delay: Duration) -> Self {
        self.enter_delay_allowance = delay;
        self
    }

    pub(crate) fn enter_delay_allowance(&self) -> Duration {
        self.enter_delay_allowance
    }

    #[cfg(all(test, unix))]
    pub(crate) fn recv(&self) -> Result<io::Result<()>, mpsc::RecvError> {
        self.receiver.recv()
    }

    /// Only callers that intentionally send without waiting, such as dialog
    /// automation, may detach. Prompt APIs must retain cancellation ownership.
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
            enter_delay_allowance: Duration::ZERO,
            cancellation: Some(Cancellation {
                permit,
                armed: true,
            }),
        }
    }
}

/// One API response owns the whole observed submission, including keys the
/// background observer might enqueue after its current write has settled.
#[derive(Clone, Default)]
pub(crate) struct InputScope {
    #[cfg(unix)]
    permit: Option<WritePermit>,
}

impl InputScope {
    pub(crate) fn completion(&self, receiver: mpsc::Receiver<io::Result<()>>) -> InputCompletion {
        #[cfg(unix)]
        if let Some(permit) = &self.permit {
            return InputCompletion::tracked(receiver, permit.clone());
        }
        receiver.into()
    }

    #[cfg(unix)]
    pub(super) fn new(wake: crate::terminal::pty::fd::WakeWriter) -> Self {
        Self {
            permit: Some(WritePermit::new(None, wake)),
        }
    }
}

#[cfg(unix)]
use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::Instant,
};

#[cfg(unix)]
#[derive(Default)]
struct WriteState {
    cancelled: bool,
    started: bool,
}

#[cfg(unix)]
#[derive(Clone)]
pub(super) struct WritePermit {
    state: Arc<Mutex<WriteState>>,
    scope: Option<Arc<Mutex<WriteState>>>,
    pub(super) deadline: Option<Instant>,
    wake: crate::terminal::pty::fd::WakeWriter,
}

#[cfg(unix)]
pub(super) struct WriteGuard<'a> {
    state: MutexGuard<'a, WriteState>,
    scope: Option<MutexGuard<'a, WriteState>>,
}

#[cfg(unix)]
impl WriteGuard<'_> {
    pub(super) fn mark_started(&mut self) {
        self.state.started = true;
        if let Some(scope) = &mut self.scope {
            scope.started = true;
        }
    }
}

#[cfg(unix)]
impl WritePermit {
    pub(super) fn new(
        deadline: Option<Instant>,
        wake: crate::terminal::pty::fd::WakeWriter,
    ) -> Self {
        Self::in_scope(deadline, wake, &InputScope::default())
    }

    pub(super) fn in_scope(
        deadline: Option<Instant>,
        wake: crate::terminal::pty::fd::WakeWriter,
        scope: &InputScope,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(WriteState::default())),
            scope: scope
                .permit
                .as_ref()
                .map(|permit| Arc::clone(&permit.state)),
            deadline,
            wake,
        }
    }

    // The scope lock is always acquired before the per-write lock. Both stay
    // held across each nonblocking write, so API cancellation is a write barrier.
    pub(super) fn lock(&self) -> io::Result<WriteGuard<'_>> {
        let scope = self
            .scope
            .as_ref()
            .map(|scope| scope.lock().unwrap_or_else(|error| error.into_inner()));
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.cancelled
            || scope.as_ref().is_some_and(|scope| scope.cancelled)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "PTY input submission expired or was cancelled",
            ));
        }
        Ok(WriteGuard { state, scope })
    }

    pub(super) fn with_draft_guidance(&self, error: io::Error) -> io::Error {
        let started = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .started;
        if error.kind() == io::ErrorKind::TimedOut
            && started
            && !error.to_string().contains("before resending")
        {
            io::Error::new(error.kind(), format!("{error}. A partial, unsubmitted Bus draft may be left in the agent's composer. Inspect the agent's transcript and composer before resending; preserve your edits while removing any remaining Bus draft"))
        } else {
            error
        }
    }

    fn cancel(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .cancelled = true;
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
