//! Prompt directives that make a fake provider act out a turn: `fake:work N`
//! and `fake:api-error`.
use super::working_line;
use serde_json::{json, Value};
use std::io::Write;
use std::time::Instant;

/// A prompt containing `fake:work N` keeps the fake busy for N seconds (at
/// most 600), so a driver can see a Working agent and a live spinner.
pub(super) fn work_seconds(prompt: &str) -> Option<u64> {
    let rest = prompt.split("fake:work").nth(1)?;
    let seconds: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(seconds.min(600))
}

/// Claude's working line, redrawn in place: `✻ Working… (3s · esc to
/// interrupt)`. Claude hides its prompt box meanwhile; input waits.
pub(super) fn spin(seconds: u64) {
    let started = Instant::now();
    let mut out = std::io::stdout();
    let mut index = 0;
    while started.elapsed().as_secs() < seconds {
        let _ = write!(
            out,
            "\r\x1b[2K{}",
            working_line("claude", index, started.elapsed().as_secs())
        );
        let _ = out.flush();
        index += 1;
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    let _ = write!(out, "\r\x1b[2K");
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
