//! Observation-only hook entry, before ordinary CLI parsing and session checks.
use super::{claude_code::statusline, spool, ProviderKind};
use crate::utils::logging::{self, LoggingOptions};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// The Bus server's pid, set on every pane. Its child is the pane shell, so a hook
/// can tell the pane's own agent from a provider that agent started.
pub(crate) const BUS_SERVER_PID_ENV_VAR: &str = "HERDR_SERVER_PID";

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
    let parsed = spool::parse(provider, &value);
    if matches!(parsed, Ok(spool::Parsed::Ignore)) {
        tracing::debug!(event = "bus.callback.ignored", provider = ?provider,
            reason = "not_interactive_or_unhandled", "Hook does not represent a terminal reply");
        return Ok(());
    }
    let ancestry = crate::platform::process_ancestry(std::process::id());
    let nested_provider = runs_under_nested_provider(&ancestry);
    if nested_provider {
        tracing::info!(event = "bus.callback.nested_provider", provider = ?provider,
            "Hook ran under a provider the agent started; the agent ignores it");
    }
    // A session start names its process chain so the server can tell a late report
    // from an exited agent apart from its replacement's.
    let reporter = if matches!(parsed, Ok(spool::Parsed::Session { .. })) {
        ancestry
    } else {
        Vec::new()
    };
    spool::append(
        Path::new(&dir),
        &launch.to_string_lossy(),
        provider,
        value,
        &reporter,
        nested_provider,
    )
}

/// Whether more than one provider runs between this hook and its pane's shell, the
/// child of the Bus server that `HERDR_SERVER_PID` names. A pane started by an older
/// server has no such variable, and its hooks keep counting as the agent's.
fn runs_under_nested_provider(ancestry: &[crate::platform::ProcessInstance]) -> bool {
    let Some(server) = std::env::var(BUS_SERVER_PID_ENV_VAR)
        .ok()
        .and_then(|pid| pid.parse::<u32>().ok())
    else {
        return false;
    };
    let Some(ancestors) = ancestry
        .iter()
        .position(|process| process.pid == server)
        .and_then(|server| ancestry.get(1..server))
    else {
        return false;
    };
    let providers: Vec<_> = ancestors
        .iter()
        .map(|process| {
            crate::platform::process_info(process.pid)
                .as_ref()
                .and_then(crate::agents::provider_process)
        })
        .collect();
    crate::agents::runs_nested_provider(&providers)
}
