//! Prompt directives that make a fake provider act out a turn: `fake:work N`,
//! `fake:dialog N`, `fake:api-error`, `fake:shell NAME` and `fake:wake N`.
use serde_json::{json, Value};
use std::io::Write;

/// A prompt containing `fake:work N` keeps the fake busy for N seconds (at
/// most 600), so a driver can see a Working agent and a live spinner.
pub(super) fn work_seconds(prompt: &str) -> Option<u64> {
    directive_seconds(prompt, "fake:work")
}

/// A prompt containing `fake:dialog N` holds its turn on [`DIALOG`] for N
/// seconds (at most 600), so a driver can see an agent wait on a dialog.
pub(super) fn dialog_seconds(prompt: &str) -> Option<u64> {
    directive_seconds(prompt, "fake:dialog")
}

/// A Claude prompt containing `fake:shell NAME` starts background shell NAME
/// in that turn: the turn's `Stop` lists it running, as Claude Code's does, so
/// Bus holds the reply for it.
pub(super) fn shell_name(prompt: &str) -> Option<String> {
    let rest = prompt.split("fake:shell").nth(1)?;
    rest.split_whitespace().next().map(str::to_owned)
}

/// A Claude prompt containing `fake:wake N` ends its newest shell N seconds
/// after the turn's `Stop` (at most 600): the fake wakes a turn with the task
/// notification, as Claude Code does, works a second, and returns to its
/// prompt box without a `Stop`, as Claude Code does when it folds the next
/// notification into that turn.
pub(super) fn wake_seconds(prompt: &str) -> Option<u64> {
    directive_seconds(prompt, "fake:wake")
}

fn directive_seconds(prompt: &str, directive: &str) -> Option<u64> {
    let rest = prompt.split(directive).nth(1)?;
    let seconds: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(seconds.min(600))
}

/// A permission dialog as Claude Code draws one, which Bus reads off the
/// screen as the agent waiting on a dialog.
pub(super) const DIALOG: &str =
    "Do you want to proceed?\r\n\u{276f} 1. Yes\r\n  2. No\r\n\r\nEsc to cancel\r\n";

/// Shows [`DIALOG`] for `seconds`, then clears it. Nothing is chosen: the
/// dialog only holds the turn, which then answers as usual.
pub(super) fn show_dialog(seconds: u64) {
    let mut out = std::io::stdout();
    let _ = write!(out, "{DIALOG}");
    let _ = out.flush();
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    let rows = DIALOG.matches("\r\n").count();
    let _ = write!(out, "\x1b[{rows}A\r\x1b[J");
    let _ = out.flush();
}

/// A Claude prompt containing `fake:api-error` ends its turn the way an API
/// error ends a real one: no reply, and a StopFailure hook with the payload
/// measured from Claude Code 2.1.296. Returns that payload's extra fields.
pub(super) fn api_error(provider: &str, prompt: &str) -> Option<Value> {
    (provider == "claude" && prompt.contains("fake:api-error")).then(|| {
        json!({
            "error": "server_error",
            "last_assistant_message":
                "API Error: Connection lost mid-response. The response above may be incomplete.",
        })
    })
}
