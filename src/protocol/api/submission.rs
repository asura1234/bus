//! Shared bounds for a native prompt, including withdrawal before Enter.
use super::schema::Method;
use std::time::Duration;

pub(crate) const SUBMIT_DELAY: Duration = Duration::from_millis(300);
const CONFIRMATION_GRACE: Duration = Duration::from_secs(2);
pub(crate) const WITHDRAW_GRACE: Duration = Duration::from_secs(1);
/// Allow dispatch, response delivery and scheduling beyond the native bound.
pub(crate) const RPC_MARGIN: Duration = Duration::from_secs(1);

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

pub(crate) fn maximum_budget(delay: Duration) -> Duration {
    confirmation_budget(delay) + WITHDRAW_GRACE
}

/// The guarded calls identify their provider. An unguarded prompt may target
/// any provider, so reserve the largest applicable paste delay for its text.
pub(crate) fn method_budget(method: &Method) -> Option<Duration> {
    let delay = match method {
        Method::AgentPromptIfIdle(params) => {
            submit_delay(params.expected_agent == "codex", params.text.len())
        }
        Method::AgentPromptIfUnbound(params) => submit_delay(true, params.text.len()),
        Method::AgentPrompt(params) => submit_delay(true, params.text.len()).max(SUBMIT_DELAY),
        _ => return None,
    };
    Some(maximum_budget(delay))
}
