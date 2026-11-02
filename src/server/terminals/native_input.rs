//! Attach the pane-owned Codex runtime. This does not construct a second one.
use crate::terminal::{runtime::PaneLaunchEnv, TerminalRuntime};

/// Fill an empty pane slot from the launch environment. A runtime already
/// retained at spawn, including one whose session is open, stays in place.
pub(in crate::server) fn bind(runtime: &TerminalRuntime, env: &PaneLaunchEnv) {
    let occupied = runtime
        .shared_codex_handle()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_some();
    if !occupied {
        runtime.retain_shared_codex_from_env(env.extra());
    }
}

#[cfg(test)]
pub(in crate::server) fn test_bind(
    runtime: &TerminalRuntime,
    socket: std::path::PathBuf,
) -> crate::terminal::runtime::native_input::NativeInputHandle {
    *runtime
        .shared_codex_handle()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) =
        Some(crate::agents::providers::codex::native::test_support::connected_runtime(socket));
    runtime
        .native_input()
        .expect("pane codex runtime")
}
