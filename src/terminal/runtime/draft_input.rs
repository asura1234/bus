//! Serialize a draft transfer with developer input, without blocking the UI.
use super::io::PaneRuntimeIo;
use bytes::Bytes;
use std::{
    io,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// A pause longer than an ordinary inter-key gap, including short thinking
/// pauses. Every key/text/paste sent by the TUI resets this pane-local clock.
pub(crate) const DRAFT_QUIET_WINDOW: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct DraftInput {
    last_key: Option<Instant>,
    buffered: Option<Vec<u8>>,
    revision: u64,
    pending: Option<PendingPaste>,
    working_started: Option<Instant>,
    completed_turn: Option<(Instant, Instant)>,
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
}

impl DraftInput {
    pub(super) fn send(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
        bytes: Bytes,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<Bytes>> {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.last_key = Some(io.submission_clock().now());
        state.revision = state.revision.wrapping_add(1);
        state.pending = None;
        if let Some(buffered) = &mut state.buffered {
            buffered.extend_from_slice(&bytes);
            return Ok(());
        }
        io.try_send_bytes(bytes)
    }

    pub(super) fn acquire(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
    ) -> io::Result<DraftInputLease> {
        Self::acquire_inner(state, io, true)
    }

    /// Launch input has its own positive prompt and empty-buffer witness. It
    /// must not delay a shell startup answer by the provider-draft quiet window.
    pub(super) fn acquire_launch(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
    ) -> io::Result<DraftInputLease> {
        Self::acquire_inner(state, io, false)
    }

    fn acquire_inner(
        state: &SharedDraftInput,
        io: &PaneRuntimeIo,
        require_quiet: bool,
    ) -> io::Result<DraftInputLease> {
        let mut input = state
            .lock()
            .map_err(|_| io::Error::other("pane input lock poisoned"))?;
        if input.buffered.is_some()
            || require_quiet
                && input.last_key.is_some_and(|at| {
                    io.submission_clock().now().saturating_duration_since(at) < DRAFT_QUIET_WINDOW
                })
        {
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
        })
    }

    pub(super) fn invalidate(state: &SharedDraftInput) {
        let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
        state.revision = state.revision.wrapping_add(1);
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
                let _ = completion.into_receiver();
            }
            Err(error) => {
                recover_input(&bytes);
                tracing::error!(event = "bus.draft.input_replay_failed", %error, "Could not replay buffered pane input");
            }
        }
    }
}
