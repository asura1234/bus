//! Grok uses first-message instructions and durable Bus-delivered wakes.
use std::path::Path;

const GROK_GUIDE: &str = include_str!("../../../../orchestration/providers/grok.md");

/// The caller has already filled room/name/docs values. Preserve the authored
/// text verbatim: another fill pass would expand placeholder-like inserted values.
pub(crate) fn orchestrator_prompt(text: &str) -> Result<String, String> {
    Ok(format!(
        "{GROK_GUIDE}\n\n## Authored room instructions\n\n{text}"
    ))
}

/// Fresh and adopted sessions both use the first-message bootstrap. Cold resume
/// uses this same policy and retains the session's instructions and queued work.
pub(crate) fn prompt_args(_path: &Path, _adopted: bool) -> Result<Option<Vec<String>>, String> {
    Ok(None)
}

pub(crate) fn prompt_message(text: &str) -> String {
    format!(
        "Bus: these are your Grok orchestrator instructions. Follow them for the rest of this session, then reply briefly that you are ready. Do not dispatch or replay worker tasks during this bootstrap. If this session already has work in progress, retain its context and reconcile durable Bus state when work continues.\n\n{text}"
    )
}

#[cfg(test)]
#[path = "tests/system_prompt_test.rs"]
mod tests;
