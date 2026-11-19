//! Narrow launch/input port, supplied by the server composition layer.
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

pub(crate) struct NativePrompt<'a> {
    pub thread: &'a str,
    pub text: &'a str,
    pub steer: bool,
    pub message: &'a str,
    pub deadline: Instant,
    pub expected_turn: Option<&'a str>,
}

/// Positive native completion, including the provider turn generation observed.
pub(crate) struct NativeInterruptCompletion {
    pub turn_id: Option<String>,
}

pub(crate) trait NativeInput: Send + Sync {
    /// Must be nonblocking: this is queried on the App dispatch thread.
    fn has_session(&self) -> bool;
    /// Startup proof is separate from live turn admission. This call cannot wait.
    fn startup_ready(&self, _thread: &str, _frontend_idle: bool) -> bool {
        true
    }
    fn prompt(&self, input: NativePrompt<'_>) -> Option<io::Result<()>>;
    /// None permits typed fallback. Some(Err) is unconfirmed and must never
    /// replay keys or be promoted to completion by a visual idle observation.
    fn interrupt(
        &self,
        thread: &str,
        deadline: Instant,
    ) -> Option<io::Result<NativeInterruptCompletion>>;
}

pub(crate) type NativeInputHandle = Arc<dyn NativeInput>;

/// Forwards admission to the pane-owned runtime. This is not a second runtime.
pub(crate) struct PaneCodex {
    runtime: Arc<Mutex<Option<crate::agents::providers::codex::native::Runtime>>>,
    startup_confirmed: Arc<AtomicBool>,
}

impl PaneCodex {
    pub(super) fn new(
        runtime: Arc<Mutex<Option<crate::agents::providers::codex::native::Runtime>>>,
        startup_confirmed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            runtime,
            startup_confirmed,
        }
    }
}

impl NativeInput for PaneCodex {
    fn has_session(&self) -> bool {
        match self.runtime.try_lock() {
            Ok(runtime) => runtime
                .as_ref()
                .is_some_and(|runtime| runtime.has_session()),
            // The app thread must not wait. A held lock is an in-flight call
            // on this same runtime, so admission still proceeds.
            Err(_) => true,
        }
    }

    fn startup_ready(&self, thread: &str, frontend_idle: bool) -> bool {
        if self.startup_confirmed.load(Ordering::Acquire) {
            return true;
        }
        match self.runtime.try_lock() {
            Ok(runtime) => {
                let ready = runtime
                    .as_ref()
                    .is_none_or(|runtime| runtime.startup_ready(thread, frontend_idle));
                self.startup_confirmed.store(ready, Ordering::Release);
                ready
            }
            // Contention is inconclusive, never positive startup evidence.
            Err(_) => false,
        }
    }

    fn prompt(&self, input: NativePrompt<'_>) -> Option<io::Result<()>> {
        self.runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .and_then(|runtime| {
                runtime.prompt(
                    input.thread,
                    input.text,
                    input.steer,
                    input.message,
                    input.deadline,
                    input.expected_turn,
                )
            })
    }

    fn interrupt(
        &self,
        thread: &str,
        deadline: Instant,
    ) -> Option<io::Result<NativeInterruptCompletion>> {
        self.runtime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .and_then(|runtime| {
                runtime
                    .interrupt(thread, deadline)
                    .map(|result| result.map(|turn_id| NativeInterruptCompletion { turn_id }))
            })
    }
}
