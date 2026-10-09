//! Observation-only hook entry, before ordinary CLI parsing and session checks.
use super::{claude_code::statusline, spool, ProviderKind};
use crate::utils::logging::{self, LoggingOptions};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// Process composition supplies the inherited spool log root and logging policy.
pub(crate) fn dispatch(
    args: &[String],
    diagnostics_dir: Option<PathBuf>,
    logging_options: &LoggingOptions,
) -> Option<io::Result<()>> {
    if args.get(1).map(String::as_str) != Some("--bus-callback") {
        return None;
    }
    if args.get(2).map(String::as_str) == Some("claude-statusline") {
        if args.len() == 3 {
            statusline::run();
        }
        return Some(Ok(()));
    }
    // Separate rotating-log writers from the spool writer. Diagnostic failure
    // must never prevent saving the callback, and must not create a log root.
    let _diagnostics_lease = diagnostics_dir
        .as_ref()
        .filter(|dir| dir.is_dir())
        .and_then(|dir| spool::append_lock(&dir.join("diagnostics.lock")).ok());
    if _diagnostics_lease.is_some() {
        if let Some(dir) = diagnostics_dir {
            logging::init_file_logging_at(dir, "hook.log", logging_options);
        }
    }
    tracing::debug!(event = "bus.callback.capture", "Provider hook invoked");
    let result = capture(args, io::stdin().lock());
    // Keep the old stdout bytes and write-failure behavior; observation hooks
    // must never inject context or approve a permission prompt.
    if let Err(error) = writeln!(io::stdout().lock(), "{{}}") {
        panic!("failed printing to stdout: {error}");
    }
    if let Err(error) = result {
        tracing::warn!(event = "bus.callback.capture_failed", error_kind = ?error.kind(), "Provider hook capture failed");
        eprintln!("Bus callback: {error}");
    }
    Some(Ok(()))
}

fn capture(args: &[String], input: impl Read) -> io::Result<()> {
    let (Some(dir), Some(launch)) = (
        std::env::var_os("BUS_CALLBACK_DIR"),
        std::env::var_os("BUS_LAUNCH_ID"),
    ) else {
        return Ok(());
    };
    if args.len() != 3 {
        return Err(io::Error::other("Invalid Bus callback arguments"));
    }
    let provider = match args[2].as_str() {
        "codex-hook" => ProviderKind::Codex,
        "claude-hook" => ProviderKind::ClaudeCode,
        "cursor-hook" => ProviderKind::Cursor,
        _ => return Err(io::Error::other("Unknown Bus callback provider")),
    };
    let mut bytes = Vec::new();
    input
        .take(spool::MAX_CALLBACK_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > spool::MAX_CALLBACK_BYTES {
        return Err(io::Error::other("Bus callback exceeds 2 MiB"));
    }
    let value = serde_json::from_slice(&bytes)?;
    if matches!(spool::parse(provider, &value), Ok(spool::Parsed::Ignore)) {
        tracing::debug!(event = "bus.callback.ignored", provider = ?provider,
            reason = "not_interactive_or_unhandled", "Hook does not represent a terminal reply");
        return Ok(());
    }
    spool::append(Path::new(&dir), &launch.to_string_lossy(), provider, value)
}
