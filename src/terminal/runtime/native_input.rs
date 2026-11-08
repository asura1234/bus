//! Narrow launch/input port, supplied by the server composition layer.
use std::{
    io,
    sync::{Arc, Mutex},
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
    fn launch(&self, argv: &[String]) -> io::Result<Vec<String>>;
    /// Must be nonblocking: this is queried on the App dispatch thread.
    fn has_session(&self) -> bool;
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
pub(crate) struct PaneCodex(Arc<Mutex<Option<crate::agents::providers::codex::native::Runtime>>>);

impl PaneCodex {
    pub(super) fn new(
        handle: Arc<Mutex<Option<crate::agents::providers::codex::native::Runtime>>>,
    ) -> Self {
        Self(handle)
    }
}

impl NativeInput for PaneCodex {
    fn launch(&self, argv: &[String]) -> io::Result<Vec<String>> {
        match self
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
        {
            Some(runtime) => runtime.launch(argv),
            None => Ok(argv.to_vec()),
        }
    }

    fn has_session(&self) -> bool {
        match self.0.try_lock() {
            Ok(runtime) => runtime
                .as_ref()
                .is_some_and(|runtime| runtime.has_session()),
            // The app thread must not wait. A held lock is an in-flight call
            // on this same runtime, so admission still proceeds.
            Err(_) => true,
        }
    }

    fn prompt(&self, input: NativePrompt<'_>) -> Option<io::Result<()>> {
        self.0
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
        self.0
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
