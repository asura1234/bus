//! Shared bounds for a native prompt, including withdrawal before Enter.
use super::schema::Method;
use std::time::Duration;

pub(crate) const SUBMIT_DELAY: Duration = Duration::from_millis(300);
const CONFIRMATION_GRACE: Duration = Duration::from_secs(2);
/// Minimum withdrawal allowance and maximum interval without observed progress.
pub(crate) const WITHDRAW_GRACE: Duration = Duration::from_secs(1);
pub(crate) const CLEAR_KEY_DELAY: Duration = Duration::from_millis(25);
/// Allow response delivery and scheduling beyond the native writer bound.
/// Dispatch consumes the native budget, measured from socket receipt.
pub(crate) const RPC_MARGIN: Duration = Duration::from_secs(1);
/// Let a writer publish its last withdrawal result before timing out the
/// response. The rest of RPC_MARGIN remains available for socket delivery.
pub(crate) const RESPONSE_GRACE: Duration =
    Duration::from_millis(RPC_MARGIN.as_millis() as u64 / 10);

pub(crate) fn submit_delay(codex: bool, prompt_bytes: usize) -> Duration {
    #[cfg(windows)]
    if codex {
        // ConPTY paste consumption plus Codex's Enter suppression window.
        return Duration::from_millis(600 + prompt_bytes as u64 / 4);
    }
    let _ = (codex, prompt_bytes);
    SUBMIT_DELAY
}

pub(crate) fn confirmation_budget(delay: Duration) -> Duration {
    delay + CONFIRMATION_GRACE
}

/// Each logical line can require three edits. Reserve their spacing plus an
/// equal I/O/render allowance; CRLF is one separator. Folded pastes may clear
/// faster, while an unresponsive composer retains the short no-progress bound.
pub(crate) fn withdrawal_budget(text: &[u8]) -> Duration {
    let mut lines = 1_u32;
    let mut previous = 0;
    for &byte in text {
        if byte == b'\r' || byte == b'\n' && previous != b'\r' {
            lines = lines.saturating_add(1);
        }
        previous = byte;
    }
    WITHDRAW_GRACE.max(CLEAR_KEY_DELAY.saturating_mul(lines.saturating_mul(6)))
}

pub(crate) fn maximum_budget(delay: Duration, text: &[u8]) -> Duration {
    confirmation_budget(delay) + withdrawal_budget(text)
}

/// The guarded calls identify their provider. An unguarded prompt may target
/// any provider, so reserve the largest applicable paste delay for its text.
pub(crate) fn method_budget(method: &Method) -> Option<Duration> {
    let (delay, text) = match method {
        Method::AgentPromptIfIdle(params) => (
            submit_delay(params.expected_agent == "codex", params.text.len()),
            params.text.as_bytes(),
        ),
        Method::AgentPromptIfUnbound(params) => (
            submit_delay(true, params.text.len()),
            params.text.as_bytes(),
        ),
        Method::AgentPrompt(params) => (
            submit_delay(true, params.text.len()).max(SUBMIT_DELAY),
            params.text.as_bytes(),
        ),
        _ => return None,
    };
    Some(maximum_budget(delay, text))
}
