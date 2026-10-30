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

#[cfg(test)]
#[path = "tests/callback_metadata_test.rs"]
mod metadata_tests;

/// Process composition supplies the inherited spool log root and logging policy.
pub(crate) fn dispatch(
    args: &[String],
    diagnostics_dir: Option<PathBuf>,
    logging_options: &LoggingOptions,
) -> Option<io::Result<()>> {
    if args.get(1).map(String::as_str) != Some("--bus-callback") {
        return None;
    }
    if args.get(2).map(String::as_str) == Some("draft-editor") {
        return Some(crate::platform::draft_editor::run(args));
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
        .and_then(|dir| spool::try_append_lock(&dir.join("diagnostics.lock")).ok());
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
    if let Err(error) = &result {
        tracing::warn!(event = "bus.callback.capture_failed", error_kind = ?error.kind(), "Provider hook capture failed");
        eprintln!("Bus callback: {error}");
    }
    Some(result)
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
        #[cfg(feature = "dev-tools")]
        "fake-hook"
            if std::env::var_os("BUS_TUI_FAKE_AGENT").is_some()
                && std::env::var_os("BUS_TUI_OWNER").is_some() =>
        {
            ProviderKind::Fake
        }
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
    observe_payload(provider, &value, &parsed);
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

/// Retain the evidence that disappears when an ignored/invalid hook is discarded.
/// Only field shapes and bounded correlation tokens are safe here, never text,
/// paths, arbitrary event names or the parser's payload-derived error string.
fn observe_payload(
    provider: ProviderKind,
    value: &serde_json::Value,
    parsed: &Result<spool::Parsed, String>,
) {
    let hook_event = match value
        .get("hook_event_name")
        .and_then(serde_json::Value::as_str)
    {
        Some(
            event @ ("SessionStart" | "UserPromptSubmit" | "Stop" | "StopFailure" | "sessionStart"
            | "beforeSubmitPrompt" | "afterAgentResponse" | "stop"),
        ) => event,
        Some(_) => "other",
        None => "missing_or_invalid",
    };
    let token = |key| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| {
                !text.is_empty()
                    && text.len() <= 128
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
            })
    };
    let session_id = token("session_id").or_else(|| token("conversation_id"));
    let turn_id = token("turn_id")
        .or_else(|| token("prompt_id"))
        .or_else(|| token("generation_id"));
    let final_value = value.get(if provider == ProviderKind::Cursor {
        "text"
    } else {
        "last_assistant_message"
    });
    let final_state = match final_value {
        None => "missing",
        Some(serde_json::Value::Null) => "null",
        Some(serde_json::Value::String(text)) if text.is_empty() => "empty",
        Some(serde_json::Value::String(_)) => "text",
        Some(_) => "invalid_type",
    };
    let final_bytes = final_value
        .and_then(serde_json::Value::as_str)
        .map(str::len);
    let transcript_present = value
        .get("transcript_path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|path| !path.is_empty());
    let parse_outcome = match parsed {
        Ok(spool::Parsed::Ignore) => "ignored",
        Ok(_) => "parsed",
        Err(_) => "invalid",
    };
    tracing::info!(
        event = "bus.callback.payload_shape",
        ?provider,
        hook_event,
        ?session_id,
        ?turn_id,
        transcript_present,
        final_state,
        ?final_bytes,
        parse_outcome,
        "Provider hook correlation metadata"
    );
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
