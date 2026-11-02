//! Narrow launch/input port, supplied by the server composition layer.
use std::{io, sync::Arc, time::Instant};

pub(crate) struct NativePrompt<'a> {
    pub thread: &'a str,
    pub text: &'a str,
    pub steer: bool,
    pub message: &'a str,
    pub deadline: Instant,
    pub expected_turn: Option<&'a str>,
}

pub(crate) trait NativeInput: Send + Sync {
    fn launch(&self, argv: &[String]) -> io::Result<Vec<String>>;
    /// Must be nonblocking: this is queried on the App dispatch thread.
    fn has_session(&self) -> bool;
    fn prompt(&self, input: NativePrompt<'_>) -> Option<io::Result<()>>;
    fn interrupt(&self, thread: &str, deadline: Instant) -> Option<io::Result<()>>;
}

pub(crate) type NativeInputHandle = Arc<dyn NativeInput>;
