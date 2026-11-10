//! Serialize a draft transfer with developer input, without blocking the UI.
use super::io::PaneRuntimeIo;
use bytes::Bytes;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub(super) struct DraftInput {
    buffered: Option<Vec<u8>>,
    revision: u64,
    invalidation_revision: u64,
    pending: Option<PendingPaste>,
    working_started: Option<Instant>,
    completed_turn: Option<(Instant, Instant)>,
    // Shared with the runtime launch window. AtomicBool's Default is false,
    // which would hide the placeholder, so this Default starts it true.
    launch_window: Arc<AtomicBool>,
}

impl Default for DraftInput {
    fn default() -> Self {
        Self {
            buffered: None,
            revision: 0,
            invalidation_revision: 0,
            pending: None,
            working_started: None,
            completed_turn: None,
            launch_window: Arc::new(AtomicBool::new(true)),
        }
    }
}

/// Lives with the PTY, across client/coordinator reconnects, never a new PTY.
pub(super) struct PendingPaste {
    pub(super) inspect: Box<dyn super::submission::SubmissionInspector>,
    pub(super) clear: Vec<Bytes>,
    pub(super) since: Instant,
    pub(super) entered: bool,
    pub(super) withdraw_grace: Duration,
}

pub(super) type SharedDraftInput = Arc<Mutex<DraftInput>>;

pub(super) struct DraftInputLease {
    state: SharedDraftInput,
    io: PaneRuntimeIo,
    replay_safe: std::cell::Cell<bool>,
    revision: u64,
    invalidation_revision: u64,
}

impl DraftInput {
    pub(super) fn owns_pending_composer(
        state: &SharedDraftInput,
        screen: &str,
        cursor: Option<(u16, u16)>,
    ) -> bool {
        state.lock().is_ok_and(|state| {
            state.pending.as_ref().is_some_and(|pending| {
                pending.inspect.confirm_consumption()
                    && pending.inspect.recoverable_input(screen, cursor)
            })
        })
    }

    pub(super) fn shared_with_launch_window() -> (SharedDraftInput, Arc<AtomicBool>) {
        let state = SharedDraftInput::default();
        let launch_window = state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .launch_window
            .clone();
        (state, launch_window)
    }

    pub(super) fn send(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
        bytes: Bytes,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<Bytes>> {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        // A full channel returns bytes that never reached the pane.
        // A buffer-only Ok does not
        // close the launch window; only a queued write does.
        let result = if let Some(buffered) = &mut state.buffered {
            buffered.extend_from_slice(&bytes);
            Ok(())
        } else {
            let result = io.try_send_bytes(bytes);
            if result.is_ok() {
                state.launch_window.store(false, Ordering::Release);
            }
            result
        };
        if result.is_ok() {
            state.revision = state.revision.wrapping_add(1);
            state.pending = None;
        }
        result
    }

    pub(super) fn acquire(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
    ) -> io::Result<DraftInputLease> {
        Self::acquire_inner(state, io)
    }

    /// Launch input shares the same exclusive input buffer as a draft swap.
    pub(super) fn acquire_launch(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
    ) -> io::Result<DraftInputLease> {
        Self::acquire_inner(state, io)
    }

    fn acquire_inner(state: &SharedDraftInput, io: &PaneRuntimeIo) -> io::Result<DraftInputLease> {
        let mut input = state
            .lock()
            .map_err(|_| io::Error::other("pane input lock poisoned"))?;
        if input.buffered.is_some() {
            return Err(io::Error::other(
                super::submission::PromptNotShown::ComposerBusy,
            ));
        }
        input.buffered = Some(Vec::new());
        Ok(DraftInputLease {
            state: state.clone(),
            io: io.clone(),
            replay_safe: std::cell::Cell::new(false),
            revision: input.revision,
            invalidation_revision: input.invalidation_revision,
        })
    }

    pub(super) fn invalidate(state: &SharedDraftInput) {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.revision = state.revision.wrapping_add(1);
        state.invalidation_revision = state.invalidation_revision.wrapping_add(1);
        state.pending = None;
        state.working_started = None;
        state.completed_turn = None;
    }

    pub(super) fn observe_turn(
        state: &SharedDraftInput,
        agent_state: crate::agents::AgentState,
        at: Instant,
    ) {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        match agent_state {
            crate::agents::AgentState::Working => {
                state.working_started.get_or_insert(at);
            }
            crate::agents::AgentState::Idle => {
                if let Some(start) = state.working_started.take().filter(|start| at >= *start) {
                    state.completed_turn = Some((start, at));
                }
            }
            _ => {}
        }
    }
}

impl DraftInputLease {
    pub(super) fn restored(&self) {
        self.replay_safe.set(true);
    }

    pub(super) fn take_pending(&self) -> Option<PendingPaste> {
        self.state.lock().ok()?.pending.take()
    }

    pub(super) fn retain_pending(&self, pending: PendingPaste) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.revision == self.revision {
            state.pending = Some(pending);
        }
    }

    pub(super) fn with_uninvalidated<T>(
        &self,
        action: impl FnOnce() -> io::Result<T>,
    ) -> io::Result<T> {
        let state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("pane input lock poisoned"))?;
        if state.invalidation_revision != self.invalidation_revision {
            return Err(io::Error::other(
                super::submission::PromptNotShown::ComposerBusy,
            ));
        }
        action()
    }

    pub(super) fn edited(&self) -> bool {
        self.state
            .lock()
            .map_or(true, |state| state.revision != self.revision)
    }

    pub(super) fn turn_completed_since(&self, since: Instant) -> bool {
        self.state.lock().is_ok_and(|state| {
            state
                .completed_turn
                .is_some_and(|(start, end)| start >= since && end >= start)
        })
    }
}

fn recover_input(bytes: &[u8]) {
    use std::io::Write;
    let saved = (|| -> io::Result<std::path::PathBuf> {
        let directory = crate::utils::paths::data_dir()
            .join("private")
            .join("drafts");
        std::fs::create_dir_all(&directory)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path = directory.join(format!("held-input-{}-{stamp}.bin", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(path)
    })();
    match saved {
        Ok(path) => {
            tracing::error!(event = "bus.draft.input_recovery_required", recovery = %path.display(), "Pane input retained separately because draft restoration was not proven")
        }
        Err(error) => {
            tracing::error!(event = "bus.draft.input_recovery_failed", %error, "Could not retain buffered pane input")
        }
    }
}

impl Drop for DraftInputLease {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let bytes = state.buffered.take().unwrap_or_default();
        if bytes.is_empty() {
            return;
        }
        if !self.replay_safe.get() {
            recover_input(&bytes);
            return;
        }
        // Queue while holding the same lock as incoming keys: replay is after
        // restoration and before any subsequently received developer input.
        let result = self.io.queue_input_write(
            Bytes::copy_from_slice(&bytes),
            Bytes::new(),
            Duration::ZERO,
            None,
        );
        match result {
            Ok(completion) => {
                state.launch_window.store(false, Ordering::Release);
                let _ = completion.into_receiver();
            }
            Err(error) => {
                recover_input(&bytes);
                tracing::error!(event = "bus.draft.input_replay_failed", %error, "Could not replay buffered pane input");
            }
        }
    }
}
