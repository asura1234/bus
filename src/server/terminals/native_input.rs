//! Compose provider ownership into the terminal's narrow native input port.
use crate::{
    agents::providers::codex::native::Runtime,
    terminal::{
        runtime::{
            native_input::{NativeInput, NativePrompt},
            PaneLaunchEnv,
        },
        TerminalRuntime,
    },
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::Instant,
};

struct CodexInput(Mutex<Runtime>);

impl NativeInput for CodexInput {
    fn launch(&self, argv: &[String]) -> io::Result<Vec<String>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .launch(argv)
    }
    fn has_session(&self) -> bool {
        self.0
            .try_lock()
            .map_or(true, |runtime| runtime.has_session())
    }
    fn prompt(&self, input: NativePrompt<'_>) -> Option<io::Result<()>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).prompt(
            input.thread,
            input.text,
            input.steer,
            input.message,
            input.deadline,
            input.expected_turn,
        )
    }
    fn interrupt(&self, thread: &str, deadline: Instant) -> Option<io::Result<()>> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .interrupt(thread, deadline)
    }
}

pub(in crate::server) fn bind(runtime: &TerminalRuntime, env: &PaneLaunchEnv) {
    if let Some(provider) = Runtime::from_environment(env.extra()) {
        runtime.bind_native_input(Arc::new(CodexInput(Mutex::new(provider))));
    }
}

#[cfg(test)]
pub(in crate::server) fn test_bind(
    runtime: &TerminalRuntime,
    socket: std::path::PathBuf,
) -> crate::terminal::runtime::native_input::NativeInputHandle {
    let input: crate::terminal::runtime::native_input::NativeInputHandle =
        Arc::new(CodexInput(Mutex::new(
            crate::agents::providers::codex::native::test_support::connected_runtime(socket),
        )));
    runtime.bind_native_input(input.clone());
    input
}
