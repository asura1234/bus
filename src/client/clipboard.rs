use std::{ffi::OsStr, io::Write};

use tracing::warn;

pub(super) fn decode_clipboard_payload(data: &str) -> Option<Vec<u8>> {
    use base64::Engine;

    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

pub(super) fn forward_clipboard(data: &str) -> bool {
    let Some(bytes) = decode_clipboard_payload(data) else {
        warn!("received invalid clipboard payload from server");
        return false;
    };
    write_osc52_bytes(&bytes);
    true
}

fn osc52_sequence(bytes: &[u8]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("\x1b]52;c;{encoded}\x07")
}

fn contains_wsl_marker(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("microsoft") || lower.contains("wsl2") || lower.contains("-wsl")
}

fn is_wsl_for_env(
    os_release: Option<&str>,
    proc_version: Option<&str>,
    wsl_distro_name: Option<&OsStr>,
    wsl_interop: Option<&OsStr>,
    runtime_marker_exists: bool,
) -> bool {
    wsl_distro_name.is_some()
        || wsl_interop.is_some()
        || os_release.is_some_and(contains_wsl_marker)
        || proc_version.is_some_and(contains_wsl_marker)
        || runtime_marker_exists
}

fn is_wsl() -> bool {
    let os_release = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    let proc_version = std::fs::read_to_string("/proc/version").ok();
    is_wsl_for_env(
        os_release.as_deref(),
        proc_version.as_deref(),
        std::env::var_os("WSL_DISTRO_NAME").as_deref(),
        std::env::var_os("WSL_INTEROP").as_deref(),
        std::path::Path::new("/run/WSL").exists()
            || std::path::Path::new("/proc/sys/fs/binfmt_misc/WSLInterop").exists(),
    )
}

fn should_prefer_osc52_for_env(
    ssh_connection: Option<&OsStr>,
    ssh_tty: Option<&OsStr>,
    vscode_ipc_hook_cli: Option<&OsStr>,
    wsl: bool,
) -> bool {
    ssh_connection.is_some() || ssh_tty.is_some() || vscode_ipc_hook_cli.is_some() || wsl
}

fn should_prefer_osc52() -> bool {
    should_prefer_osc52_for_env(
        std::env::var_os("SSH_CONNECTION").as_deref(),
        std::env::var_os("SSH_TTY").as_deref(),
        std::env::var_os("VSCODE_IPC_HOOK_CLI").as_deref(),
        is_wsl(),
    )
}

/// Write clipboard bytes to the system clipboard via native platform tools or OSC 52.
///
/// OSC 52 format: `ESC ] 52 ; c ; <base64> BEL`
///
/// Some terminals still only honor BEL-terminated OSC 52 writes, so bus
/// emits BEL here even though ST works in newer emulators.
pub fn write_osc52_bytes(bytes: &[u8]) {
    if !should_prefer_osc52() && crate::platform::write_clipboard(bytes) {
        return;
    }

    let sequence = osc52_sequence(bytes);
    let _ = std::io::stdout().write_all(sequence.as_bytes());
    let _ = std::io::stdout().flush();
}

#[cfg(test)]
#[path = "clipboard/tests/output_test.rs"]
mod tests;
